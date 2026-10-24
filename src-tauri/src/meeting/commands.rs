//! Commandes Tauri de l'onglet Réunion.

use serde::Deserialize;
use specta::Type;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::{AppHandle, State};

use super::recorder::MeetingRecorder;
use super::report::{self, docx};
use super::{
    assets, GraphicCharter, MeetingAssets, MeetingError, MeetingRecorderStatus, MeetingRecording,
    MeetingReport, RecordingState, TextAssetKind,
};
use crate::managers::transcription::TranscriptionManager;
use crate::summary::engine::SummaryEngine;

type Recorder<'a> = State<'a, Arc<MeetingRecorder>>;

#[tauri::command]
#[specta::specta]
pub fn meeting_recorder_status(recorder: Recorder<'_>) -> MeetingRecorderStatus {
    recorder.status()
}

#[tauri::command]
#[specta::specta]
pub async fn meeting_start(recorder: Recorder<'_>) -> Result<MeetingRecorderStatus, MeetingError> {
    recorder.start()
}

#[tauri::command]
#[specta::specta]
pub async fn meeting_pause(recorder: Recorder<'_>) -> Result<MeetingRecorderStatus, MeetingError> {
    recorder.pause()
}

#[tauri::command]
#[specta::specta]
pub async fn meeting_resume(recorder: Recorder<'_>) -> Result<MeetingRecorderStatus, MeetingError> {
    recorder.resume()
}

#[tauri::command]
#[specta::specta]
pub async fn meeting_stop(recorder: Recorder<'_>) -> Result<MeetingRecording, MeetingError> {
    recorder.stop()
}

#[tauri::command]
#[specta::specta]
pub async fn meeting_list(recorder: Recorder<'_>) -> Result<Vec<MeetingRecording>, MeetingError> {
    recorder.store().list()
}

#[tauri::command]
#[specta::specta]
pub async fn meeting_rename(
    recorder: Recorder<'_>,
    id: i64,
    name: String,
) -> Result<(), MeetingError> {
    recorder.store().rename(id, &name)
}

fn ensure_not_in_progress(recording: &MeetingRecording) -> Result<(), MeetingError> {
    if recording.state == RecordingState::InProgress {
        return Err(MeetingError::InProgress);
    }
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn meeting_delete(recorder: Recorder<'_>, id: i64) -> Result<(), MeetingError> {
    ensure_not_in_progress(&recorder.store().get(id)?)?;
    recorder.store().delete(id)
}

/// Fichier WAV à écouter (le premier, pour un enregistrement de plus de 37 h).
#[tauri::command]
#[specta::specta]
pub async fn meeting_audio_path(recorder: Recorder<'_>, id: i64) -> Result<String, MeetingError> {
    let recording = recorder.store().get(id)?;
    ensure_not_in_progress(&recording)?;
    let path = recorder.store().part_paths(id, recording.parts).remove(0);
    if !path.exists() {
        return Err(MeetingError::NotFound);
    }
    Ok(path.to_string_lossy().into_owned())
}

/// Destinations d'export : `dest` pour le premier fichier, puis
/// `<nom>-2.wav`, `<nom>-3.wav`… pour les suivants.
fn export_targets(dest: &Path, parts: usize) -> Vec<PathBuf> {
    let stem = dest
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "reunion".into());
    let ext = dest
        .extension()
        .map(|e| e.to_string_lossy().into_owned())
        .unwrap_or_else(|| "wav".into());
    (1..=parts)
        .map(|part| {
            if part == 1 {
                dest.to_path_buf()
            } else {
                dest.with_file_name(format!("{stem}-{part}.{ext}"))
            }
        })
        .collect()
}

/// Copie l'enregistrement là où l'utilisateur l'a choisi.
#[tauri::command]
#[specta::specta]
pub async fn meeting_export(
    recorder: Recorder<'_>,
    id: i64,
    dest_path: String,
) -> Result<(), MeetingError> {
    let recording = recorder.store().get(id)?;
    ensure_not_in_progress(&recording)?;
    let sources = recorder.store().part_paths(id, recording.parts);
    let targets = export_targets(Path::new(&dest_path), sources.len());
    tauri::async_runtime::spawn_blocking(move || {
        for (source, target) in sources.iter().zip(&targets) {
            std::fs::copy(source, target).map_err(MeetingError::storage)?;
        }
        Ok(())
    })
    .await
    .map_err(MeetingError::storage)?
}

#[tauri::command]
#[specta::specta]
pub async fn meeting_assets(recorder: Recorder<'_>) -> Result<MeetingAssets, MeetingError> {
    assets::load(recorder.store())
}

#[tauri::command]
#[specta::specta]
pub async fn meeting_import_text_asset(
    recorder: Recorder<'_>,
    kind: TextAssetKind,
    path: String,
) -> Result<MeetingAssets, MeetingError> {
    assets::import_text(recorder.store(), kind, Path::new(&path))?;
    assets::load(recorder.store())
}

#[tauri::command]
#[specta::specta]
pub async fn meeting_set_text_asset(
    recorder: Recorder<'_>,
    kind: TextAssetKind,
    text: String,
) -> Result<MeetingAssets, MeetingError> {
    assets::set_text(recorder.store(), kind, &text)?;
    assets::load(recorder.store())
}

#[tauri::command]
#[specta::specta]
pub async fn meeting_clear_text_asset(
    recorder: Recorder<'_>,
    kind: TextAssetKind,
) -> Result<MeetingAssets, MeetingError> {
    recorder.store().clear_asset(kind.as_db())?;
    assets::load(recorder.store())
}

#[tauri::command]
#[specta::specta]
pub async fn meeting_save_graphic_charter(
    recorder: Recorder<'_>,
    charter: GraphicCharter,
) -> Result<MeetingAssets, MeetingError> {
    assets::save_graphic(recorder.store(), &charter)?;
    assets::load(recorder.store())
}

#[tauri::command]
#[specta::specta]
pub async fn meeting_import_logo(
    recorder: Recorder<'_>,
    path: String,
) -> Result<MeetingAssets, MeetingError> {
    assets::import_logo(recorder.store(), Path::new(&path))?;
    assets::load(recorder.store())
}

#[tauri::command]
#[specta::specta]
pub async fn meeting_clear_logo(recorder: Recorder<'_>) -> Result<MeetingAssets, MeetingError> {
    assets::clear_logo(recorder.store())?;
    assets::load(recorder.store())
}

#[tauri::command]
#[specta::specta]
pub async fn meeting_clear_graphic_charter(
    recorder: Recorder<'_>,
) -> Result<MeetingAssets, MeetingError> {
    assets::clear_graphic(recorder.store())?;
    assets::load(recorder.store())
}

/// Rédige le compte rendu de l'enregistrement (progression : événement
/// `meeting-report-progress`). `retranscribe` refait la transcription au lieu
/// de réutiliser celle déjà conservée.
#[tauri::command]
#[specta::specta]
pub async fn meeting_generate_report(
    app: AppHandle,
    recorder: Recorder<'_>,
    engine: State<'_, Arc<SummaryEngine>>,
    transcription: State<'_, Arc<TranscriptionManager>>,
    id: i64,
    retranscribe: bool,
) -> Result<MeetingReport, MeetingError> {
    report::generate(
        &app,
        recorder.store(),
        &engine,
        Arc::clone(&transcription),
        id,
        retranscribe,
    )
    .await
}

#[tauri::command]
#[specta::specta]
pub fn meeting_cancel_report(engine: State<'_, Arc<SummaryEngine>>) {
    report::cancel(&engine);
}

/// Un compte rendu est en cours de rédaction.
#[tauri::command]
#[specta::specta]
pub fn meeting_report_running() -> bool {
    report::is_running()
}

#[tauri::command]
#[specta::specta]
pub async fn meeting_get_report(
    recorder: Recorder<'_>,
    id: i64,
) -> Result<MeetingReport, MeetingError> {
    recorder.store().report(id)
}

/// Logo converti en PNG par l'interface (qui sait lire SVG, WebP, GIF…).
#[derive(Debug, Clone, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ReportLogo {
    pub png_base64: String,
    pub width: u32,
    pub height: u32,
}

/// Enregistre le compte rendu en document Word, mis en forme avec la charte
/// graphique (couleurs, police, pied de page, logo).
#[tauri::command]
#[specta::specta]
pub async fn meeting_export_report_docx(
    recorder: Recorder<'_>,
    id: i64,
    dest_path: String,
    logo: Option<ReportLogo>,
) -> Result<(), MeetingError> {
    let store = recorder.store();
    let recording = store.get(id)?;
    let markdown = store.report(id)?.markdown.ok_or(MeetingError::NotFound)?;
    let charter = assets::load(store)?
        .graphic
        .unwrap_or_else(docx::default_charter);
    let logo = match logo {
        Some(logo) => {
            use base64::Engine;
            let png = base64::engine::general_purpose::STANDARD
                .decode(logo.png_base64.as_bytes())
                .map_err(|e| MeetingError::InvalidFile {
                    detail: e.to_string(),
                })?;
            Some(docx::DocxLogo {
                png,
                width: logo.width,
                height: logo.height,
            })
        }
        None => None,
    };
    let bytes = docx::build_docx(&docx::DocxInput {
        markdown: &markdown,
        title: &recording.name,
        charter: &charter,
        logo,
    })
    .map_err(MeetingError::storage)?;
    std::fs::write(&dest_path, bytes).map_err(MeetingError::storage)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_targets_number_the_extra_parts() {
        let dest = Path::new("/out/Comité.wav");
        assert_eq!(export_targets(dest, 1), vec![dest.to_path_buf()]);
        assert_eq!(
            export_targets(dest, 3),
            vec![
                dest.to_path_buf(),
                PathBuf::from("/out/Comité-2.wav"),
                PathBuf::from("/out/Comité-3.wav"),
            ]
        );
    }
}
