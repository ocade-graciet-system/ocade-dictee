//! Jeton de licence : JWS compact signé Ed25519 par le serveur de licences.
//!
//! `base64url(header).base64url(payload).base64url(signature)`, la signature
//! portant sur les octets ASCII de `header.payload`. Le jeton prouve la licence
//! **sans réseau** pendant au plus 14 jours ; l'application ne fait confiance
//! qu'aux clés publiques qu'elle embarque (voir `keys.rs`).

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use ed25519_dalek::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};

/// Tolérance sur un jeton « émis dans le futur » : l'horloge du poste peut
/// retarder un peu sur celle du serveur.
const CLOCK_SKEW_SECS: i64 = 86_400;

/// Postes autorisés et utilisés sur la licence, au moment de l'émission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Seats {
    pub max: u32,
    pub used: u32,
}

/// Contenu signé du jeton. Horodatages en secondes Unix (UTC).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Claims {
    pub v: u32,
    /// Identifiant de la licence.
    pub lid: String,
    /// Identifiant de l'activation (le poste).
    pub aid: String,
    /// Empreinte du poste à qui le jeton a été délivré.
    pub fp: String,
    /// Quatre derniers symboles de la clé (affichage).
    pub kfx: String,
    pub iat: i64,
    /// Fin de validité du jeton : min(émission + 14 jours, échéance de la licence).
    pub exp: i64,
    /// Échéance de la licence ; `None` pour une licence sans échéance.
    pub lexp: Option<i64>,
    pub seats: Seats,
}

#[derive(Deserialize)]
struct Header {
    alg: String,
    kid: String,
}

/// Clé publique de confiance, désignée par son identifiant (`kid`).
#[derive(Clone)]
pub struct TrustedKey {
    pub kid: String,
    pub key: VerifyingKey,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenError {
    /// Structure, encodage ou JSON invalides.
    Malformed,
    /// Aucune clé publique embarquée ne porte ce `kid`.
    UnknownKeyId,
    /// La signature ne correspond pas : jeton modifié ou contrefait.
    BadSignature,
    /// Jeton délivré à un autre poste.
    WrongMachine,
    /// Émis trop loin dans le futur par rapport à l'horloge du poste.
    NotYetValid,
    /// Fin de validité dépassée.
    Expired,
}

fn decode_part(part: &str) -> Result<Vec<u8>, TokenError> {
    URL_SAFE_NO_PAD
        .decode(part.trim_end_matches('='))
        .map_err(|_| TokenError::Malformed)
}

/// Vérifie la structure et la signature, puis renvoie le contenu. Ne regarde
/// **ni** l'heure **ni** le poste : voir [`check_validity`].
pub fn decode_verified(token: &str, keys: &[TrustedKey]) -> Result<Claims, TokenError> {
    let mut parts = token.split('.');
    let (Some(header_b64), Some(payload_b64), Some(signature_b64), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(TokenError::Malformed);
    };

    let header: Header =
        serde_json::from_slice(&decode_part(header_b64)?).map_err(|_| TokenError::Malformed)?;
    if header.alg != "EdDSA" {
        return Err(TokenError::Malformed);
    }
    let trusted = keys
        .iter()
        .find(|k| k.kid == header.kid)
        .ok_or(TokenError::UnknownKeyId)?;

    let signature =
        Signature::from_slice(&decode_part(signature_b64)?).map_err(|_| TokenError::Malformed)?;
    let signing_input = format!("{header_b64}.{payload_b64}");
    trusted
        .key
        .verify_strict(signing_input.as_bytes(), &signature)
        .map_err(|_| TokenError::BadSignature)?;

    let claims: Claims =
        serde_json::from_slice(&decode_part(payload_b64)?).map_err(|_| TokenError::Malformed)?;
    if claims.v != 1 {
        return Err(TokenError::Malformed);
    }
    Ok(claims)
}

/// Le jeton vaut-il pour **ce** poste, **maintenant** (horloge effective) ?
pub fn check_validity(claims: &Claims, fingerprint: &str, now: i64) -> Result<(), TokenError> {
    if claims.fp != fingerprint {
        return Err(TokenError::WrongMachine);
    }
    if claims.iat - CLOCK_SKEW_SECS > now {
        return Err(TokenError::NotYetValid);
    }
    if now >= claims.exp {
        return Err(TokenError::Expired);
    }
    Ok(())
}

/// Décode une clé publique brute de 32 octets en **base64 standard** (avec
/// `+`, `/` et `=`), telle qu'affichée par `npm run gen:key` côté serveur.
pub fn parse_public_key(kid: &str, b64: &str) -> Option<TrustedKey> {
    use base64::engine::general_purpose::STANDARD;
    let bytes: [u8; 32] = STANDARD.decode(b64.trim()).ok()?.try_into().ok()?;
    VerifyingKey::from_bytes(&bytes).ok().map(|key| TrustedKey {
        kid: kid.to_string(),
        key,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    /// Paire **de test** déterministe. Jamais utilisée en production.
    pub(crate) fn test_signing_key() -> SigningKey {
        // Générée au vol à chaque exécution : aucune clé privée, même de
        // test, n'est écrite dans le dépôt.
        static KEY: std::sync::OnceLock<[u8; 32]> = std::sync::OnceLock::new();
        SigningKey::from_bytes(KEY.get_or_init(rand::random))
    }

    pub(crate) fn test_keys() -> Vec<TrustedKey> {
        vec![TrustedKey {
            kid: "test".into(),
            key: test_signing_key().verifying_key(),
        }]
    }

    pub(crate) fn sample_claims() -> Claims {
        Claims {
            v: 1,
            lid: "lic-1".into(),
            aid: "act-1".into(),
            fp: "fp-poste".into(),
            kfx: "3RCP".into(),
            iat: 1_000_000,
            exp: 1_000_000 + 14 * 86_400,
            lexp: Some(1_000_000 + 200 * 86_400),
            seats: Seats { max: 2, used: 1 },
        }
    }

    /// Signe comme le serveur (Node `crypto.sign(null, data, privateKey)`).
    pub(crate) fn sign(claims: &Claims, kid: &str, key: &SigningKey) -> String {
        let header = serde_json::json!({ "alg": "EdDSA", "typ": "OCADE-LIC", "kid": kid });
        let h = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&header).unwrap());
        let p = URL_SAFE_NO_PAD.encode(serde_json::to_vec(claims).unwrap());
        let signature = key.sign(format!("{h}.{p}").as_bytes());
        format!("{h}.{p}.{}", URL_SAFE_NO_PAD.encode(signature.to_bytes()))
    }

    #[test]
    fn a_genuine_token_is_decoded() {
        let claims = sample_claims();
        let token = sign(&claims, "test", &test_signing_key());
        assert_eq!(decode_verified(&token, &test_keys()), Ok(claims));
    }

    #[test]
    fn a_tampered_payload_is_rejected() {
        let token = sign(&sample_claims(), "test", &test_signing_key());
        let mut forged = sample_claims();
        forged.lexp = None; // « licence sans échéance »
        let forged_payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&forged).unwrap());
        let parts: Vec<&str> = token.split('.').collect();
        let tampered = format!("{}.{}.{}", parts[0], forged_payload, parts[2]);
        assert_eq!(
            decode_verified(&tampered, &test_keys()),
            Err(TokenError::BadSignature)
        );
    }

    #[test]
    fn another_signer_or_kid_is_rejected() {
        let other = SigningKey::from_bytes(&rand::random());
        let token = sign(&sample_claims(), "test", &other);
        assert_eq!(
            decode_verified(&token, &test_keys()),
            Err(TokenError::BadSignature)
        );
        let token = sign(&sample_claims(), "k2", &test_signing_key());
        assert_eq!(
            decode_verified(&token, &test_keys()),
            Err(TokenError::UnknownKeyId)
        );
        assert_eq!(
            decode_verified("pas-un-jeton", &test_keys()),
            Err(TokenError::Malformed)
        );
    }

    #[test]
    fn validity_depends_on_machine_and_time() {
        let claims = sample_claims();
        assert_eq!(check_validity(&claims, "fp-poste", claims.iat), Ok(()));
        assert_eq!(
            check_validity(&claims, "autre-poste", claims.iat),
            Err(TokenError::WrongMachine)
        );
        assert_eq!(
            check_validity(&claims, "fp-poste", claims.exp),
            Err(TokenError::Expired)
        );
        assert_eq!(
            check_validity(&claims, "fp-poste", claims.iat - CLOCK_SKEW_SECS),
            Ok(()),
            "un jour de retard d'horloge est toléré"
        );
        assert_eq!(
            check_validity(&claims, "fp-poste", claims.iat - CLOCK_SKEW_SECS - 1),
            Err(TokenError::NotYetValid)
        );
    }

    #[test]
    fn public_keys_are_parsed_from_standard_base64() {
        use base64::engine::general_purpose::STANDARD;
        let raw = test_signing_key().verifying_key().to_bytes();
        let parsed = parse_public_key("k1", &STANDARD.encode(raw)).unwrap();
        assert_eq!(parsed.key.to_bytes(), raw);
        assert_eq!(parsed.kid, "k1");
        assert!(parse_public_key("k1", "trop-court").is_none());
    }

    #[test]
    fn signature_covers_the_received_ascii_exactly() {
        // Même JSON, encodage différent (espace ajouté) : la signature ne vaut
        // que pour les octets reçus, jamais pour un JSON réencodé.
        let token = sign(&sample_claims(), "test", &test_signing_key());
        let parts: Vec<&str> = token.split('.').collect();
        let mut json = String::from_utf8(URL_SAFE_NO_PAD.decode(parts[1]).unwrap()).unwrap();
        json.insert(1, ' ');
        let reencoded = format!("{}.{}.{}", parts[0], URL_SAFE_NO_PAD.encode(json), parts[2]);
        assert_eq!(
            decode_verified(&reencoded, &test_keys()),
            Err(TokenError::BadSignature)
        );
    }

    #[test]
    fn unknown_payload_fields_and_null_lexp_are_accepted() {
        // Compatibilité avec une future v2 du jeton et licence sans échéance.
        let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"EdDSA","typ":"OCADE-LIC","kid":"test"}"#);
        let payload = URL_SAFE_NO_PAD.encode(
            br#"{"v":1,"lid":"l","aid":"a","fp":"f","kfx":"3RCP","iat":1,"exp":2,"lexp":null,"seats":{"max":1,"used":1},"nouveau":true}"#,
        );
        let signature = test_signing_key().sign(format!("{header}.{payload}").as_bytes());
        let token = format!(
            "{header}.{payload}.{}",
            URL_SAFE_NO_PAD.encode(signature.to_bytes())
        );
        let claims = decode_verified(&token, &test_keys()).unwrap();
        assert_eq!(claims.lexp, None);
    }

    #[test]
    fn only_eddsa_is_accepted() {
        let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"none","kid":"test"}"#);
        let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&sample_claims()).unwrap());
        let token = format!("{header}.{payload}.");
        assert_eq!(
            decode_verified(&token, &test_keys()),
            Err(TokenError::Malformed)
        );
    }
}
