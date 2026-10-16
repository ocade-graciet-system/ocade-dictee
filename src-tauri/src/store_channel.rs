//! Canal de distribution de l'application et, pour le build « Microsoft
//! Store » (feature `store`), vérification de l'abonnement et démarrage
//! automatique via les API Windows.
//!
//! Deux canaux coexistent et ne se mélangent jamais :
//!
//! - **GitHub** (feature absente) : installeur NSIS publié sur les releases,
//!   mises à jour par l'updater in-app, aucune vérification de licence. C'est
//!   le build par défaut, celui des postes de l'éditeur et des cas
//!   particuliers.
//! - **Store** (feature `store`) : package MSIX soumis au Microsoft Store.
//!   L'abonnement annuel est un *add-on* du Store ; au démarrage l'application
//!   interroge la licence (`Windows.Services.Store`) et bloque l'usage tant
//!   qu'aucun abonnement actif n'est trouvé. Les mises à jour sont faites par le
//!   Store et le démarrage automatique passe par une `StartupTask` déclarée
//!   dans le manifeste (la clé `Run` du registre, virtualisée pour une app
//!   packagée, n'aurait aucun effet).
//!
//! Les fonctions publiques existent dans les deux builds, avec un comportement
//! neutre côté GitHub, pour que les commandes Tauri et les bindings du front
//! soient identiques.

use serde::{Deserialize, Serialize};
use specta::Type;

/// Canal de distribution compilé dans le binaire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "lowercase")]
pub enum DistributionChannel {
    /// Installeur GitHub : aucune licence, updater in-app.
    GitHub,
    /// Package MSIX du Microsoft Store : abonnement vérifié, mises à jour par le Store.
    Store,
}

pub fn distribution_channel() -> DistributionChannel {
    if cfg!(feature = "store") {
        DistributionChannel::Store
    } else {
        DistributionChannel::GitHub
    }
}

/// Identifiant Store (« Store ID ») de l'add-on « abonnement annuel », tel
/// qu'affiché dans Partner Center. Injecté à la compilation par la variable
/// d'environnement `OCADE_STORE_SUBSCRIPTION_ID` (CI) ; la valeur par défaut
/// n'est là que pour que le build compile avant la création du produit.
#[cfg_attr(not(all(target_os = "windows", feature = "store")), allow(dead_code))]
pub const SUBSCRIPTION_STORE_ID: &str = match option_env!("OCADE_STORE_SUBSCRIPTION_ID") {
    Some(id) => id,
    None => "9XXXXXXXXXXX",
};

/// Identifiant de la tâche de démarrage déclarée dans `msix/AppxManifest.xml`
/// (`<desktop:StartupTask TaskId=…>`). Doit rester identique des deux côtés.
#[cfg_attr(not(all(target_os = "windows", feature = "store")), allow(dead_code))]
pub const STARTUP_TASK_ID: &str = "OcadeDicteeStartup";

/// État de la licence vu par l'application.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct StoreLicenseStatus {
    /// Canal du binaire, pour que le front n'ait qu'un appel à faire.
    pub channel: DistributionChannel,
    /// `true` si l'application peut être utilisée : toujours vrai côté GitHub ;
    /// côté Store, un abonnement actif (ou une licence d'essai active).
    pub active: bool,
    /// Licence d'essai du Store (si l'éditeur en propose une).
    pub is_trial: bool,
    /// Fin de l'abonnement en secondes Unix, quand le Store la communique.
    pub expires_at: Option<i64>,
    /// Cause technique quand la licence n'a pas pu être lue (hors Store,
    /// package non associé, API indisponible…). En anglais, pour le détail.
    pub detail: Option<String>,
}

/// Résultat d'une demande d'achat de l'abonnement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum PurchaseOutcome {
    /// Achat effectué : la licence est désormais active.
    Succeeded,
    /// L'utilisateur possédait déjà l'abonnement.
    AlreadyPurchased,
    /// L'utilisateur a fermé la fenêtre d'achat sans acheter.
    NotPurchased,
    /// Le Store n'a pas pu finaliser (réseau, serveur, moyen de paiement).
    Failed,
}

// ---------------------------------------------------------------------------
// Build GitHub : rien à vérifier, rien à acheter.
// ---------------------------------------------------------------------------

#[cfg(not(all(target_os = "windows", feature = "store")))]
pub fn license_status() -> StoreLicenseStatus {
    StoreLicenseStatus {
        channel: distribution_channel(),
        active: true,
        is_trial: false,
        expires_at: None,
        detail: None,
    }
}

#[cfg(not(all(target_os = "windows", feature = "store")))]
pub fn purchase_subscription(_hwnd: isize) -> Result<PurchaseOutcome, String> {
    Err("Les achats ne sont disponibles que dans la version Microsoft Store.".to_string())
}

/// Démarrage automatique côté GitHub : clé `Run` du registre via
/// tauri-plugin-autostart (comportement historique).
#[cfg(not(all(target_os = "windows", feature = "store")))]
pub fn set_autostart(app: &tauri::AppHandle, enabled: bool) {
    use tauri_plugin_autostart::ManagerExt;
    let manager = app.autolaunch();
    let result = if enabled {
        manager.enable()
    } else {
        manager.disable()
    };
    if let Err(e) = result {
        log::warn!("Démarrage automatique ({enabled}) non appliqué : {e}");
    }
}

// ---------------------------------------------------------------------------
// Build Store : Windows.Services.Store + StartupTask.
// ---------------------------------------------------------------------------

#[cfg(all(target_os = "windows", feature = "store"))]
mod store_impl {
    use super::{PurchaseOutcome, StoreLicenseStatus, SUBSCRIPTION_STORE_ID};
    use log::{info, warn};
    use windows::core::{Interface, HSTRING};
    use windows::ApplicationModel::{StartupTask, StartupTaskState};
    use windows::Foundation::DateTime;
    use windows::Services::Store::{StoreContext, StorePurchaseStatus};
    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
    use windows::Win32::UI::Shell::IInitializeWithWindow;

    /// Les API WinRT exigent un apartment COM initialisé sur le thread
    /// appelant. Les commandes tournent sur des threads de travail Tauri :
    /// on initialise à chaque appel, le double appel est sans effet.
    fn init_com() {
        // SAFETY: appel documenté comme réentrant ; un HRESULT d'échec
        // (apartment déjà d'un autre mode) n'empêche pas l'usage de WinRT.
        let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    }

    /// `DateTime` WinRT (intervalles de 100 ns depuis le 01/01/1601) → Unix.
    fn to_unix_seconds(date: DateTime) -> Option<i64> {
        const EPOCH_DIFF_SECS: i64 = 11_644_473_600;
        if date.UniversalTime <= 0 {
            return None;
        }
        Some(date.UniversalTime / 10_000_000 - EPOCH_DIFF_SECS)
    }

    fn unavailable(detail: String) -> StoreLicenseStatus {
        StoreLicenseStatus {
            channel: super::DistributionChannel::Store,
            active: false,
            is_trial: false,
            expires_at: None,
            detail: Some(detail),
        }
    }

    /// Lit la licence de l'application puis celle de l'add-on abonnement.
    /// Le Store met la licence en cache sur le poste : l'appel répond aussi
    /// hors ligne, avec la dernière licence connue.
    pub fn license_status() -> StoreLicenseStatus {
        init_com();
        let context = match StoreContext::GetDefault() {
            Ok(c) => c,
            Err(e) => return unavailable(format!("StoreContext unavailable: {e}")),
        };
        let license = match context.GetAppLicenseAsync().and_then(|op| op.get()) {
            Ok(l) => l,
            Err(e) => return unavailable(format!("GetAppLicenseAsync failed: {e}")),
        };

        let app_active = license.IsActive().unwrap_or(false);
        let is_trial = license.IsTrial().unwrap_or(false);

        // L'abonnement est un add-on : sa licence est dans `AddOnLicenses`,
        // indexée par « <Store ID de l'add-on>/<SKU> ». On compare donc sur le
        // préfixe, le SKU n'étant pas connu à l'avance.
        let mut subscription_active = false;
        let mut expires_at = None;
        match license.AddOnLicenses().and_then(|map| map.First()) {
            Ok(iter) => {
                while iter.HasCurrent().unwrap_or(false) {
                    if let Ok(pair) = iter.Current() {
                        let key = pair.Key().map(|k| k.to_string()).unwrap_or_default();
                        if key.starts_with(SUBSCRIPTION_STORE_ID) {
                            if let Ok(add_on) = pair.Value() {
                                if add_on.IsActive().unwrap_or(false) {
                                    subscription_active = true;
                                    expires_at =
                                        add_on.ExpirationDate().ok().and_then(to_unix_seconds);
                                }
                            }
                        }
                    }
                    if iter.MoveNext().is_err() {
                        break;
                    }
                }
            }
            Err(e) => warn!("Licences d'add-on illisibles : {e}"),
        }

        info!(
            "Licence Store : app active={app_active}, essai={is_trial}, abonnement actif={subscription_active}"
        );
        StoreLicenseStatus {
            channel: super::DistributionChannel::Store,
            // Une licence d'essai active vaut autorisation (durée gérée par le Store).
            active: subscription_active || (app_active && is_trial),
            is_trial,
            expires_at,
            detail: None,
        }
    }

    /// Ouvre la fenêtre d'achat du Store pour l'abonnement, rattachée à la
    /// fenêtre principale (`hwnd`) : sans ce rattachement, une application de
    /// bureau n'a pas de surface où le Store peut afficher sa boîte de dialogue.
    pub fn purchase_subscription(hwnd: isize) -> Result<PurchaseOutcome, String> {
        init_com();
        let context =
            StoreContext::GetDefault().map_err(|e| format!("StoreContext unavailable: {e}"))?;
        let init: IInitializeWithWindow = context
            .cast()
            .map_err(|e| format!("IInitializeWithWindow unavailable: {e}"))?;
        // SAFETY: `hwnd` provient de la fenêtre principale Tauri, vivante tant
        // que l'application tourne.
        unsafe { init.Initialize(HWND(hwnd as *mut core::ffi::c_void)) }
            .map_err(|e| format!("IInitializeWithWindow::Initialize failed: {e}"))?;

        let result = context
            .RequestPurchaseAsync(&HSTRING::from(SUBSCRIPTION_STORE_ID))
            .and_then(|op| op.get())
            .map_err(|e| format!("RequestPurchaseAsync failed: {e}"))?;
        let status = result.Status().map_err(|e| e.to_string())?;
        let outcome = match status {
            StorePurchaseStatus::Succeeded => PurchaseOutcome::Succeeded,
            StorePurchaseStatus::AlreadyPurchased => PurchaseOutcome::AlreadyPurchased,
            StorePurchaseStatus::NotPurchased => PurchaseOutcome::NotPurchased,
            _ => {
                let extended = result
                    .ExtendedError()
                    .map(|e| format!("{e:?}"))
                    .unwrap_or_default();
                warn!("Achat de l'abonnement non abouti : {status:?} {extended}");
                PurchaseOutcome::Failed
            }
        };
        info!("Achat de l'abonnement : {outcome:?}");
        Ok(outcome)
    }

    /// Démarrage automatique via la `StartupTask` du manifeste. Windows garde
    /// la main : si l'utilisateur l'a désactivée dans Paramètres → Applications
    /// de démarrage, `RequestEnableAsync` renvoie `DisabledByUser` et on n'y
    /// peut rien depuis l'application.
    pub fn set_autostart(enabled: bool) {
        init_com();
        let task = match StartupTask::GetAsync(&HSTRING::from(super::STARTUP_TASK_ID))
            .and_then(|op| op.get())
        {
            Ok(t) => t,
            Err(e) => {
                warn!("StartupTask introuvable (manifeste MSIX ?) : {e}");
                return;
            }
        };
        if enabled {
            match task.RequestEnableAsync().and_then(|op| op.get()) {
                Ok(StartupTaskState::Enabled) => {
                    info!("Démarrage automatique activé (StartupTask)")
                }
                Ok(state) => warn!("Démarrage automatique non activé : état {state:?}"),
                Err(e) => warn!("Démarrage automatique non activé : {e}"),
            }
        } else if let Err(e) = task.Disable() {
            warn!("Démarrage automatique non désactivé : {e}");
        }
    }
}

#[cfg(all(target_os = "windows", feature = "store"))]
pub fn license_status() -> StoreLicenseStatus {
    store_impl::license_status()
}

#[cfg(all(target_os = "windows", feature = "store"))]
pub fn purchase_subscription(hwnd: isize) -> Result<PurchaseOutcome, String> {
    store_impl::purchase_subscription(hwnd)
}

#[cfg(all(target_os = "windows", feature = "store"))]
pub fn set_autostart(_app: &tauri::AppHandle, enabled: bool) {
    store_impl::set_autostart(enabled)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_build_is_always_licensed() {
        if distribution_channel() == DistributionChannel::GitHub {
            let status = license_status();
            assert!(status.active);
            assert_eq!(status.channel, DistributionChannel::GitHub);
            assert!(status.detail.is_none());
        }
    }

    #[test]
    fn subscription_id_is_set_or_placeholder() {
        assert!(!SUBSCRIPTION_STORE_ID.is_empty());
    }
}
