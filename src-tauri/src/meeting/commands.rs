//! Commandes Tauri de l'onglet Réunion.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::State;

use super::recorder::MeetingRecorder;
use super::{
    assets, GraphicCharter, MeetingAssets, MeetingError, MeetingRecorderStatus, MeetingRecording,
    RecordingState, TextAssetKind,
};

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
