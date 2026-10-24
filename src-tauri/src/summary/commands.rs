//! Commandes Tauri du compte-rendu local et colle avec l'application
//! (progression, attente de fin de dictée, historique, veilleur d'inactivité).

use std::sync::Arc;

use chrono::{Local, NaiveDate, TimeZone};
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

/// Jour local d'un horodatage unix (secondes) ; `None` hors des dates
/// représentables.
fn local_date(unix_secs: i64) -> Option<NaiveDate> {
    Local
        .timestamp_opt(unix_secs, 0)
        .single()
        .map(|datetime| datetime.date_naive())
}

/// Date de l'enregistrement donnée au modèle : jour (heure locale) de création
/// de l'entrée d'historique, sinon aujourd'hui.
fn recording_date(created_at: Option<i64>) -> NaiveDate {
    created_at
        .and_then(local_date)
        .unwrap_or_else(|| Local::now().date_naive())
}

/// Produit le compte-rendu Markdown de `text`. Si `history_id` est fourni, le
/// compte-rendu est enregistré dans l'entrée d'historique correspondante (un
/// échec d'enregistrement n'invalide pas le résultat renvoyé), et la date de
/// création de l'entrée sert de date de l'enregistrement dans les prompts.
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
    let created_at = history_id.and_then(|id| {
        file_history.created_at(id).unwrap_or_else(|e| {
            log::warn!("Date de l'entrée d'historique {id} illisible, date du jour retenue: {e}");
            None
        })
    });
    let dictation_app = app.clone();
    let progress_app = app.clone();
    let result = engine
        .run(
            &text,
            recording_date(created_at),
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
        .await;
    // Le message d'interface et la fiche support renvoient au journal : la
    // cause doit s'y trouver, le front ne reçoit qu'une variante d'erreur.
    let markdown = match result {
        Ok(markdown) => markdown,
        Err(SummaryError::Cancelled) => {
            log::info!("Résumé annulé à la demande de l'utilisateur");
            return Err(SummaryError::Cancelled);
        }
        Err(e) => {
            log::error!("Résumé impossible : {e}");
            return Err(e);
        }
    };
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_recording_date_is_the_local_day_of_the_history_entry() {
        // Midi, heure locale : le même jour quel que soit le fuseau de la machine.
        let noon = NaiveDate::from_ymd_opt(2026, 10, 22)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap();
        let created_at = Local.from_local_datetime(&noon).unwrap().timestamp();
        assert_eq!(
            recording_date(Some(created_at)),
            NaiveDate::from_ymd_opt(2026, 10, 22).unwrap()
        );
    }

    #[test]
    fn without_a_usable_history_date_the_recording_date_is_today() {
        let today = Local::now().date_naive();
        // Tolère un passage à minuit entre les deux lectures de l'horloge.
        let is_today = |date: NaiveDate| date == today || date == Local::now().date_naive();
        assert!(is_today(recording_date(None)));
        assert!(
            is_today(recording_date(Some(i64::MAX))),
            "date hors limites"
        );
    }
}
