//! Rédaction du compte rendu avec le modèle local : passe directe quand la
//! transcription tient dans le budget laissé par la charte et le skill,
//! sinon notes neutres par partie puis synthèse finale. Une seule relance,
//! ciblée (concision si la sortie est coupée, structure sinon).

use tokio_util::sync::CancellationToken;

use super::prompt::{check_structure, clean_report, ReportContext, CONCISE_REMINDER};
use crate::summary::chunking::{estimate_tokens, split_into_chunks, MAX_CHUNK_TOKENS};
use crate::summary::prompt::ChatMessages;
use crate::summary::summarize::{Completion, CompletionOutput};
use crate::summary::SummaryError;

/// Plafond de sortie du compte rendu : 1 800 tokens, comme le résumé de
/// l'onglet Fichier (le contexte de 12 288 tokens l'absorbe).
pub const MAX_TOKENS_REPORT: u32 = 1_800;
/// Notes d'une partie : 1 800 et non 1 400 comme l'onglet Fichier, où le banc
/// d'essai a vu la première tranche coupée deux fois sur deux.
pub const MAX_TOKENS_NOTES: u32 = 1_800;
/// Tours de notes au plus (au-delà, la synthèse part quoi qu'il arrive).
pub const MAX_NOTE_ROUNDS: usize = 3;

async fn complete(
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

/// Compte rendu et son unique relance. Après la relance, la sortie est gardée
/// même imparfaite : un brouillon à relire vaut mieux qu'un échec, et le
/// journal garde la trace de l'écart.
async fn report_with_single_retry(
    ctx: &ReportContext,
    completion: &dyn Completion,
    cancel: &CancellationToken,
    messages: &ChatMessages,
) -> Result<String, SummaryError> {
    let first = complete(completion, cancel, messages, MAX_TOKENS_REPORT).await?;
    let text = clean_report(&first.text, &ctx.skill);
    let reminder = if first.truncated {
        log::warn!("Compte rendu de réunion coupé au plafond, relance avec rappel de concision");
        CONCISE_REMINDER.to_string()
    } else {
        match check_structure(&text, &ctx.skill) {
            Ok(()) => return Ok(text),
            Err(issue) => {
                log::warn!("Compte rendu de réunion hors structure ({issue:?}), relance");
                ctx.strict_reminder()
            }
        }
    };
    let retry = ChatMessages {
        system: messages.system.clone(),
        user: format!("{}\n\n{reminder}", messages.user),
    };
    let second = complete(completion, cancel, &retry, MAX_TOKENS_REPORT).await?;
    let second_text = clean_report(&second.text, &ctx.skill);
    if second_text.trim().is_empty() {
        return if text.trim().is_empty() {
            Err(SummaryError::IncompleteOutput)
        } else {
            Ok(text)
        };
    }
    if second.truncated {
        log::warn!("Compte rendu de réunion encore coupé après relance, gardé tel quel");
    } else if let Err(issue) = check_structure(&second_text, &ctx.skill) {
        log::warn!("Compte rendu de réunion encore hors structure ({issue:?}), gardé tel quel");
    }
    Ok(second_text)
}

/// Rédige le compte rendu de `transcript`. `on_progress(partie, total)` suit
/// les appels au modèle, la partie en cours ne redescendant jamais.
pub async fn generate_report(
    transcript: &str,
    ctx: &ReportContext,
    completion: &dyn Completion,
    cancel: &CancellationToken,
    on_progress: &mut (dyn FnMut(u32, u32) + Send),
) -> Result<String, SummaryError> {
    // Contrôlé par l'appelant avant de lancer le moteur ; par sécurité, le
    // minimum reste utilisable.
    let budget = ctx
        .content_budget()
        .unwrap_or(super::prompt::MIN_CONTENT_TOKENS);
    let transcript = transcript.trim();
    if transcript.is_empty() {
        return Err(SummaryError::IncompleteOutput);
    }

    let messages = if estimate_tokens(transcript) <= budget {
        on_progress(1, 1);
        ctx.direct_messages(transcript)
    } else {
        let mut chunks = split_into_chunks(transcript, MAX_CHUNK_TOKENS);
        let mut done = 0u32;
        let mut round = 0usize;
        loop {
            let round_total = chunks.len() as u32;
            // +1 : la synthèse finale.
            let total = done + round_total + 1;
            let mut notes = Vec::with_capacity(chunks.len());
            for (index, part) in chunks.iter().enumerate() {
                done += 1;
                on_progress(done, total);
                let output = complete(
                    completion,
                    cancel,
                    &ctx.notes_messages(part, index as u32 + 1, round_total),
                    MAX_TOKENS_NOTES,
                )
                .await?;
                if output.truncated {
                    log::warn!(
                        "Notes de la partie {} sur {round_total} coupées au plafond",
                        index + 1
                    );
                }
                notes.push(output.text);
            }
            let joined = notes.join("\n\n");
            if joined.trim().is_empty() {
                return Err(SummaryError::IncompleteOutput);
            }
            round += 1;
            if estimate_tokens(&joined) <= budget || round >= MAX_NOTE_ROUNDS {
                on_progress(done + 1, done + 1);
                break ctx.final_messages(&notes);
            }
            log::info!(
                "Notes de réunion trop longues ({} tokens estimés), nouvelle passe",
                estimate_tokens(&joined)
            );
            chunks = split_into_chunks(&joined, MAX_CHUNK_TOKENS);
        }
    };

    report_with_single_retry(ctx, completion, cancel, &messages).await
}

#[cfg(test)]
mod tests {
    use super::super::prompt::{french_date, parse_skill, DEFAULT_SKILL};
    use super::*;
    use chrono::NaiveDate;
    use futures_util::future::BoxFuture;
    use std::sync::Mutex;

    const GOOD: &str = "# Compte rendu – Site\n\n**Date :** x\n\n## Ordre du jour\n1. Site\n\n## Synthèse\nTexte.\n\n## Décisions\n- Aucune\n\n## Plan d'actions\n| Qui | Quoi | Échéance |\n|---|---|---|\n| A | B | C |\n\n## Points en suspens\n- Aucun\n\n## Prochaine réunion\nNon fixée";

    struct Scripted {
        replies: Mutex<Vec<CompletionOutput>>,
        calls: Mutex<Vec<(ChatMessages, u32)>>,
    }

    impl Scripted {
        fn new(replies: &[(&str, bool)]) -> Self {
            Self {
                replies: Mutex::new(
                    replies
                        .iter()
                        .rev()
                        .map(|(text, truncated)| CompletionOutput {
                            text: text.to_string(),
                            truncated: *truncated,
                        })
                        .collect(),
                ),
                calls: Mutex::new(Vec::new()),
            }
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
                    .push((messages.clone(), max_tokens));
                Ok(self
                    .replies
                    .lock()
                    .unwrap()
                    .pop()
                    .expect("réponse manquante"))
            })
        }
    }

    fn ctx(charter: Option<&str>) -> ReportContext {
        ReportContext {
            date_label: french_date(NaiveDate::from_ymd_opt(2026, 10, 22).unwrap()),
            charter: charter.map(str::to_string),
            skill: parse_skill(DEFAULT_SKILL),
        }
    }

    #[tokio::test]
    async fn a_short_meeting_is_written_in_one_pass() {
        let scripted = Scripted::new(&[(GOOD, false)]);
        let mut progress = Vec::new();
        let out = generate_report(
            "Bonjour à tous.",
            &ctx(Some("Vouvoyez.")),
            &scripted,
            &CancellationToken::new(),
            &mut |c, t| progress.push((c, t)),
        )
        .await
        .unwrap();
        assert_eq!(out, GOOD);
        assert_eq!(progress, vec![(1, 1)]);
        let calls = scripted.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert!(calls[0].0.system.contains("<charte>"));
        assert_eq!(calls[0].1, MAX_TOKENS_REPORT);
    }

    #[tokio::test]
    async fn a_long_meeting_goes_through_neutral_notes() {
        let transcript = "Le comité valide le budget de trente mille euros. ".repeat(1_500);
        let parts = split_into_chunks(&transcript, MAX_CHUNK_TOKENS).len();
        assert!(parts >= 2);
        let mut replies: Vec<(&str, bool)> = vec![("- note", false); parts];
        replies.push((GOOD, false));
        let scripted = Scripted::new(&replies);
        let mut progress = Vec::new();
        let out = generate_report(
            &transcript,
            &ctx(Some("Charte.")),
            &scripted,
            &CancellationToken::new(),
            &mut |c, t| progress.push((c, t)),
        )
        .await
        .unwrap();
        assert_eq!(out, GOOD);
        let calls = scripted.calls.lock().unwrap();
        assert_eq!(calls.len(), parts + 1);
        // Les notes sont neutres, la synthèse porte la charte et le skill.
        assert!(!calls[0].0.system.contains("<charte>"));
        assert_eq!(calls[0].1, MAX_TOKENS_NOTES);
        assert!(calls[parts].0.system.contains("<charte>"));
        assert!(calls[parts].0.user.contains("<skill>"));
        let total = parts as u32 + 1;
        assert_eq!(progress.last(), Some(&(total, total)));
        assert!(progress.windows(2).all(|w| w[0].0 <= w[1].0));
    }

    #[tokio::test]
    async fn a_report_out_of_structure_is_retried_once_then_kept() {
        let scripted =
            Scripted::new(&[("Texte sans structure.", false), ("Toujours libre.", false)]);
        let out = generate_report(
            "Bonjour.",
            &ctx(None),
            &scripted,
            &CancellationToken::new(),
            &mut |_, _| {},
        )
        .await
        .unwrap();
        assert_eq!(out, "Toujours libre.");
        let calls = scripted.calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert!(calls[1].0.user.ends_with(&ctx(None).strict_reminder()));
    }

    #[tokio::test]
    async fn a_truncated_report_gets_the_concise_reminder() {
        let scripted = Scripted::new(&[(GOOD, true), (GOOD, false)]);
        generate_report(
            "Bonjour.",
            &ctx(None),
            &scripted,
            &CancellationToken::new(),
            &mut |_, _| {},
        )
        .await
        .unwrap();
        let calls = scripted.calls.lock().unwrap();
        assert!(calls[1].0.user.ends_with(CONCISE_REMINDER));
    }

    #[tokio::test]
    async fn cancellation_wins_before_any_call() {
        let scripted = Scripted::new(&[(GOOD, false)]);
        let cancel = CancellationToken::new();
        cancel.cancel();
        let result =
            generate_report("Bonjour.", &ctx(None), &scripted, &cancel, &mut |_, _| {}).await;
        assert_eq!(result, Err(SummaryError::Cancelled));
        assert!(scripted.calls.lock().unwrap().is_empty());
    }
}
