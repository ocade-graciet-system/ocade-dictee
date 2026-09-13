use crate::managers::file_history::{FileHistoryEntry, FileHistoryItem, FileHistoryManager};
use std::sync::Arc;
use tauri::State;

/// Liste allégée (sans les textes complets), plus récent d'abord.
#[tauri::command]
#[specta::specta]
pub async fn file_history_list(
    file_history: State<'_, Arc<FileHistoryManager>>,
) -> Result<Vec<FileHistoryItem>, String> {
    file_history.list().map_err(|e| e.to_string())
}

/// Entrée complète (texte brut + compte-rendu) pour l'affichage d'un résultat.
#[tauri::command]
#[specta::specta]
pub async fn file_history_get(
    file_history: State<'_, Arc<FileHistoryManager>>,
    id: i64,
) -> Result<Option<FileHistoryEntry>, String> {
    file_history.get(id).map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn file_history_delete(
    file_history: State<'_, Arc<FileHistoryManager>>,
    id: i64,
) -> Result<(), String> {
    file_history.delete(id).map_err(|e| e.to_string())
}
