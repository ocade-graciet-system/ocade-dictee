//! Commandes du canal de distribution (GitHub / Microsoft Store). Identiques
//! dans les deux builds ; côté GitHub la licence est toujours active et
//! l'achat renvoie une erreur explicite.

use crate::store_channel::{self, DistributionChannel, PurchaseOutcome, StoreLicenseStatus};
use tauri::{AppHandle, Manager};

#[tauri::command]
#[specta::specta]
pub fn get_distribution_channel() -> DistributionChannel {
    store_channel::distribution_channel()
}

/// Lit la licence (bloquant côté Store : appels WinRT synchronisés) hors du
/// runtime async.
#[tauri::command]
#[specta::specta]
pub async fn get_store_license() -> Result<StoreLicenseStatus, String> {
    tokio::task::spawn_blocking(store_channel::license_status)
        .await
        .map_err(|e| format!("license task panicked: {e}"))
}

/// Ouvre la fenêtre d'achat de l'abonnement, rattachée à la fenêtre principale.
#[tauri::command]
#[specta::specta]
pub async fn purchase_store_subscription(app: AppHandle) -> Result<PurchaseOutcome, String> {
    let hwnd = main_window_handle(&app)?;
    tokio::task::spawn_blocking(move || store_channel::purchase_subscription(hwnd))
        .await
        .map_err(|e| format!("purchase task panicked: {e}"))?
}

#[cfg(target_os = "windows")]
fn main_window_handle(app: &AppHandle) -> Result<isize, String> {
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "Fenêtre principale introuvable.".to_string())?;
    let hwnd = window.hwnd().map_err(|e| e.to_string())?;
    Ok(hwnd.0 as isize)
}

#[cfg(not(target_os = "windows"))]
fn main_window_handle(_app: &AppHandle) -> Result<isize, String> {
    Ok(0)
}
