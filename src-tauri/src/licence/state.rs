//! Calcul de l'état de la licence : fonction pure, sans réseau ni horloge
//! implicite, pour être entièrement testable.

use super::store::{LicenceMode, Persisted};
use super::token::{check_validity, Claims, TokenError};
use serde::{Deserialize, Serialize};
use specta::Type;

const DAY: i64 = 86_400;
/// Seuils des rappels dans l'application.
pub const SOON_DAYS: i64 = 30;
pub const CRITICAL_DAYS: i64 = 7;

/// État de la licence sur ce poste.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum LicenceState {
    /// Build de développement : licence ignorée.
    Development,
    /// Aucune clé activée sur ce poste.
    NotActivated,
    Active,
    /// Échéance à 30 jours ou moins.
    ExpiringSoon,
    /// Échéance à 7 jours ou moins.
    ExpiringCritical,
    /// Jeton encore valide, mais la dernière vérification a échoué.
    OfflineGrace,
    /// Jeton périmé sans réponse du serveur (plus de 14 jours hors ligne).
    TokenExpired,
    Revoked,
    Archived,
    Expired,
    /// Clé inconnue du serveur (supprimée).
    UnknownKey,
    /// La clé n'est plus activée sur ce poste (poste libéré, autre machine).
    NotActivatedHere,
    /// Jeton illisible ou de signature invalide.
    Invalid,
}

impl LicenceState {
    /// En mode strict, cet état bloque-t-il la dictée ?
    pub fn is_blocking(self) -> bool {
        !matches!(
            self,
            Self::Development
                | Self::Active
                | Self::ExpiringSoon
                | Self::ExpiringCritical
                | Self::OfflineGrace
        )
    }
}

/// Vue complète, envoyée telle quelle à l'interface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct LicenceView {
    pub state: LicenceState,
    pub mode: LicenceMode,
    /// L'état bloquerait la dictée en mode strict.
    pub blocking: bool,
    /// La dictée est **effectivement** bloquée : état bloquant et mode strict.
    pub enforced: bool,
    /// Quatre derniers symboles de la clé.
    pub key_suffix: Option<String>,
    /// Échéance de la licence (secondes Unix) ; `None` : sans échéance ou inconnue.
    pub expires_at: Option<i64>,
    /// Jours restants avant l'échéance, arrondis au jour supérieur.
    pub days_left: Option<i64>,
    /// Fin de validité du jeton : date limite de reconnexion hors ligne.
    pub token_expires_at: Option<i64>,
    pub seats_max: Option<u32>,
    pub seats_used: Option<u32>,
    pub hostname: Option<String>,
    pub last_check_at: Option<i64>,
    /// Code du dernier refus du serveur, s'il y en a un.
    pub refusal: Option<String>,
}

/// Entrées du calcul. `decoded` est le résultat de la vérification de
/// signature du jeton stocké (`None` s'il n'y a pas de jeton).
pub struct Inputs<'a> {
    pub persisted: &'a Persisted,
    pub decoded: Option<Result<Claims, TokenError>>,
    pub fingerprint: Option<&'a str>,
    /// Heure effective (voir `Persisted::effective_now`).
    pub now: i64,
    pub bypass: bool,
    pub hostname: Option<String>,
}

fn ceil_days(seconds: i64) -> i64 {
    if seconds <= 0 {
        0
    } else {
        (seconds + DAY - 1) / DAY
    }
}

fn refusal_state(code: &str) -> LicenceState {
    match code {
        "revoked" => LicenceState::Revoked,
        "archived" => LicenceState::Archived,
        "expired" => LicenceState::Expired,
        "unknown_key" => LicenceState::UnknownKey,
        _ => LicenceState::NotActivatedHere,
    }
}

pub fn compute(inputs: Inputs<'_>) -> LicenceView {
    let p = inputs.persisted;
    let mode = p.mode_cache.unwrap_or_default();
    let claims = inputs.decoded.as_ref().and_then(|d| d.as_ref().ok());

    let state = if inputs.bypass {
        LicenceState::Development
    } else if p.key.is_none() {
        LicenceState::NotActivated
    } else if let Some(code) = &p.last_refusal {
        refusal_state(code)
    } else {
        match &inputs.decoded {
            None => LicenceState::NotActivated,
            Some(Err(_)) => LicenceState::Invalid,
            Some(Ok(claims)) => {
                let fingerprint = inputs.fingerprint.unwrap_or_default();
                match check_validity(claims, fingerprint, inputs.now) {
                    Err(TokenError::WrongMachine) => LicenceState::NotActivatedHere,
                    Err(TokenError::Expired) => {
                        if claims.lexp.is_some_and(|lexp| inputs.now >= lexp) {
                            LicenceState::Expired
                        } else {
                            LicenceState::TokenExpired
                        }
                    }
                    Err(_) => LicenceState::Invalid,
                    Ok(()) => {
                        let days = claims.lexp.map(|lexp| ceil_days(lexp - inputs.now));
                        match days {
                            Some(d) if d <= CRITICAL_DAYS => LicenceState::ExpiringCritical,
                            _ if p.last_refresh_failed => LicenceState::OfflineGrace,
                            Some(d) if d <= SOON_DAYS => LicenceState::ExpiringSoon,
                            _ => LicenceState::Active,
                        }
                    }
                }
            }
        }
    };

    let blocking = state.is_blocking();
    LicenceView {
        state,
        mode,
        blocking,
        enforced: blocking && mode == LicenceMode::Strict && !inputs.bypass,
        key_suffix: p.key.as_deref().map(super::key::suffix),
        expires_at: claims.and_then(|c| c.lexp),
        days_left: claims.and_then(|c| c.lexp.map(|l| ceil_days(l - inputs.now))),
        token_expires_at: claims.map(|c| c.exp),
        seats_max: claims.map(|c| c.seats.max),
        seats_used: claims.map(|c| c.seats.used),
        hostname: inputs.hostname,
        last_check_at: p.last_refresh_ok_ts,
        refusal: p.last_refusal.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::licence::token::tests::sample_claims;

    const FP: &str = "fp-poste";

    fn persisted_with_key() -> Persisted {
        Persisted {
            key: Some("OCAD-7XQ2-M9KD-T4HW-3RCP".into()),
            token: Some("x".into()),
            ..Default::default()
        }
    }

    fn run(p: &Persisted, claims: Option<Result<Claims, TokenError>>, now: i64) -> LicenceView {
        compute(Inputs {
            persisted: p,
            decoded: claims,
            fingerprint: Some(FP),
            now,
            bypass: false,
            hostname: None,
        })
    }

    #[test]
    fn without_key_the_app_is_not_activated_and_soft_by_default() {
        let view = run(&Persisted::default(), None, 0);
        assert_eq!(view.state, LicenceState::NotActivated);
        assert_eq!(view.mode, LicenceMode::Soft);
        assert!(view.blocking);
        assert!(!view.enforced, "le mode souple ne bloque jamais");
    }

    #[test]
    fn strict_mode_enforces_blocking_states_only() {
        let mut p = Persisted {
            mode_cache: Some(LicenceMode::Strict),
            ..Default::default()
        };
        assert!(run(&p, None, 0).enforced);
        p = Persisted {
            mode_cache: Some(LicenceMode::Strict),
            ..persisted_with_key()
        };
        let c = sample_claims();
        let view = run(&p, Some(Ok(c.clone())), c.iat);
        assert_eq!(view.state, LicenceState::Active);
        assert!(!view.enforced);
    }

    #[test]
    fn days_left_drive_the_reminders() {
        let p = persisted_with_key();
        let mut c = sample_claims();
        let now = c.iat;
        c.lexp = Some(now + 200 * DAY);
        assert_eq!(
            run(&p, Some(Ok(c.clone())), now).state,
            LicenceState::Active
        );
        c.lexp = Some(now + 30 * DAY);
        let view = run(&p, Some(Ok(c.clone())), now);
        assert_eq!(view.state, LicenceState::ExpiringSoon);
        assert_eq!(view.days_left, Some(30));
        c.lexp = Some(now + 6 * DAY + 1);
        c.exp = c.exp.min(c.lexp.unwrap());
        let view = run(&p, Some(Ok(c.clone())), now);
        assert_eq!(view.state, LicenceState::ExpiringCritical);
        assert_eq!(
            view.days_left,
            Some(7),
            "6 jours et une seconde : 7 jours entamés"
        );
        c.lexp = None;
        let view = run(&p, Some(Ok(c)), now);
        assert_eq!(view.state, LicenceState::Active);
        assert_eq!(view.days_left, None, "licence sans échéance");
    }

    #[test]
    fn a_failed_check_shows_the_offline_grace() {
        let p = Persisted {
            last_refresh_failed: true,
            ..persisted_with_key()
        };
        let c = sample_claims();
        let view = run(&p, Some(Ok(c.clone())), c.iat);
        assert_eq!(view.state, LicenceState::OfflineGrace);
        assert_eq!(view.token_expires_at, Some(c.exp));
        assert!(!view.blocking);
    }

    #[test]
    fn an_old_token_distinguishes_expiry_from_absence() {
        let p = persisted_with_key();
        let c = sample_claims();
        assert_eq!(
            run(&p, Some(Ok(c.clone())), c.exp).state,
            LicenceState::TokenExpired,
            "14 jours sans réseau, licence encore valable"
        );
        assert_eq!(
            run(&p, Some(Ok(c.clone())), c.lexp.unwrap()).state,
            LicenceState::Expired
        );
    }

    #[test]
    fn refusals_and_bad_tokens_block() {
        let mut p = persisted_with_key();
        for (code, state) in [
            ("revoked", LicenceState::Revoked),
            ("archived", LicenceState::Archived),
            ("expired", LicenceState::Expired),
            ("unknown_key", LicenceState::UnknownKey),
            ("not_activated", LicenceState::NotActivatedHere),
        ] {
            p.last_refusal = Some(code.into());
            let view = run(&p, None, 0);
            assert_eq!(view.state, state, "{code}");
            assert!(view.blocking);
        }
        p.last_refusal = None;
        assert_eq!(
            run(&p, Some(Err(TokenError::BadSignature)), 0).state,
            LicenceState::Invalid
        );
        let mut c = sample_claims();
        c.fp = "autre-poste".into();
        assert_eq!(
            run(&p, Some(Ok(c.clone())), c.iat).state,
            LicenceState::NotActivatedHere
        );
    }

    #[test]
    fn development_never_blocks() {
        let p = Persisted {
            mode_cache: Some(LicenceMode::Strict),
            ..Default::default()
        };
        let view = compute(Inputs {
            persisted: &p,
            decoded: None,
            fingerprint: None,
            now: 0,
            bypass: true,
            hostname: None,
        });
        assert_eq!(view.state, LicenceState::Development);
        assert!(!view.enforced);
    }
}
