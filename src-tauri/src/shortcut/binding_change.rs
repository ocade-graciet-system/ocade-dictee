//! Changement de raccourci avec retour arrière (spec 08 §4).
//!
//! Avant la 1.1.0, `change_binding` désenregistrait l'ancien raccourci **avant**
//! de valider le nouveau et ne le remettait pas en cas d'échec : l'utilisateur
//! se retrouvait sans dictée jusqu'au redémarrage. La séquence est désormais
//! valider → désenregistrer → enregistrer → **ré-enregistrer l'ancien si
//! l'enregistrement échoue**, dans une fonction sans `AppHandle`, testable avec
//! un registrar factice.

use log::{error, warn};
use serde::Serialize;
use specta::Type;

use crate::settings::ShortcutBinding;
use crate::shortcut::rules::{validate_custom_shortcut, ShortcutRejection, TargetOs};

/// Enregistrement effectif auprès de l'implémentation clavier active.
/// Abstrait pour que `apply_binding_change` reste testable hors Tauri.
pub trait ShortcutRegistrar {
    fn register(&mut self, binding: &ShortcutBinding) -> Result<(), String>;
    fn unregister(&mut self, binding: &ShortcutBinding) -> Result<(), String>;
}

/// Code de refus envoyé au front, qui le traduit via
/// `settings.general.shortcut.rejections.*`. Énumération **plate** : la spec
/// range `registrationFailed` au même niveau que les raisons de refus.
#[derive(Serialize, Type, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum BindingErrorCode {
    Empty,
    Unparseable,
    NoKey,
    MultipleKeys,
    NoModifier,
    ShiftOnlyWithPrintable,
    EscapeKey,
    ReservedBySystem,
    RegistrationFailed,
    UnknownBinding,
}

/// Échec d'un changement de raccourci. `previous_binding` est le raccourci
/// **resté actif** : le front y revient et le cite dans son message.
#[derive(Serialize, Type, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BindingError {
    pub code: BindingErrorCode,
    pub previous_binding: String,
    /// Complément affichable : combinaison réservée, message du système,
    /// jeton non reconnu.
    pub detail: Option<String>,
}

impl BindingError {
    /// Traduit un refus de `rules` en erreur transmissible au front.
    pub fn from_rejection(rejection: ShortcutRejection, previous_binding: &str) -> Self {
        let (code, detail) = match rejection {
            ShortcutRejection::Empty => (BindingErrorCode::Empty, None),
            ShortcutRejection::Unparseable { detail } => {
                (BindingErrorCode::Unparseable, Some(detail))
            }
            ShortcutRejection::NoKey => (BindingErrorCode::NoKey, None),
            ShortcutRejection::MultipleKeys => (BindingErrorCode::MultipleKeys, None),
            ShortcutRejection::NoModifier => (BindingErrorCode::NoModifier, None),
            ShortcutRejection::ShiftOnlyWithPrintable => {
                (BindingErrorCode::ShiftOnlyWithPrintable, None)
            }
            ShortcutRejection::EscapeKey => (BindingErrorCode::EscapeKey, None),
            ShortcutRejection::ReservedBySystem { combo } => {
                (BindingErrorCode::ReservedBySystem, Some(combo))
            }
        };
        Self {
            code,
            previous_binding: previous_binding.to_string(),
            detail,
        }
    }

    /// Le système a refusé d'enregistrer la combinaison (déjà prise, backend
    /// clavier incompatible…).
    pub fn registration_failed(previous_binding: &str, detail: String) -> Self {
        Self {
            code: BindingErrorCode::RegistrationFailed,
            previous_binding: previous_binding.to_string(),
            detail: Some(detail),
        }
    }

    /// L'identifiant de raccourci demandé n'existe ni dans les réglages ni
    /// dans les défauts.
    pub fn unknown_binding(id: &str) -> Self {
        Self {
            code: BindingErrorCode::UnknownBinding,
            previous_binding: String::new(),
            detail: Some(id.to_string()),
        }
    }
}

/// Applique un changement de raccourci. Ne touche jamais aux réglages :
/// l'appelant ne persiste qu'en cas de `Ok`.
pub fn apply_binding_change(
    previous: &ShortcutBinding,
    requested: &str,
    os: TargetOs,
    registrar: &mut impl ShortcutRegistrar,
) -> Result<ShortcutBinding, BindingError> {
    // 1. Les règles d'abord : un refus ne doit rien désenregistrer.
    if let Err(rejection) = validate_custom_shortcut(requested, os) {
        warn!(
            "Raccourci « {requested} » refusé ({rejection:?}) ; « {} » conservé",
            previous.current_binding
        );
        return Err(BindingError::from_rejection(
            rejection,
            &previous.current_binding,
        ));
    }

    // 2. Libérer l'ancien. Un échec ici n'est pas bloquant (il peut ne pas
    //    avoir été enregistré), mais il est journalisé.
    if let Err(e) = registrar.unregister(previous) {
        warn!(
            "Désenregistrement de « {} » impossible : {e}",
            previous.current_binding
        );
    }

    let mut updated = previous.clone();
    updated.current_binding = requested.trim().to_string();

    // 3. Enregistrer le nouveau ; 4. en cas d'échec, remettre l'ancien.
    if let Err(e) = registrar.register(&updated) {
        error!(
            "Enregistrement de « {} » refusé par le système : {e}",
            updated.current_binding
        );
        if let Err(restore) = registrar.register(previous) {
            error!(
                "Ré-enregistrement de « {} » impossible : {restore}",
                previous.current_binding
            );
        }
        return Err(BindingError::registration_failed(
            &previous.current_binding,
            e,
        ));
    }

    Ok(updated)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::ShortcutBinding;
    use crate::shortcut::rules::{ShortcutRejection, TargetOs};

    /// Registrar factice : journalise les appels et sait échouer à la demande.
    #[derive(Default)]
    struct FakeRegistrar {
        calls: Vec<String>,
        /// Raccourci dont l'enregistrement doit échouer.
        fail_register_for: Option<String>,
        /// Fait échouer tous les désenregistrements.
        fail_unregister: bool,
    }

    impl ShortcutRegistrar for FakeRegistrar {
        fn register(&mut self, binding: &ShortcutBinding) -> Result<(), String> {
            self.calls
                .push(format!("register:{}", binding.current_binding));
            if self.fail_register_for.as_deref() == Some(binding.current_binding.as_str()) {
                return Err("le système a refusé".to_string());
            }
            Ok(())
        }

        fn unregister(&mut self, binding: &ShortcutBinding) -> Result<(), String> {
            self.calls
                .push(format!("unregister:{}", binding.current_binding));
            if self.fail_unregister {
                return Err("désenregistrement impossible".to_string());
            }
            Ok(())
        }
    }

    fn binding(current: &str) -> ShortcutBinding {
        ShortcutBinding {
            id: "transcribe".to_string(),
            name: "Transcribe".to_string(),
            description: "Converts your speech into text.".to_string(),
            default_binding: "ctrl+shift+space".to_string(),
            current_binding: current.to_string(),
        }
    }

    #[test]
    fn a_rejected_shortcut_never_touches_the_registrar() {
        let previous = binding("ctrl+shift+space");
        let mut registrar = FakeRegistrar::default();

        let error =
            apply_binding_change(&previous, "cmd+q", TargetOs::MacOs, &mut registrar).unwrap_err();

        assert_eq!(error.code, BindingErrorCode::ReservedBySystem);
        assert_eq!(error.previous_binding, "ctrl+shift+space");
        assert_eq!(error.detail.as_deref(), Some("cmd+q"));
        assert!(
            registrar.calls.is_empty(),
            "le registrar a été appelé : {:?}",
            registrar.calls
        );
    }

    #[test]
    fn a_registration_failure_restores_the_previous_shortcut() {
        let previous = binding("ctrl+shift+space");
        let mut registrar = FakeRegistrar {
            fail_register_for: Some("ctrl+option+j".to_string()),
            ..FakeRegistrar::default()
        };

        let error =
            apply_binding_change(&previous, "ctrl+option+j", TargetOs::MacOs, &mut registrar)
                .unwrap_err();

        assert_eq!(error.code, BindingErrorCode::RegistrationFailed);
        assert_eq!(error.previous_binding, "ctrl+shift+space");
        assert_eq!(error.detail.as_deref(), Some("le système a refusé"));
        assert_eq!(
            registrar.calls,
            vec![
                "unregister:ctrl+shift+space",
                "register:ctrl+option+j",
                "register:ctrl+shift+space",
            ]
        );
    }

    #[test]
    fn a_successful_change_unregisters_then_registers() {
        let previous = binding("ctrl+shift+space");
        let mut registrar = FakeRegistrar::default();

        let updated =
            apply_binding_change(&previous, "ctrl+option+j", TargetOs::MacOs, &mut registrar)
                .unwrap();

        assert_eq!(updated.current_binding, "ctrl+option+j");
        assert_eq!(updated.id, "transcribe");
        assert_eq!(updated.default_binding, "ctrl+shift+space");
        assert_eq!(
            registrar.calls,
            vec!["unregister:ctrl+shift+space", "register:ctrl+option+j"]
        );
    }

    #[test]
    fn a_failed_unregistration_does_not_block_the_change() {
        let previous = binding("ctrl+shift+space");
        let mut registrar = FakeRegistrar {
            fail_unregister: true,
            ..FakeRegistrar::default()
        };

        let updated =
            apply_binding_change(&previous, "ctrl+option+j", TargetOs::MacOs, &mut registrar)
                .unwrap();

        assert_eq!(updated.current_binding, "ctrl+option+j");
        assert_eq!(
            registrar.calls,
            vec!["unregister:ctrl+shift+space", "register:ctrl+option+j"]
        );
    }

    #[test]
    fn the_requested_shortcut_is_trimmed() {
        let previous = binding("ctrl+shift+space");
        let mut registrar = FakeRegistrar::default();

        let updated = apply_binding_change(
            &previous,
            "  ctrl+option+j  ",
            TargetOs::MacOs,
            &mut registrar,
        )
        .unwrap();

        assert_eq!(updated.current_binding, "ctrl+option+j");
    }

    #[test]
    fn errors_serialise_in_camel_case() {
        let error = BindingError {
            code: BindingErrorCode::ReservedBySystem,
            previous_binding: "ctrl+shift+space".to_string(),
            detail: Some("cmd+q".to_string()),
        };

        assert_eq!(
            serde_json::to_value(&error).unwrap(),
            serde_json::json!({
                "code": "reservedBySystem",
                "previousBinding": "ctrl+shift+space",
                "detail": "cmd+q"
            })
        );
    }

    #[test]
    fn every_rejection_maps_to_a_code() {
        let cases = [
            (ShortcutRejection::Empty, BindingErrorCode::Empty, None),
            (
                ShortcutRejection::Unparseable {
                    detail: "Unknown key: banane".to_string(),
                },
                BindingErrorCode::Unparseable,
                Some("Unknown key: banane"),
            ),
            (ShortcutRejection::NoKey, BindingErrorCode::NoKey, None),
            (
                ShortcutRejection::MultipleKeys,
                BindingErrorCode::MultipleKeys,
                None,
            ),
            (
                ShortcutRejection::NoModifier,
                BindingErrorCode::NoModifier,
                None,
            ),
            (
                ShortcutRejection::ShiftOnlyWithPrintable,
                BindingErrorCode::ShiftOnlyWithPrintable,
                None,
            ),
            (
                ShortcutRejection::EscapeKey,
                BindingErrorCode::EscapeKey,
                None,
            ),
            (
                ShortcutRejection::ReservedBySystem {
                    combo: "cmd+q".to_string(),
                },
                BindingErrorCode::ReservedBySystem,
                Some("cmd+q"),
            ),
        ];

        for (rejection, expected_code, expected_detail) in cases {
            let error = BindingError::from_rejection(rejection.clone(), "ctrl+shift+space");
            assert_eq!(error.code, expected_code, "refus {rejection:?}");
            assert_eq!(
                error.detail.as_deref(),
                expected_detail,
                "refus {rejection:?}"
            );
            assert_eq!(error.previous_binding, "ctrl+shift+space");
        }
    }
}
