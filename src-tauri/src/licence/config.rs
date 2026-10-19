//! Configuration figée dans l'application : adresse du serveur de licences et
//! clés publiques de confiance.
//!
//! **À compléter avant la première publication** : la clé publique de
//! production `k1` (`PRODUCTION_KEYS`) est affichée par `npm run gen:key` dans le
//! dépôt `ocade-dictee-licences`, au moment de la mise en service du serveur.
//! Tant que la liste est vide, aucun jeton n'est accepté hors build de
//! débogage : en mode souple l'application reste utilisable, mais aucune clé ne
//! peut s'activer. La PR qui l'ajoute doit être fusionnée **avant** toute
//! publication qui compte sur l'activation.

use super::token::{parse_public_key, TrustedKey};

/// Adresse de l'API publique de production. Un changement de domaine
/// demanderait une nouvelle version de l'application.
pub const PRODUCTION_URL: &str = "https://licences-ocade-dictee.graciet.org/api/v1";

/// Clés publiques Ed25519 de production : (`kid`, 32 octets bruts en **base64
/// standard**, avec `+`, `/` et `=`). Plusieurs entrées permettent une rotation
/// sans coupure : on publie d'abord une version qui connaît la nouvelle clé,
/// puis le serveur bascule dessus.
pub const PRODUCTION_KEYS: &[(&str, &str)] = &[];

/// Réglages effectifs, résolus une fois au démarrage.
pub struct LicenceConfig {
    pub base_url: String,
    pub keys: Vec<TrustedKey>,
    /// Licence ignorée : uniquement possible dans un build de débogage.
    pub bypass: bool,
}

/// Lit `kid:base64[,kid:base64…]`. Les entrées illisibles sont ignorées et
/// signalées dans le journal.
fn parse_key_list(spec: &str) -> Vec<TrustedKey> {
    spec.split(',')
        .filter(|entry| !entry.trim().is_empty())
        .filter_map(|entry| {
            let parsed = entry
                .split_once(':')
                .and_then(|(kid, b64)| parse_public_key(kid.trim(), b64));
            if parsed.is_none() {
                log::warn!("OCADE_LICENCE_PUBKEYS : entrée illisible (attendu kid:base64)");
            }
            parsed
        })
        .collect()
}

impl LicenceConfig {
    pub fn resolve() -> Self {
        let mut keys: Vec<TrustedKey> = PRODUCTION_KEYS
            .iter()
            .filter_map(|(kid, b64)| parse_public_key(kid, b64))
            .collect();

        // Build de débogage (`tauri dev`) : la licence est ignorée, sauf si l'on
        // pointe vers un serveur de licences local avec `OCADE_LICENCE_URL`. Ce
        // serveur fournit sa clé publique par `OCADE_LICENCE_PUBKEYS`
        // (`k1:<base64>`). Ces variables sont **sans effet en release** : un
        // client ne peut ni contourner ni rediriger la licence.
        if cfg!(debug_assertions) {
            let dev_url = std::env::var("OCADE_LICENCE_URL")
                .ok()
                .map(|u| u.trim().trim_end_matches('/').to_string())
                .filter(|u| !u.is_empty());
            let forced_bypass = std::env::var("OCADE_LICENCE_BYPASS").as_deref() == Ok("1");
            if let Ok(spec) = std::env::var("OCADE_LICENCE_PUBKEYS") {
                keys.extend(parse_key_list(&spec));
            }
            return Self {
                base_url: dev_url
                    .clone()
                    .unwrap_or_else(|| PRODUCTION_URL.to_string()),
                keys,
                bypass: forced_bypass || dev_url.is_none(),
            };
        }

        if keys.is_empty() {
            log::warn!(
                "Aucune clé publique de licence de production embarquée : aucune activation possible"
            );
        }
        Self {
            base_url: PRODUCTION_URL.to_string(),
            keys,
            bypass: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{engine::general_purpose::STANDARD, Engine};

    #[test]
    fn key_lists_accept_several_entries_and_skip_bad_ones() {
        // Clés publiques valides (points de la courbe), générées au vol.
        let public = || {
            ed25519_dalek::SigningKey::from_bytes(&rand::random())
                .verifying_key()
                .to_bytes()
        };
        let a = STANDARD.encode(public());
        let b = STANDARD.encode(public());
        let keys = parse_key_list(&format!("k1:{a}, k2:{b},cassé,k3:court"));
        let kids: Vec<&str> = keys.iter().map(|k| k.kid.as_str()).collect();
        assert_eq!(kids, ["k1", "k2"]);
    }
}
