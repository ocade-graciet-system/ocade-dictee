//! Licence d'OCADE Dictée.
//!
//! Le serveur de licences délivre, pour une clé activée sur un poste, un jeton
//! signé valable 14 jours. L'application le vérifie **hors ligne** et le
//! renouvelle au démarrage puis toutes les 24 h. Le mode d'application, piloté
//! par le serveur, décide de l'effet d'une licence absente ou invalide :
//! `soft` (on affiche, rien n'est bloqué) ou `strict` (la dictée est bloquée).
//!
//! Spécification complète : docs/licences-passation.md (sections 5 et 6).

mod client;
mod config;
mod fingerprint;
mod key;
mod state;
mod store;
mod token;

pub mod commands;

pub use state::LicenceView;

use client::{ApiError, ClientMeta, LicenceClient, OsInfo, TokenResponse};
use config::LicenceConfig;
use serde::{Deserialize, Serialize};
use specta::Type;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use store::Persisted;
use tauri::{AppHandle, Emitter, Manager};
use token::TrustedKey;

/// Événement émis à chaque changement de l'état de licence.
pub const STATE_EVENT: &str = "licence-state-changed";
/// Délai avant la première vérification, pour ne pas ralentir le démarrage.
const STARTUP_DELAY: Duration = Duration::from_secs(5);
/// Vérification normale.
const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
/// Nouvel essai après un échec réseau.
const RETRY_INTERVAL: Duration = Duration::from_secs(30 * 60);

/// Erreur renvoyée à l'interface : un code stable, traduit côté front.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct LicenceError {
    /// `invalid_key_format`, `unknown_key`, `revoked`, `archived`, `expired`,
    /// `not_activated`, `seats_full`, `release_quota_exceeded`, `rate_limited`,
    /// `internal_error`, `network`, `invalid_response`, `no_fingerprint`,
    /// `no_key`, `licence_required`.
    pub code: String,
    pub seats_used: Option<u32>,
    pub seats_max: Option<u32>,
    pub expired_at: Option<i64>,
    /// Détail technique (en anglais), pour le support.
    pub detail: Option<String>,
}

impl LicenceError {
    fn new(code: &str) -> Self {
        Self {
            code: code.to_string(),
            seats_used: None,
            seats_max: None,
            expired_at: None,
            detail: None,
        }
    }

    fn from_api(err: &ApiError) -> Self {
        let mut out = Self::new(err.code());
        match err {
            ApiError::Refused {
                expired_at, seats, ..
            } => {
                out.expired_at = *expired_at;
                out.seats_used = seats.map(|(used, _)| used);
                out.seats_max = seats.map(|(_, max)| max);
            }
            ApiError::Network(d) => out.detail = Some(d.clone()),
        }
        out
    }
}

pub struct LicenceManager {
    app: AppHandle,
    path: PathBuf,
    state: Mutex<Persisted>,
    client: LicenceClient,
    keys: Vec<TrustedKey>,
    bypass: bool,
    /// Sérialise les échanges réseau (vérification de fond et actions de
    /// l'utilisateur) pour qu'ils ne se marchent pas dessus.
    network: tokio::sync::Mutex<()>,
}

impl LicenceManager {
    pub fn new(app: &AppHandle) -> Result<Self, String> {
        let dir = crate::portable::app_data_dir(app).map_err(|e| e.to_string())?;
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let path = dir.join("licence.json");
        let mut persisted = store::load(&path);
        persisted.observe_time(store::system_now());
        let config = LicenceConfig::resolve();
        let manager = Self {
            app: app.clone(),
            path,
            state: Mutex::new(persisted),
            client: LicenceClient::new(config.base_url),
            keys: config.keys,
            bypass: config.bypass,
            network: tokio::sync::Mutex::new(()),
        };
        manager.persist();
        if manager.bypass {
            log::info!("Licence ignorée (build de développement)");
        }
        Ok(manager)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Persisted> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn persist(&self) {
        let snapshot = self.lock().clone();
        if let Err(e) = store::save(&self.path, &snapshot) {
            log::warn!("Écriture de licence.json impossible : {e}");
        }
    }

    /// État courant, recalculé à chaque appel (le temps passe).
    pub fn view(&self) -> LicenceView {
        let persisted = self.lock().clone();
        let decoded = persisted
            .token
            .as_deref()
            .map(|t| token::decode_verified(t, &self.keys));
        let fingerprint = fingerprint::current();
        state::compute(state::Inputs {
            persisted: &persisted,
            decoded,
            fingerprint: fingerprint.as_deref(),
            now: persisted.effective_now(store::system_now()),
            bypass: self.bypass,
            hostname: hostname(),
        })
    }

    /// La dictée est-elle autorisée maintenant ?
    pub fn is_allowed(&self) -> bool {
        !self.view().enforced
    }

    fn emit(&self) {
        let _ = self.app.emit(STATE_EVENT, self.view());
    }

    /// Corps des requêtes, conforme au schéma strict du serveur : empreinte en
    /// hexadécimal minuscule, nom du poste non vide (255 au plus), champs du
    /// système à 64 ou 32 unités au plus, jamais `null`.
    fn meta(&self, key: &str) -> Result<ClientMeta, LicenceError> {
        let hostname = hostname()
            .map(|h| h.trim().to_string())
            .filter(|h| !h.is_empty())
            .unwrap_or_else(|| "poste".to_string());
        Ok(ClientMeta {
            key: key.to_string(),
            fingerprint: fingerprint::current()
                .ok_or_else(|| LicenceError::new("no_fingerprint"))?,
            hostname: client::clip(&hostname, client::HOSTNAME_MAX),
            os: OsInfo {
                name: client::clip(
                    &sysinfo::System::name().unwrap_or_default(),
                    client::OS_TEXT_MAX,
                ),
                version: client::clip(
                    &sysinfo::System::os_version().unwrap_or_default(),
                    client::OS_TEXT_MAX,
                ),
                arch: client::clip(std::env::consts::ARCH, client::SHORT_TEXT_MAX),
            },
            app_version: client::clip(env!("CARGO_PKG_VERSION"), client::SHORT_TEXT_MAX),
        })
    }

    /// Vérifie un jeton reçu du serveur avant de le conserver : on ne garde
    /// jamais un jeton que l'application rejetterait ensuite.
    fn accept_token(&self, response: &TokenResponse, key: &str) -> Result<(), LicenceError> {
        let claims = token::decode_verified(&response.token, &self.keys).map_err(|e| {
            let mut err = LicenceError::new("invalid_response");
            err.detail = Some(format!("token rejected: {e:?}"));
            err
        })?;
        let fp = fingerprint::current().unwrap_or_default();
        let mut guard = self.lock();
        guard.observe_time(response.server_time);
        let now = guard.effective_now(store::system_now());
        token::check_validity(&claims, &fp, now).map_err(|e| {
            let mut err = LicenceError::new("invalid_response");
            err.detail = Some(format!("token not valid here: {e:?}"));
            err
        })?;
        guard.key = Some(key.to_string());
        guard.token = Some(response.token.clone());
        guard.last_refusal = None;
        guard.last_refresh_ok_ts = Some(response.server_time);
        guard.last_refresh_failed = false;
        Ok(())
    }

    /// Active une clé saisie par l'utilisateur. En cas d'échec, l'état
    /// précédent (éventuelle autre clé) est conservé tel quel.
    pub async fn activate(&self, raw_key: &str) -> Result<LicenceView, LicenceError> {
        let key = key::normalize(raw_key).map_err(|_| LicenceError::new("invalid_key_format"))?;
        let _net = self.network.lock().await;
        let meta = self.meta(&key)?;
        let response = self
            .client
            .activate(&meta)
            .await
            .map_err(|e| LicenceError::from_api(&e))?;
        self.accept_token(&response, &key)?;
        self.persist();
        self.emit();
        log::info!("Licence activée sur ce poste (…{})", key::suffix(&key));
        Ok(self.view())
    }

    /// Renouvelle le jeton. `refresh` ne crée jamais d'activation : si un
    /// opérateur a libéré ce poste, le serveur répond `not_activated` et
    /// l'utilisateur doit ressaisir sa clé. Une erreur réseau garde le jeton.
    pub async fn refresh(&self) -> Result<LicenceView, LicenceError> {
        self.refresh_checked().await.0
    }

    /// Comme [`Self::refresh`], avec en plus le délai conseillé avant le
    /// prochain essai quand l'échec est passager (`Retry-After`).
    async fn refresh_checked(&self) -> (Result<LicenceView, LicenceError>, Option<Duration>) {
        if self.bypass {
            return (Ok(self.view()), None);
        }
        let key = self.lock().key.clone();
        let Some(key) = key else {
            return (Err(LicenceError::new("no_key")), None);
        };

        let _net = self.network.lock().await;
        let meta = match self.meta(&key) {
            Ok(meta) => meta,
            Err(e) => return (Err(e), None),
        };
        let result = self.client.refresh(&meta).await;
        let mut retry = None;
        let outcome = match &result {
            Ok(response) => self.accept_token(response, &key).inspect_err(|_| {
                // Jeton reçu mais rejeté : on garde l'ancien et on réessaiera.
                self.lock().last_refresh_failed = true;
                retry = Some(RETRY_INTERVAL);
            }),
            Err(err) => {
                if err.is_temporary() {
                    let wait = Duration::from_secs(err.retry_after().unwrap_or(0));
                    retry = Some(wait.max(RETRY_INTERVAL));
                }
                let mut guard = self.lock();
                if let ApiError::Refused {
                    server_time: Some(t),
                    ..
                } = err
                {
                    guard.observe_time(*t);
                }
                if err.is_definitive() {
                    guard.token = None;
                    guard.last_refusal = Some(err.code().to_string());
                    guard.last_refresh_failed = false;
                    log::warn!("Licence refusée par le serveur : {}", err.code());
                } else {
                    guard.last_refresh_failed = true;
                    log::info!("Vérification de licence impossible : {:?}", err);
                }
                Err(LicenceError::from_api(err))
            }
        };
        self.persist();
        self.emit();
        (outcome.map(|()| self.view()), retry)
    }

    /// Retire la clé de ce poste : libère l'emplacement sur le serveur, puis
    /// efface la clé localement. Si le serveur ne connaît plus ce poste ou
    /// cette clé, l'effacement local a lieu quand même.
    pub async fn remove(&self) -> Result<LicenceView, LicenceError> {
        let key = self.lock().key.clone();
        let Some(key) = key else {
            return Ok(self.view());
        };
        {
            let _net = self.network.lock().await;
            let meta = self.meta(&key)?;
            let server_time = match self.client.release(&meta).await {
                Ok(response) => response.server_time,
                Err(ApiError::Refused {
                    code, server_time, ..
                }) if matches!(code.as_str(), "not_activated" | "unknown_key") => server_time,
                Err(err) => return Err(LicenceError::from_api(&err)),
            };
            let mut guard = self.lock();
            if let Some(t) = server_time {
                guard.observe_time(t);
            }
            guard.key = None;
            guard.token = None;
            guard.last_refusal = None;
            guard.last_refresh_failed = false;
            guard.last_refresh_ok_ts = None;
        }
        self.persist();
        self.emit();
        log::info!("Licence retirée de ce poste");
        Ok(self.view())
    }

    /// Remplace la clé. La nouvelle est validée localement, puis activée
    /// **avant** de libérer l'ancienne : si le serveur la refuse, l'ancienne
    /// reste active et aucun quota de retrait n'est consommé. C'est possible
    /// parce que les emplacements sont comptés par licence : activer une autre
    /// licence n'a pas besoin de la place de l'ancienne. Si la libération de
    /// l'ancienne échoue ensuite (réseau), son emplacement reste occupé côté
    /// serveur jusqu'à ce qu'un opérateur le libère ; c'est journalisé.
    pub async fn change(&self, raw_key: &str) -> Result<LicenceView, LicenceError> {
        let new_key =
            key::normalize(raw_key).map_err(|_| LicenceError::new("invalid_key_format"))?;
        let old_key = self.lock().key.clone();
        if old_key.as_deref() == Some(new_key.as_str()) {
            return self.activate(&new_key).await;
        }
        let view = self.activate(&new_key).await?;
        if let Some(old) = old_key {
            if let Ok(meta) = self.meta(&old) {
                let _net = self.network.lock().await;
                if let Err(e) = self.client.release(&meta).await {
                    log::warn!("Ancienne clé non libérée sur le serveur : {:?}", e);
                }
            }
        }
        Ok(view)
    }

    /// Lit le mode d'application. Sans réponse, on garde le dernier connu.
    pub async fn fetch_mode(&self) {
        if self.bypass {
            return;
        }
        match self.client.mode().await {
            Ok(response) => {
                let changed = {
                    let mut guard = self.lock();
                    guard.observe_time(response.server_time);
                    let changed = guard.mode_cache != Some(response.mode);
                    guard.mode_cache = Some(response.mode);
                    changed
                };
                self.persist();
                if changed {
                    log::info!("Mode de licence : {:?}", response.mode);
                    self.emit();
                }
            }
            Err(e) => log::debug!("Mode de licence non lu : {:?}", e),
        }
    }

    /// Boucle de fond : mode et vérification au démarrage, puis toutes les
    /// 24 h, avec un nouvel essai toutes les 30 minutes après un échec.
    pub fn start_background(self: Arc<Self>) {
        if self.bypass {
            return;
        }
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(STARTUP_DELAY).await;
            loop {
                self.fetch_mode().await;
                let has_key = self.lock().key.is_some();
                // Échec passager : nouvel essai après 30 minutes, ou plus tard
                // si le serveur le demande (`Retry-After`). Jamais de boucle rapide.
                let next = if has_key {
                    self.refresh_checked().await.1.unwrap_or(CHECK_INTERVAL)
                } else {
                    CHECK_INTERVAL
                };
                tokio::time::sleep(next).await;
            }
        });
    }
}

fn hostname() -> Option<String> {
    sysinfo::System::host_name()
}

/// Autorise ou refuse le démarrage d'une dictée ou d'une transcription. En
/// mode souple, en développement, ou si la licence n'est pas encore
/// initialisée, c'est toujours autorisé.
pub fn ensure_allowed(app: &AppHandle) -> Result<(), LicenceError> {
    match app.try_state::<Arc<LicenceManager>>() {
        Some(manager) if !manager.is_allowed() => Err(LicenceError::new("licence_required")),
        _ => Ok(()),
    }
}
