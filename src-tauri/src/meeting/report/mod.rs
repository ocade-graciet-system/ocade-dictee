//! Compte rendu de réunion : transcription de l'enregistrement, rédaction par
//! le modèle local en appliquant la charte éditoriale et le skill, corrections
//! mécaniques par le code, puis export Word aux couleurs de la charte
//! graphique. Conception et limites : `docs/compte-rendu-reunion-faisabilite.md`.

pub mod docx;
pub mod generate;
pub mod postprocess;
pub mod prompt;
pub mod transcribe;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use chrono::TimeZone;
use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::AppHandle;
use tauri_specta::Event;
use tokio_util::sync::CancellationToken;

use super::store::MeetingStore;
use super::{assets, MeetingError, MeetingReport, RecordingState};
use crate::managers::transcription::TranscriptionManager;
use crate::summary::engine::{GenerationJob, SummaryEngine};
use crate::summary::summarize::Completion;
use crate::summary::{SummaryError, SummaryPhase};
use prompt::{french_date, parse_skill, ReportContext, DEFAULT_SKILL};
use transcribe::TranscribeError;

/// Un seul compte rendu à la fois (le moteur de transcription et le modèle de
/// rédaction sont partagés avec la dictée et l'onglet Fichier).
static REPORT_RUNNING: AtomicBool = AtomicBool::new(false);
static CANCEL_REPORT: AtomicBool = AtomicBool::new(false);
/// Le compte rendu est dans sa phase de rédaction : l'annuler passe par le
/// moteur. En dehors, annuler le moteur couperait un résumé de l'onglet Fichier.
static REPORT_IN_ENGINE: AtomicBool = AtomicBool::new(false);

struct RunningGuard;

impl Drop for RunningGuard {
    fn drop(&mut self) {
        REPORT_IN_ENGINE.store(false, Ordering::Release);
        REPORT_RUNNING.store(false, Ordering::Release);
    }
}

/// Étape en cours, pour l'encart de progression de l'onglet Réunion.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum ReportPhase {
    /// `current` / `total` : tronçons de 30 s transcrits.
    Transcribing,
    /// Téléchargement unique du moteur, en pourcentage.
    Engine,
    /// Téléchargement unique du modèle de rédaction, en pourcentage.
    Model,
    /// Chargement du modèle en mémoire.
    Starting,
    /// `current` / `total` : appels au modèle (notes puis synthèse).
    Writing,
    /// Corrections de la charte et enregistrement.
    Formatting,
}

/// Événement `meeting-report-progress`.
#[derive(Clone, Debug, Serialize, Deserialize, Type, tauri_specta::Event)]
#[serde(rename_all = "camelCase")]
pub struct MeetingReportProgress {
    pub recording_id: i64,
    pub phase: ReportPhase,
    pub current: u32,
    pub total: u32,
}

fn emit(app: &AppHandle, recording_id: i64, phase: ReportPhase, current: u32, total: u32) {
    let _ = MeetingReportProgress {
        recording_id,
        phase,
        current,
        total,
    }
    .emit(app);
}

/// Rédaction confiée au moteur local.
struct ReportJob {
    transcript: String,
    ctx: ReportContext,
}

impl GenerationJob for ReportJob {
    fn generate<'a>(
        &'a self,
        completion: &'a dyn Completion,
        cancel: &'a CancellationToken,
        on_part: &'a mut (dyn FnMut(u32, u32) + Send),
    ) -> BoxFuture<'a, Result<String, SummaryError>> {
        Box::pin(generate::generate_report(
            &self.transcript,
            &self.ctx,
            completion,
            cancel,
            on_part,
        ))
    }
}

/// Date de l'enregistrement en toutes lettres, à l'heure locale.
fn recording_date(created_at: i64) -> String {
    let date = chrono::Local
        .timestamp_opt(created_at, 0)
        .single()
        .map(|dt| dt.date_naive())
        .unwrap_or_else(|| chrono::Local::now().date_naive());
    french_date(date)
}

/// Charte, skill (ou gabarit par défaut) et date : ce que la rédaction applique.
fn report_context(store: &MeetingStore, created_at: i64) -> Result<ReportContext, MeetingError> {
    let saved = assets::load(store)?;
    let skill = saved
        .skill
        .as_ref()
        .map_or(DEFAULT_SKILL, |skill| skill.content.as_str());
    Ok(ReportContext {
        date_label: recording_date(created_at),
        charter: saved.editorial.map(|charter| charter.content),
        skill: parse_skill(skill),
    })
}

/// Transcrit l'enregistrement dans un fil bloquant (inférence lourde).
async fn transcribe_recording(
    app: &AppHandle,
    tm: Arc<TranscriptionManager>,
    id: i64,
    paths: Vec<PathBuf>,
) -> Result<String, MeetingError> {
    // Chargement du modèle de transcription s'il dort (sans effet sinon) ;
    // `transcribe()` attend la fin d'un chargement en cours.
    tm.initiate_model_load();
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let mut transcribe = |pcm: &[f32]| tm.transcribe(pcm.to_vec()).map_err(|e| e.to_string());
        transcribe::transcribe_parts(
            &paths,
            &mut transcribe,
            &|| CANCEL_REPORT.load(Ordering::Relaxed),
            &mut |done, total| emit(&app, id, ReportPhase::Transcribing, done, total),
        )
    })
    .await
    .map_err(MeetingError::storage)?
    .map_err(|e| match e {
        TranscribeError::Cancelled => MeetingError::Cancelled,
        TranscribeError::Unreadable(detail) | TranscribeError::Failed(detail) => {
            MeetingError::Transcription { detail }
        }
    })
}

/// Rédige (ou rédige à nouveau) le compte rendu de l'enregistrement `id`. La
/// transcription déjà faite est réutilisée, sauf `retranscribe`.
pub async fn generate(
    app: &AppHandle,
    store: &MeetingStore,
    engine: &SummaryEngine,
    tm: Arc<TranscriptionManager>,
    id: i64,
    retranscribe: bool,
) -> Result<MeetingReport, MeetingError> {
    crate::licence::ensure_allowed(app).map_err(|_| MeetingError::LicenceRequired)?;
    if REPORT_RUNNING
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err(MeetingError::ReportBusy);
    }
    let _guard = RunningGuard;
    CANCEL_REPORT.store(false, Ordering::Release);

    let recording = store.get(id)?;
    if recording.state == RecordingState::InProgress {
        return Err(MeetingError::InProgress);
    }
    let ctx = report_context(store, recording.created_at)?;
    // Vérifié avant de transcrire : inutile de passer dix minutes sur l'audio
    // pour buter ensuite sur une charte trop longue.
    ctx.content_budget()
        .map_err(|_| MeetingError::AssetsTooLong)?;

    let saved = store.report(id)?;
    let transcript = match saved.transcript {
        Some(transcript) if !retranscribe && !transcript.trim().is_empty() => transcript,
        _ => {
            let paths = store.part_paths(id, recording.parts);
            let transcript = transcribe_recording(app, tm, id, paths).await?;
            store.set_transcript(id, &transcript)?;
            transcript
        }
    };
    if CANCEL_REPORT.load(Ordering::Acquire) {
        return Err(MeetingError::Cancelled);
    }

    let lexicon = ctx
        .charter
        .as_deref()
        .map(postprocess::parse_lexicon)
        .unwrap_or_default();
    let bold_names = ctx
        .charter
        .as_deref()
        .is_some_and(postprocess::charter_wants_bold_names);
    let date_label = ctx.date_label.clone();
    let job = ReportJob { transcript, ctx };
    REPORT_IN_ENGINE.store(true, Ordering::Release);
    let dictation_app = app.clone();
    let progress_app = app.clone();
    let result = engine
        .run_job(
            &job,
            move || crate::summary::commands::dictation_in_progress(&dictation_app),
            move |phase, current, total| {
                let phase = match phase {
                    SummaryPhase::Engine => ReportPhase::Engine,
                    SummaryPhase::Model => ReportPhase::Model,
                    SummaryPhase::Starting => ReportPhase::Starting,
                    SummaryPhase::Summarizing => ReportPhase::Writing,
                };
                emit(&progress_app, id, phase, current, total);
            },
        )
        .await;
    REPORT_IN_ENGINE.store(false, Ordering::Release);
    let markdown = match result {
        Ok(markdown) => markdown,
        Err(SummaryError::Cancelled) => return Err(MeetingError::Cancelled),
        Err(error) => {
            log::error!("Compte rendu de réunion {id} impossible : {error}");
            return Err(MeetingError::Summary { error });
        }
    };

    emit(app, id, ReportPhase::Formatting, 0, 1);
    let markdown = postprocess::apply_lexicon(&markdown, &lexicon);
    let markdown = if bold_names {
        postprocess::bold_participants(&markdown)
    } else {
        markdown
    };
    let markdown = postprocess::fix_date_header(&markdown, &date_label);
    let markdown = postprocess::separate_header_lines(&markdown);
    store.set_report(id, &markdown)?;
    emit(app, id, ReportPhase::Formatting, 1, 1);
    log::info!(
        "Compte rendu de réunion {id} rédigé ({} règles de vocabulaire appliquées)",
        lexicon.len()
    );
    store.report(id)
}

/// Demande l'annulation du compte rendu en cours : la transcription s'arrête
/// au prochain tronçon, la rédaction tout de suite.
pub fn cancel(engine: &SummaryEngine) {
    if !REPORT_RUNNING.load(Ordering::Acquire) {
        return;
    }
    CANCEL_REPORT.store(true, Ordering::Release);
    if REPORT_IN_ENGINE.load(Ordering::Acquire) {
        engine.cancel();
    }
}

/// Un compte rendu est en cours (la mise à jour automatique attend).
pub fn is_running() -> bool {
    REPORT_RUNNING.load(Ordering::Acquire)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_recording_date_is_written_in_full() {
        // 22 octobre 2026 à midi UTC : même jour dans tous les fuseaux de France.
        let label = recording_date(1_792_670_400);
        assert!(label.ends_with("octobre 2026"), "{label}");
    }

    #[test]
    fn without_a_skill_the_default_template_is_used() {
        let dir = tempfile::tempdir().unwrap();
        let store = MeetingStore::new_at(dir.path()).unwrap();
        let ctx = report_context(&store, 1_792_670_400).unwrap();
        assert!(ctx.charter.is_none());
        assert_eq!(ctx.skill.sections.len(), 6);

        assets::set_text(&store, super::super::TextAssetKind::Editorial, "Vouvoyez.").unwrap();
        assets::set_text(
            &store,
            super::super::TextAssetKind::Skill,
            "---\nname: x\n---\n```\n# T\n## A\n## B\n```",
        )
        .unwrap();
        let ctx = report_context(&store, 1_792_670_400).unwrap();
        assert_eq!(ctx.charter.as_deref(), Some("Vouvoyez."));
        assert_eq!(ctx.skill.sections, vec!["## A", "## B"]);
    }

    #[test]
    fn progress_events_use_camel_case() {
        let json = serde_json::to_value(MeetingReportProgress {
            recording_id: 3,
            phase: ReportPhase::Writing,
            current: 1,
            total: 2,
        })
        .unwrap();
        assert_eq!(
            json,
            serde_json::json!({ "recordingId": 3, "phase": "writing", "current": 1, "total": 2 })
        );
    }
}
