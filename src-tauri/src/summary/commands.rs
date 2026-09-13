//! Commandes Tauri du compte-rendu local et colle avec l'application
//! (progression, attente de fin de dictée, historique, veilleur d'inactivité).

use std::sync::Arc;

use tauri::{AppHandle, Manager, State};
use tauri_specta::Event;

use super::engine::{SummaryEngine, IDLE_TIMEOUT, WATCHDOG_PERIOD};
use super::{SummaryError, SummaryProgress, SummaryStatus};
use crate::managers::audio::AudioRecordingManager;
use crate::managers::file_history::FileHistoryManager;
use crate::TranscriptionCoordinator;

/// Une dictée est en cours : enregistrement (`is_recording`) ou seconde
/// moitié du pipeline, transcription et collage (`is_transcribing`).
fn dictation_in_progress(app: &AppHandle) -> bool {
    let recording = app
        .try_state::<Arc<AudioRecordingManager>>()
        .is_some_and(|manager| manager.is_recording());
    let transcribing = app
        .try_state::<TranscriptionCoordinator>()
        .is_some_and(|coordinator| coordinator.is_transcribing());
    recording || transcribing
}

/// Produit le compte-rendu Markdown de `text`. Si `history_id` est fourni, le
/// compte-rendu est enregistré dans l'entrée d'historique correspondante (un
/// échec d'enregistrement n'invalide pas le résultat renvoyé).
///
/// Un seul résumé à la fois : le moteur rend `SummaryError::Busy` si un autre
/// est déjà en cours. La progression part en événement `summary-progress`.
#[tauri::command]
#[specta::specta]
pub async fn summarize_document(
    app: AppHandle,
    engine: State<'_, Arc<SummaryEngine>>,
    file_history: State<'_, Arc<FileHistoryManager>>,
    text: String,
    history_id: Option<i64>,
) -> Result<String, SummaryError> {
    let dictation_app = app.clone();
    let progress_app = app.clone();
    let markdown = engine
        .run(
            &text,
            move || dictation_in_progress(&dictation_app),
            move |phase, current, total| {
                let _ = SummaryProgress {
                    phase,
                    current,
                    total,
                }
                .emit(&progress_app);
            },
        )
        .await?;
    if let Some(id) = history_id {
        if let Err(e) = file_history.update_summary(id, &markdown) {
            log::warn!("Enregistrement du compte-rendu dans l'historique impossible: {e}");
        }
    }
    Ok(markdown)
}

/// Demande l'annulation du résumé en cours (téléchargement, démarrage ou
/// génération) ; sans effet s'il n'y en a pas.
#[tauri::command]
#[specta::specta]
pub fn cancel_summary(engine: State<'_, Arc<SummaryEngine>>) {
    engine.cancel();
}

/// Moteur et modèle présents ? Taille restant à télécharger (pour annoncer le
/// téléchargement unique d'environ 2 Go avant le premier usage).
#[tauri::command]
#[specta::specta]
pub fn summary_status(engine: State<'_, Arc<SummaryEngine>>) -> SummaryStatus {
    engine.status()
}

/// Toutes les 30 s : arrête le serveur inactif depuis 10 min (le modèle
/// libère ≈ 2,7 Go de mémoire ; il est rechargé au prochain résumé).
///
/// Variante `_async` : l'attente de la fin du processus (déchargement du
/// modèle) part sur un fil bloquant au lieu d'immobiliser un worker tokio.
/// `stop_if_idle_async` ne prend jamais un serveur en cours d'usage.
pub fn spawn_idle_watchdog(engine: Arc<SummaryEngine>) {
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(WATCHDOG_PERIOD).await;
            engine.stop_if_idle_async(IDLE_TIMEOUT).await;
        }
    });
}
