//! Commandes Tauri de l'onglet Licence.

use super::{LicenceError, LicenceManager, LicenceView};
use std::sync::Arc;
use tauri::State;

/// État courant de la licence.
#[tauri::command]
#[specta::specta]
pub fn licence_get_state(manager: State<'_, Arc<LicenceManager>>) -> LicenceView {
    manager.view()
}

/// Active une clé sur ce poste.
#[tauri::command]
#[specta::specta]
pub async fn licence_activate(
    manager: State<'_, Arc<LicenceManager>>,
    key: String,
) -> Result<LicenceView, LicenceError> {
    manager.activate(&key).await
}

/// « Vérifier maintenant » : renouvelle tout de suite le jeton.
#[tauri::command]
#[specta::specta]
pub async fn licence_refresh(
    manager: State<'_, Arc<LicenceManager>>,
) -> Result<LicenceView, LicenceError> {
    manager.fetch_mode().await;
    manager.refresh().await
}

/// Retire la clé de ce poste et libère son emplacement.
#[tauri::command]
#[specta::specta]
pub async fn licence_remove(
    manager: State<'_, Arc<LicenceManager>>,
) -> Result<LicenceView, LicenceError> {
    manager.remove().await
}

/// Remplace la clé de ce poste par une autre.
#[tauri::command]
#[specta::specta]
pub async fn licence_change(
    manager: State<'_, Arc<LicenceManager>>,
    key: String,
) -> Result<LicenceView, LicenceError> {
    manager.change(&key).await
}
