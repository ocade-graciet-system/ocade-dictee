//! Client HTTPS de l'API publique du serveur de licences (`/api/v1`).
//!
//! Référence : le serveur `ocade-dictee-licences` (src/routes/public,
//! src/services/licensing.ts, src/services/errors.ts). Les décisions ne se
//! prennent que sur `error.code`, jamais sur `error.message`.

use super::store::LicenceMode;
use super::token::Seats;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Limites de taille imposées par le serveur (schéma `clientMetaSchema`).
pub const HOSTNAME_MAX: usize = 255;
pub const OS_TEXT_MAX: usize = 64;
pub const SHORT_TEXT_MAX: usize = 32;

/// Système du poste, envoyé pour l'identifier dans la supervision. Jamais
/// `null` : une valeur inconnue est envoyée comme chaîne vide.
#[derive(Debug, Clone, Serialize)]
pub struct OsInfo {
    pub name: String,
    pub version: String,
    pub arch: String,
}

/// Corps commun à `activate`, `refresh` et `release`.
#[derive(Debug, Clone, Serialize)]
pub struct ClientMeta {
    pub key: String,
    /// 64 caractères hexadécimaux **en minuscules**.
    pub fingerprint: String,
    pub hostname: String,
    pub os: OsInfo,
    pub app_version: String,
}

/// Coupe un texte pour respecter une longueur maximale **telle que le serveur
/// la compte** (unités UTF-16, comme `String.length` en JavaScript), sans
/// jamais couper un caractère en deux.
pub fn clip(text: &str, max_utf16: usize) -> String {
    let mut used = 0;
    let mut out = String::new();
    for ch in text.chars() {
        used += ch.len_utf16();
        if used > max_utf16 {
            break;
        }
        out.push(ch);
    }
    out
}

/// Réponse d'`activate` et de `refresh`. Le serveur renvoie aussi un résumé
/// `license` : l'application ne s'y fie pas et lit ces informations dans le
/// jeton signé, seule source qui fait foi.
#[derive(Debug, Clone, Deserialize)]
pub struct TokenResponse {
    pub token: String,
    pub server_time: i64,
}

/// Réponse de `release` ; seuls les champs utiles sont lus.
#[derive(Debug, Clone, Deserialize)]
pub struct ReleaseResponse {
    #[serde(default)]
    pub server_time: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ModeResponse {
    pub mode: LicenceMode,
    pub server_time: i64,
}

/// Erreur d'un appel au serveur.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiError {
    /// Réponse d'erreur du serveur, avec son code (`revoked`, `seats_full`…).
    Refused {
        code: String,
        server_time: Option<i64>,
        /// `error.expired_at`, pour `expired`.
        expired_at: Option<i64>,
        /// `error.seats` (utilisés, maximum), pour `seats_full`.
        seats: Option<(u32, u32)>,
        /// Secondes à attendre (`Retry-After`), pour `rate_limited`.
        retry_after: Option<u64>,
    },
    /// Serveur injoignable, délai dépassé, coupure, ou réponse illisible
    /// (proxy, page HTML) : on ne touche ni au jeton ni à la clé.
    Network(String),
}

impl ApiError {
    /// Refus qui efface le jeton : la licence est retirée, expirée, supprimée
    /// ou n'est plus activée sur ce poste.
    pub fn is_definitive(&self) -> bool {
        matches!(self, Self::Refused { code, .. } if matches!(code.as_str(),
            "revoked" | "archived" | "expired" | "unknown_key" | "not_activated"))
    }

    /// Incident passager : réseau, serveur en erreur, trop de requêtes.
    pub fn is_temporary(&self) -> bool {
        match self {
            Self::Network(_) => true,
            Self::Refused { code, .. } => {
                matches!(code.as_str(), "rate_limited" | "internal_error")
            }
        }
    }

    pub fn code(&self) -> &str {
        match self {
            Self::Refused { code, .. } => code,
            Self::Network(_) => "network",
        }
    }

    pub fn retry_after(&self) -> Option<u64> {
        match self {
            Self::Refused { retry_after, .. } => *retry_after,
            Self::Network(_) => None,
        }
    }
}

#[derive(Deserialize)]
struct ErrorBody {
    error: ErrorDetail,
    #[serde(default)]
    server_time: Option<i64>,
    /// Le limiteur global de Fastify place aussi la durée dans le corps.
    #[serde(default)]
    retry_after: Option<u64>,
}

#[derive(Deserialize)]
struct ErrorDetail {
    code: String,
    #[serde(default)]
    expired_at: Option<i64>,
    #[serde(default)]
    seats: Option<Seats>,
}

/// Traduit une réponse non 2xx en [`ApiError`]. `retry_after_header` est la
/// valeur de l'en-tête `Retry-After`, prioritaire sur le corps.
pub fn parse_error(status: u16, body: &[u8], retry_after_header: Option<u64>) -> ApiError {
    match serde_json::from_slice::<ErrorBody>(body) {
        Ok(parsed) => {
            if parsed.error.code == "invalid_request" {
                // Le serveur a rejeté le corps : c'est un défaut de l'application.
                log::error!(
                    "Serveur de licences : requête invalide (HTTP {status}), corps à corriger"
                );
            }
            ApiError::Refused {
                code: parsed.error.code,
                server_time: parsed.server_time,
                expired_at: parsed.error.expired_at,
                seats: parsed.error.seats.map(|s| (s.used, s.max)),
                retry_after: retry_after_header.or(parsed.retry_after),
            }
        }
        Err(_) => ApiError::Network(format!("HTTP {status} sans corps d'erreur exploitable")),
    }
}

pub struct LicenceClient {
    base_url: String,
    http: reqwest::Client,
}

impl LicenceClient {
    pub fn new(base_url: String) -> Self {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(20))
            .user_agent(concat!("OCADE-Dictee/", env!("CARGO_PKG_VERSION")))
            .build()
            .unwrap_or_default();
        Self { base_url, http }
    }

    async fn send<T: DeserializeOwned>(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<T, ApiError> {
        let response = request
            .send()
            .await
            .map_err(|e| ApiError::Network(e.to_string()))?;
        let status = response.status();
        let retry_after = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse::<u64>().ok());
        let body = response
            .bytes()
            .await
            .map_err(|e| ApiError::Network(e.to_string()))?;
        if status.is_success() {
            serde_json::from_slice(&body)
                .map_err(|e| ApiError::Network(format!("réponse illisible : {e}")))
        } else {
            Err(parse_error(status.as_u16(), &body, retry_after))
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}/{}", self.base_url, path)
    }

    pub async fn activate(&self, meta: &ClientMeta) -> Result<TokenResponse, ApiError> {
        self.send(self.http.post(self.url("activate")).json(meta))
            .await
    }

    pub async fn refresh(&self, meta: &ClientMeta) -> Result<TokenResponse, ApiError> {
        self.send(self.http.post(self.url("refresh")).json(meta))
            .await
    }

    pub async fn release(&self, meta: &ClientMeta) -> Result<ReleaseResponse, ApiError> {
        self.send(self.http.post(self.url("release")).json(meta))
            .await
    }

    pub async fn mode(&self) -> Result<ModeResponse, ApiError> {
        self.send(self.http.get(self.url("mode"))).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extra_fields_are_read_inside_error() {
        // Forme exacte du serveur : `{ error: { code, message, ...extra }, server_time }`.
        let body = br#"{"error":{"code":"seats_full","message":"x","seats":{"max":1,"used":1}},"server_time":10}"#;
        assert_eq!(
            parse_error(409, body, None),
            ApiError::Refused {
                code: "seats_full".into(),
                server_time: Some(10),
                expired_at: None,
                seats: Some((1, 1)),
                retry_after: None,
            }
        );
        let body = br#"{"error":{"code":"expired","message":"x","expired_at":99},"server_time":5}"#;
        let err = parse_error(403, body, None);
        assert!(err.is_definitive());
        assert!(matches!(
            err,
            ApiError::Refused {
                expired_at: Some(99),
                ..
            }
        ));
    }

    #[test]
    fn rate_limits_carry_their_delay_and_stay_temporary() {
        let body =
            br#"{"error":{"code":"rate_limited","message":"x"},"retry_after":42,"server_time":1}"#;
        let err = parse_error(429, body, None);
        assert_eq!(err.retry_after(), Some(42));
        assert!(err.is_temporary() && !err.is_definitive());
        assert_eq!(
            parse_error(429, body, Some(7)).retry_after(),
            Some(7),
            "en-tête prioritaire"
        );
    }

    #[test]
    fn unreadable_responses_count_as_network_errors() {
        for status in [400, 403, 502] {
            let err = parse_error(status, b"<html>Bad Gateway</html>", None);
            assert!(matches!(err, ApiError::Network(_)), "{status}");
            assert!(
                !err.is_definitive(),
                "une panne ne retire jamais la licence"
            );
        }
    }

    #[test]
    fn only_licence_refusals_are_definitive() {
        for code in [
            "seats_full",
            "rate_limited",
            "internal_error",
            "release_quota_exceeded",
            "invalid_key_format",
            "invalid_request",
        ] {
            let body = format!(r#"{{"error":{{"code":"{code}"}}}}"#);
            assert!(
                !parse_error(400, body.as_bytes(), None).is_definitive(),
                "{code}"
            );
        }
        for code in [
            "revoked",
            "archived",
            "expired",
            "unknown_key",
            "not_activated",
        ] {
            let body = format!(r#"{{"error":{{"code":"{code}"}}}}"#);
            assert!(
                parse_error(403, body.as_bytes(), None).is_definitive(),
                "{code}"
            );
        }
    }

    #[test]
    fn texts_are_clipped_as_the_server_counts_them() {
        assert_eq!(clip("abc", 2), "ab");
        assert_eq!(clip("été", 3), "été");
        // Un emoji compte pour 2 unités UTF-16 : il n'est pas coupé en deux.
        assert_eq!(clip("ab😀", 3), "ab");
        assert_eq!(clip(&"x".repeat(300), HOSTNAME_MAX).len(), HOSTNAME_MAX);
    }

    #[test]
    fn metadata_is_serialized_as_the_api_expects() {
        let meta = ClientMeta {
            key: "OCAD-7XQ2-M9KD-T4HW-3RCP".into(),
            fingerprint: "ab".repeat(32),
            hostname: "PC-COMPTA-02".into(),
            os: OsInfo {
                name: "Windows".into(),
                version: String::new(),
                arch: "x86_64".into(),
            },
            app_version: "1.8.0".into(),
        };
        let json = serde_json::to_value(&meta).unwrap();
        assert_eq!(json["os"]["arch"], "x86_64");
        assert_eq!(json["os"]["version"], "", "jamais null");
        assert_eq!(json["app_version"], "1.8.0");
        let mode: ModeResponse =
            serde_json::from_str(r#"{"mode":"strict","server_time":5}"#).unwrap();
        assert_eq!(mode.mode, LicenceMode::Strict);
    }
}
