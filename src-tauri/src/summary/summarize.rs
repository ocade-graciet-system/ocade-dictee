//! Orchestration du résumé : découpage, notes intermédiaires, compte-rendu
//! final, contrôle du gabarit et relance ciblée. Indépendant de Tauri : la
//! complétion est abstraite par `Completion` (vrai serveur ou double de test).

use futures_util::future::BoxFuture;
use tokio_util::sync::CancellationToken;

use super::chunking::{estimate_tokens, split_into_chunks, MAX_CHUNK_TOKENS};
use super::prompt::{
    build_direct_messages, build_final_messages, build_notes_messages, check_template,
    clean_output, ChatMessages, CONCISE_REMINDER, STRICT_REMINDER,
};
use super::server::LlamaServer;
use super::SummaryError;
use crate::llm_client::{send_chat_completion_with_options, ChatSamplingOptions};
use crate::settings::PostProcessProvider;

pub const TEMPERATURE: f64 = 0.2;
/// Plafond de tokens du compte-rendu (direct ou final). L'essai réel coupait
/// toutes les sorties à 1 200 tokens : un compte-rendu de 450 mots en français
/// en coûte davantage, d'où 1 800 (le contexte de 12 288 tokens l'absorbe, cf.
/// `chunking::MAX_CHUNK_TOKENS`).
pub const MAX_TOKENS_SUMMARY: u32 = 1_800;
/// Plafond de tokens des notes d'une tranche (jusqu'à 25 puces détaillées).
pub const MAX_TOKENS_NOTES: u32 = 1_400;
/// Tours de notes au plus : les notes de la 2e passe (≥ 6 h d'audio) sont
/// condensées à nouveau ; au-delà on synthétise quoi qu'il arrive.
pub const MAX_NOTE_ROUNDS: usize = 3;

/// Sortie d'une complétion : le texte rendu et `truncated`, vrai quand le
/// moteur a coupé au plafond de tokens (`finish_reason: "length"`). Un texte
/// amputé n'est pas une réponse : l'orchestration le relance (compte-rendu) ou
/// s'en contente sciemment (notes, recondensées ensuite).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionOutput {
    pub text: String,
    pub truncated: bool,
}

/// Une complétion de chat (système + utilisateur → texte).
pub trait Completion: Sync {
    fn complete<'a>(
        &'a self,
        messages: &'a ChatMessages,
        max_tokens: u32,
    ) -> BoxFuture<'a, Result<CompletionOutput, SummaryError>>;
}

/// Complétion via le `llama-server` local, avec un `PostProcessProvider`
/// construit en mémoire (jamais enregistré dans les réglages).
pub struct LlamaCompletion<'a> {
    pub server: &'a LlamaServer,
    pub model_name: &'a str,
}

impl LlamaCompletion<'_> {
    fn provider(&self) -> PostProcessProvider {
        PostProcessProvider {
            id: "llama-local".to_string(),
            label: "llama.cpp local".to_string(),
            base_url: self.server.base_url(),
            allow_base_url_edit: false,
            models_endpoint: None,
            supports_structured_output: false,
        }
    }
}

impl Completion for LlamaCompletion<'_> {
    fn complete<'b>(
        &'b self,
        messages: &'b ChatMessages,
        max_tokens: u32,
    ) -> BoxFuture<'b, Result<CompletionOutput, SummaryError>> {
        Box::pin(async move {
            let provider = self.provider();
            let result = send_chat_completion_with_options(
                &provider,
                self.server.api_key().to_string(),
                self.model_name,
                messages.user.clone(),
                Some(messages.system.clone()),
                ChatSamplingOptions {
                    temperature: Some(TEMPERATURE),
                    max_tokens: Some(max_tokens),
                },
            )
            .await;
            match result {
                Ok(outcome) => match outcome.content {
                    Some(text) => Ok(CompletionOutput {
                        text,
                        truncated: outcome.truncated,
                    }),
                    None => Err(SummaryError::IncompleteOutput),
                },
                Err(detail) => Err(SummaryError::EngineStartFailed {
                    detail: format!("requête au moteur échouée: {detail}"),
                }),
            }
        })
    }
}

/// Une complétion, abandonnée dès l'annulation. `biased` : le jeton est
/// examiné avant la réponse, si bien qu'une annulation déjà posée gagne
/// toujours — aucun appel n'est lancé après coup.
async fn complete_cancellable(
    completion: &dyn Completion,
    cancel: &CancellationToken,
    messages: &ChatMessages,
    max_tokens: u32,
) -> Result<CompletionOutput, SummaryError> {
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(SummaryError::Cancelled),
        result = completion.complete(messages, max_tokens) => result,
    }
}

/// Compte-rendu (direct ou final) et son unique relance, ciblée sur la cause :
/// rappel de concision si la sortie a été coupée, rappel strict si le gabarit
/// n'est pas respecté. Une seule relance en tout : au second échec, le
/// compte-rendu est déclaré incomplet.
async fn summary_with_single_retry(
    completion: &dyn Completion,
    cancel: &CancellationToken,
    messages: &ChatMessages,
) -> Result<String, SummaryError> {
    let first = complete_cancellable(completion, cancel, messages, MAX_TOKENS_SUMMARY).await?;
    let text = clean_output(&first.text);
    // La troncature passe avant le gabarit : une sortie coupée peut rester
    // conforme (titre et deux sections écrits, la fin manque).
    let reminder = if first.truncated {
        log::warn!(
            "Compte-rendu coupé au plafond de {MAX_TOKENS_SUMMARY} tokens, relance avec rappel de concision"
        );
        CONCISE_REMINDER
    } else {
        match check_template(&text) {
            Ok(()) => return Ok(text),
            Err(issue) => {
                log::warn!("Gabarit incomplet ({issue:?}), relance avec rappel strict");
                STRICT_REMINDER
            }
        }
    };
    let retry = ChatMessages {
        system: messages.system.clone(),
        user: format!("{}\n\n{reminder}", messages.user),
    };
    let second = complete_cancellable(completion, cancel, &retry, MAX_TOKENS_SUMMARY).await?;
    let text = clean_output(&second.text);
    if second.truncated {
        log::warn!("Compte-rendu encore coupé après relance, abandon");
        return Err(SummaryError::IncompleteOutput);
    }
    match check_template(&text) {
        Ok(()) => Ok(text),
        Err(issue) => {
            log::warn!("Gabarit toujours incomplet après relance: {issue:?}");
            Err(SummaryError::IncompleteOutput)
        }
    }
}

/// Résume `text` : une tranche → compte-rendu direct ; plusieurs → notes par
/// tranche puis compte-rendu final. `on_progress(partie, total)` suit les
/// tranches, la partie en cours ne redescendant jamais. Gabarit et troncature
/// contrôlés ; une relance ciblée au plus, sinon `IncompleteOutput`.
pub async fn summarize_text(
    text: &str,
    completion: &dyn Completion,
    cancel: &CancellationToken,
    on_progress: &mut (dyn FnMut(u32, u32) + Send),
) -> Result<String, SummaryError> {
    let mut chunks = split_into_chunks(text, MAX_CHUNK_TOKENS);
    if chunks.is_empty() {
        return Err(SummaryError::IncompleteOutput);
    }

    let messages = if chunks.len() == 1 {
        on_progress(1, 1);
        build_direct_messages(&chunks[0])
    } else {
        // Parties traitées depuis le début, tous tours confondus : un second
        // tour de condensation allonge le total plutôt que de renvoyer la
        // progression à 1.
        let mut done = 0u32;
        let mut round = 0usize;
        loop {
            let round_total = chunks.len() as u32;
            let total = done + round_total;
            let mut notes = Vec::with_capacity(chunks.len());
            for (index, part) in chunks.iter().enumerate() {
                done += 1;
                on_progress(done, total);
                let part_messages = build_notes_messages(part, index as u32 + 1, round_total);
                let output =
                    complete_cancellable(completion, cancel, &part_messages, MAX_TOKENS_NOTES)
                        .await?;
                if output.truncated {
                    // Accepté sans relance : ces notes sont de toute façon
                    // recondensées par le compte-rendu final.
                    log::warn!(
                        "Notes de la partie {} sur {round_total} coupées au plafond de {MAX_TOKENS_NOTES} tokens",
                        index + 1
                    );
                }
                notes.push(output.text);
            }
            let joined = notes.join("\n\n");
            if joined.trim().is_empty() {
                log::warn!("Notes vides sur les {round_total} parties, compte-rendu impossible");
                return Err(SummaryError::IncompleteOutput);
            }
            round += 1;
            if estimate_tokens(&joined) <= MAX_CHUNK_TOKENS || round >= MAX_NOTE_ROUNDS {
                on_progress(total, total);
                break build_final_messages(&notes);
            }
            log::info!(
                "Notes trop longues ({} tokens estimés), nouvelle passe de condensation",
                estimate_tokens(&joined)
            );
            chunks = split_into_chunks(&joined, MAX_CHUNK_TOKENS);
        }
    };

    summary_with_single_retry(completion, cancel, &messages).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    const GOOD: &str = "# Titre\n\n## Résumé\nPhrase.\n\n## Points clés\n- point\n";
    const BAD: &str = "Voici un résumé sans structure.";

    /// Double de test : rejoue des réponses (texte, tronqué) dans l'ordre et
    /// journalise les prompts reçus avec leur plafond de tokens.
    struct Scripted {
        replies: Mutex<Vec<CompletionOutput>>,
        calls: Mutex<Vec<(String, u32)>>,
    }

    impl Scripted {
        /// Réponses complètes (jamais tronquées).
        fn new<S: AsRef<str>>(replies: &[S]) -> Self {
            let replies = replies
                .iter()
                .map(|text| (text.as_ref(), false))
                .collect::<Vec<_>>();
            Self::truncating(&replies)
        }

        /// Réponses avec leur indicateur de troncature.
        fn truncating(replies: &[(&str, bool)]) -> Self {
            Self {
                replies: Mutex::new(
                    replies
                        .iter()
                        .rev()
                        .map(|(text, truncated)| CompletionOutput {
                            text: (*text).to_string(),
                            truncated: *truncated,
                        })
                        .collect(),
                ),
                calls: Mutex::new(Vec::new()),
            }
        }

        /// Prompts reçus, dans l'ordre : (message utilisateur, plafond).
        fn calls(&self) -> Vec<(String, u32)> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl Completion for Scripted {
        fn complete<'a>(
            &'a self,
            messages: &'a ChatMessages,
            max_tokens: u32,
        ) -> BoxFuture<'a, Result<CompletionOutput, SummaryError>> {
            Box::pin(async move {
                self.calls
                    .lock()
                    .unwrap()
                    .push((messages.user.clone(), max_tokens));
                Ok(self
                    .replies
                    .lock()
                    .unwrap()
                    .pop()
                    .expect("réponse scriptée manquante"))
            })
        }
    }

    /// Transcription assez longue pour tenir en trois tranches.
    fn three_chunk_text() -> String {
        let text = "Le conseil a validé le budget de trente mille euros pour le second semestre. "
            .repeat(1000);
        assert_eq!(
            split_into_chunks(&text, MAX_CHUNK_TOKENS).len(),
            3,
            "le découpage doit donner trois tranches"
        );
        text
    }

    #[tokio::test]
    async fn short_text_is_summarized_directly() {
        let scripted = Scripted::new(&[GOOD]);
        let mut progress = Vec::new();
        let out = summarize_text(
            "Bonjour à tous. Merci.",
            &scripted,
            &CancellationToken::new(),
            &mut |c, t| progress.push((c, t)),
        )
        .await
        .unwrap();
        assert_eq!(out, GOOD.trim());
        assert_eq!(progress, vec![(1, 1)]);
        let calls = scripted.calls();
        assert_eq!(calls.len(), 1);
        assert!(calls[0]
            .0
            .ends_with("Transcription : Bonjour à tous. Merci."));
        assert_eq!(calls[0].1, MAX_TOKENS_SUMMARY);
    }

    #[tokio::test]
    async fn bad_template_is_retried_once_with_strict_reminder() {
        let scripted = Scripted::new(&[BAD, GOOD]);
        let out = summarize_text(
            "Texte.",
            &scripted,
            &CancellationToken::new(),
            &mut |_, _| {},
        )
        .await
        .unwrap();
        assert_eq!(out, GOOD.trim());
        let calls = scripted.calls();
        assert_eq!(calls.len(), 2);
        assert!(calls[1].0.ends_with(STRICT_REMINDER));
        assert!(calls[1].0.starts_with(&calls[0].0));
    }

    #[tokio::test]
    async fn two_bad_outputs_give_incomplete_output() {
        let scripted = Scripted::new(&[BAD, "# Titre\n## Conclusion\nx"]);
        let err = summarize_text(
            "Texte.",
            &scripted,
            &CancellationToken::new(),
            &mut |_, _| {},
        )
        .await
        .unwrap_err();
        assert_eq!(err, SummaryError::IncompleteOutput);
        assert_eq!(scripted.calls().len(), 2, "une seule relance");
    }

    #[tokio::test]
    async fn truncated_summary_is_retried_with_concise_reminder() {
        // Gabarit conforme mais sortie coupée : la troncature l'emporte.
        let scripted = Scripted::truncating(&[(GOOD, true), (GOOD, false)]);
        let out = summarize_text(
            "Texte.",
            &scripted,
            &CancellationToken::new(),
            &mut |_, _| {},
        )
        .await
        .unwrap();
        assert_eq!(out, GOOD.trim());
        let calls = scripted.calls();
        assert_eq!(calls.len(), 2);
        assert!(calls[1].0.ends_with(CONCISE_REMINDER));
        assert!(!calls[1].0.contains(STRICT_REMINDER));
    }

    #[tokio::test]
    async fn twice_truncated_summary_gives_incomplete_output() {
        let scripted = Scripted::truncating(&[(GOOD, true), (GOOD, true)]);
        let err = summarize_text(
            "Texte.",
            &scripted,
            &CancellationToken::new(),
            &mut |_, _| {},
        )
        .await
        .unwrap_err();
        assert_eq!(err, SummaryError::IncompleteOutput);
        assert_eq!(scripted.calls().len(), 2, "une seule relance");
    }

    #[tokio::test]
    async fn long_text_goes_through_notes_then_final() {
        let text = three_chunk_text();
        let scripted = Scripted::new(&["- note 1", "- note 2", "- note 3", GOOD]);
        let mut progress = Vec::new();
        let out = summarize_text(&text, &scripted, &CancellationToken::new(), &mut |c, t| {
            progress.push((c, t))
        })
        .await
        .unwrap();
        assert_eq!(out, GOOD.trim());
        assert_eq!(progress, vec![(1, 3), (2, 3), (3, 3), (3, 3)]);
        let calls = scripted.calls();
        assert_eq!(calls.len(), 4);
        assert!(calls[0].0.starts_with("Voici la partie 1 sur 3"));
        assert_eq!(calls[0].1, MAX_TOKENS_NOTES);
        assert!(calls[3]
            .0
            .starts_with("Voici les notes prises sur les 3 parties"));
        assert!(calls[3].0.contains("- note 2"));
        assert_eq!(calls[3].1, MAX_TOKENS_SUMMARY);
    }

    #[tokio::test]
    async fn truncated_notes_are_kept_without_retry() {
        let text = three_chunk_text();
        let scripted = Scripted::truncating(&[
            ("- note 1", true),
            ("- note 2", true),
            ("- note 3", false),
            (GOOD, false),
        ]);
        let out = summarize_text(&text, &scripted, &CancellationToken::new(), &mut |_, _| {})
            .await
            .unwrap();
        assert_eq!(out, GOOD.trim());
        let calls = scripted.calls();
        assert_eq!(calls.len(), 4, "aucune relance sur les notes");
        assert!(calls[3].0.contains("- note 1"));
    }

    #[tokio::test]
    async fn second_notes_round_condenses_again_and_keeps_progress_monotonic() {
        let text = three_chunk_text();
        // Des notes assez longues pour ne pas tenir en une tranche : elles
        // repassent par un tour de condensation avant le compte-rendu.
        let long_note = "- Le budget a été validé. ".repeat(480);
        assert!(estimate_tokens(&vec![long_note.clone(); 3].join("\n\n")) > MAX_CHUNK_TOKENS);
        let replies = vec![
            long_note.clone(),
            long_note.clone(),
            long_note,
            "- condensé 1".to_string(),
            "- condensé 2".to_string(),
            GOOD.to_string(),
        ];
        let scripted = Scripted::new(&replies);
        let mut progress = Vec::new();
        let out = summarize_text(&text, &scripted, &CancellationToken::new(), &mut |c, t| {
            progress.push((c, t))
        })
        .await
        .unwrap();
        assert_eq!(out, GOOD.trim());
        assert_eq!(
            progress,
            vec![(1, 3), (2, 3), (3, 3), (4, 5), (5, 5), (5, 5)],
            "la partie en cours ne redescend jamais"
        );
        let calls = scripted.calls();
        assert_eq!(calls.len(), 6);
        // Le prompt numérote les parties du tour courant, pas le compteur global.
        assert!(calls[3].0.starts_with("Voici la partie 1 sur 2"));
        assert!(calls[5]
            .0
            .starts_with("Voici les notes prises sur les 2 parties"));
        assert!(calls[5].0.contains("- condensé 2"));
    }

    #[tokio::test]
    async fn empty_notes_give_incomplete_output() {
        let text = three_chunk_text();
        let scripted = Scripted::new(&["", "", ""]);
        let err = summarize_text(&text, &scripted, &CancellationToken::new(), &mut |_, _| {})
            .await
            .unwrap_err();
        assert_eq!(err, SummaryError::IncompleteOutput);
        assert_eq!(scripted.calls().len(), 3, "pas de compte-rendu sans notes");
    }

    #[tokio::test]
    async fn cancellation_wins_over_a_pending_completion() {
        struct Hanging;
        impl Completion for Hanging {
            fn complete<'a>(
                &'a self,
                _: &'a ChatMessages,
                _: u32,
            ) -> BoxFuture<'a, Result<CompletionOutput, SummaryError>> {
                Box::pin(async {
                    std::future::pending::<Result<CompletionOutput, SummaryError>>().await
                })
            }
        }
        let cancel = CancellationToken::new();
        let later = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            later.cancel();
        });
        let err = summarize_text("Texte.", &Hanging, &cancel, &mut |_, _| {})
            .await
            .unwrap_err();
        assert_eq!(err, SummaryError::Cancelled);
    }

    #[tokio::test]
    async fn an_already_cancelled_token_stops_before_the_first_call() {
        let scripted = Scripted::new(&[GOOD]);
        let cancel = CancellationToken::new();
        cancel.cancel();
        let err = summarize_text("Texte.", &scripted, &cancel, &mut |_, _| {})
            .await
            .unwrap_err();
        assert_eq!(err, SummaryError::Cancelled);
        assert!(scripted.calls().is_empty(), "rien n'est envoyé au moteur");
    }

    #[tokio::test]
    async fn empty_text_is_incomplete() {
        let scripted = Scripted::new::<&str>(&[]);
        let err = summarize_text("   ", &scripted, &CancellationToken::new(), &mut |_, _| {})
            .await
            .unwrap_err();
        assert_eq!(err, SummaryError::IncompleteOutput);
    }

    /// Test d'intégration réel, ignoré par défaut : lance le vrai
    /// `llama-server` avec le vrai modèle s'ils sont présents localement.
    ///
    /// OCADE_LLAMA_SERVER=<chemin de llama-server> OCADE_SUMMARY_MODEL=<chemin .gguf> \
    ///   cargo test --lib summary::summarize::tests::integration_real -- --ignored --nocapture
    #[tokio::test]
    #[ignore]
    async fn integration_real_llama_server_summarizes_300_words() {
        use super::super::server::{build_local_client, default_gpu_layers, LlamaServerConfig};
        let exe = std::env::var("OCADE_LLAMA_SERVER").expect("OCADE_LLAMA_SERVER");
        let model = std::env::var("OCADE_SUMMARY_MODEL").expect("OCADE_SUMMARY_MODEL");
        let client = build_local_client().unwrap();
        let server = LlamaServer::start(
            LlamaServerConfig {
                exe: exe.into(),
                model: model.into(),
                ctx_size: super::super::assets::CTX_SIZE,
                threads: super::super::system::physical_cores(),
                gpu_layers: default_gpu_layers(),
                extra_args: super::super::assets::MODEL
                    .server_args
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
                pid_file: None,
            },
            &client,
        )
        .await
        .unwrap();
        let completion = LlamaCompletion {
            server: &server,
            model_name: super::super::assets::MODEL.model_name,
        };
        let text = "Bonjour à tous, merci d'être présents pour cette réunion de lancement du projet Horizon. \
            Nous avons trois sujets : le budget, le calendrier et l'équipe. Sur le budget, la direction a validé \
            une enveloppe de quarante-cinq mille euros pour le premier semestre, dont dix mille pour la communication. \
            Le calendrier prévoit une première livraison le quinze novembre et une mise en production le premier mars. \
            Marie prend en charge la coordination avec le prestataire, Julien rédige le cahier des charges avant vendredi, \
            et je m'occupe du point hebdomadaire chaque lundi à neuf heures. Nous avons aussi décidé de reporter \
            l'achat du nouveau serveur au deuxième semestre pour ne pas dépasser l'enveloppe. Une question est restée \
            ouverte sur la formation des utilisateurs : Sophie fera une proposition d'ici la fin du mois. Merci à tous."
            .to_string();
        let started = std::time::Instant::now();
        let result = summarize_text(
            &text,
            &completion,
            &CancellationToken::new(),
            &mut |c, t| eprintln!("partie {c}/{t}"),
        )
        .await;
        let elapsed = started.elapsed();
        // Le serveur est arrêté avant les assertions : un échec ne doit pas
        // laisser un processus derrière lui.
        server.kill();
        let out = result.unwrap();
        eprintln!("--- compte-rendu en {elapsed:?} ---\n{out}");
        assert!(check_template(&out).is_ok());
        assert!(out.contains("Horizon") || out.contains("45") || out.contains("quarante"));
    }
}
