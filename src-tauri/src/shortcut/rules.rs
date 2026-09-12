//! Règles d'acceptation d'un raccourci de dictée personnalisé (spec 08 §3).
//!
//! Source unique de vérité : le backend. Le front n'affiche que la traduction
//! du code de refus produit ici — il ne redécide jamais si une combinaison est
//! acceptable.
//!
//! Le module tokenise lui-même la chaîne sur `+` en réutilisant les parseurs
//! publics de handy-keys (`Modifiers: FromStr`, `Key: FromStr`) : `Hotkey`
//! refuse « plusieurs touches » dès son `FromStr`, ce qui empêcherait de
//! distinguer `MultipleKeys` de `Unparseable`. La confirmation finale reste
//! `raw.parse::<Hotkey>()`, pour ne jamais accepter une chaîne que
//! l'enregistrement ne saurait pas relire.

use handy_keys::{Hotkey, Key, Modifiers};

/// Système d'exploitation visé par les règles. Passé explicitement (et non
/// déduit de `cfg!`) pour que les trois listes noires soient testables depuis
/// n'importe quel hôte — la CI `test` tourne sur Ubuntu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetOs {
    MacOs,
    Windows,
    Linux,
}

/// Raison pour laquelle un raccourci est refusé. Le front traduit la variante
/// (`settings.general.shortcut.rejections.*`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShortcutRejection {
    /// Chaîne vide après trim.
    Empty,
    /// Jeton inconnu de handy-keys (ni modificateur, ni touche).
    Unparseable { detail: String },
    /// Aucun modificateur parmi ctrl / alt-option / shift / cmd-super.
    NoModifier,
    /// Aucune touche non modificatrice (modificateurs seuls).
    NoKey,
    /// Plus d'une touche non modificatrice.
    MultipleKeys,
    /// Maj est le seul modificateur et la touche est imprimable : la frappe
    /// des majuscules serait capturée par le raccourci.
    ShiftOnlyWithPrintable,
    /// Échap est réservée à l'annulation de la dictée.
    EscapeKey,
    /// La combinaison normalisée figure dans la liste noire de l'OS.
    ReservedBySystem { combo: String },
}

/// Raccourcis que le système intercepte avant toute application (macOS).
const MACOS_RESERVED: [&str; 16] = [
    "cmd+q",
    "cmd+w",
    "cmd+h",
    "cmd+m",
    "cmd+tab",
    "cmd+space",
    "cmd+option+escape",
    "ctrl+cmd+q",
    "cmd+shift+3",
    "cmd+shift+4",
    "cmd+shift+5",
    "ctrl+up",
    "ctrl+down",
    "ctrl+left",
    "ctrl+right",
    "cmd+comma",
];

/// Idem sous Windows. `delete` et `forwarddelete` sont deux touches distinctes
/// pour handy-keys (retour arrière vs Suppr) : Ctrl+Alt+Suppr n'est couvert que
/// si les deux graphies figurent dans la liste.
const WINDOWS_RESERVED: [&str; 10] = [
    "ctrl+alt+delete",
    "ctrl+alt+forwarddelete",
    "win+l",
    "alt+tab",
    "alt+f4",
    "ctrl+escape",
    "ctrl+shift+escape",
    "win+d",
    "win+e",
    "win+tab",
];

/// Idem sous Linux (environnements de bureau courants).
const LINUX_RESERVED: [&str; 5] = [
    "ctrl+alt+t",
    "ctrl+alt+backspace",
    "alt+tab",
    "alt+f4",
    "super+l",
];

/// OS de la machine courante. `cfg!` plutôt que trois `#[cfg]` : les branches
/// mortes restent compilées, donc les trois variantes comptent comme
/// construites et l'analyse de code mort se tait sur chaque plateforme.
pub fn current_os() -> TargetOs {
    if cfg!(target_os = "macos") {
        TargetOs::MacOs
    } else if cfg!(target_os = "windows") {
        TargetOs::Windows
    } else {
        TargetOs::Linux
    }
}

/// Liste noire de l'OS visé.
fn reserved_list(os: TargetOs) -> &'static [&'static str] {
    match os {
        TargetOs::MacOs => &MACOS_RESERVED,
        TargetOs::Windows => &WINDOWS_RESERVED,
        TargetOs::Linux => &LINUX_RESERVED,
    }
}

/// Décomposition d'un raccourci : modificateurs cumulés, première touche non
/// modificatrice, et nombre total de touches non modificatrices rencontrées.
struct ParsedShortcut {
    modifiers: Modifiers,
    key: Option<Key>,
    key_count: usize,
}

/// Classe chaque jeton avec les parseurs de handy-keys : d'abord modificateur
/// (alias `option`/`alt`, `cmd`/`command`/`super`/`win`, variantes `_left`/
/// `_right` produites par la capture), sinon touche.
fn classify_tokens(raw: &str) -> Result<ParsedShortcut, ShortcutRejection> {
    let mut modifiers = Modifiers::empty();
    let mut key: Option<Key> = None;
    let mut key_count = 0usize;

    for token in raw.split('+') {
        let token = token.trim();
        if token.is_empty() {
            continue;
        }
        match token.parse::<Modifiers>() {
            Ok(m) if !m.is_empty() => modifiers |= m,
            _ => match token.parse::<Key>() {
                Ok(k) => {
                    key_count += 1;
                    if key.is_none() {
                        key = Some(k);
                    }
                }
                Err(e) => {
                    return Err(ShortcutRejection::Unparseable {
                        detail: e.to_string(),
                    })
                }
            },
        }
    }

    Ok(ParsedShortcut {
        modifiers,
        key,
        key_count,
    })
}

/// Forme normalisée servant de clé de comparaison avec les listes noires :
/// modificateurs dé-latéralisés, dans un ordre fixe, sous un nom unique
/// (`alt` pour option/alt, `cmd` pour command/super/win), puis la touche dans
/// la graphie de handy-keys en minuscules (`left`, `return`, `,`, `3`…).
fn canonical_form(modifiers: Modifiers, key: Key) -> String {
    let mut parts: Vec<&str> = Vec::new();
    if modifiers.intersects(Modifiers::CTRL) {
        parts.push("ctrl");
    }
    if modifiers.intersects(Modifiers::OPT) {
        parts.push("alt");
    }
    if modifiers.intersects(Modifiers::SHIFT) {
        parts.push("shift");
    }
    if modifiers.intersects(Modifiers::CMD) {
        parts.push("cmd");
    }
    if modifiers.contains(Modifiers::FN) {
        parts.push("fn");
    }

    let key_name = key.to_string().to_lowercase();
    if parts.is_empty() {
        return key_name;
    }
    format!("{}+{}", parts.join("+"), key_name)
}

/// Forme normalisée d'une entrée de liste noire. Chaîne vide si l'entrée est
/// inexploitable (faute de frappe) — le test `blacklist_entries_all_canonicalise`
/// interdit ce cas.
fn canonical_of(raw: &str) -> String {
    match classify_tokens(raw) {
        Ok(parsed) => match parsed.key {
            Some(key) => canonical_form(parsed.modifiers, key),
            None => String::new(),
        },
        Err(_) => String::new(),
    }
}

/// Touche produisant un caractère à l'écran : lettre, chiffre, ponctuation,
/// Espace, et les touches imprimables du pavé numérique (spec §3, règle 6).
/// Les touches de fonction, de navigation et d'action (Entrée, Suppr, Clear…)
/// n'en font pas partie.
fn is_printable(key: Key) -> bool {
    matches!(
        key,
        Key::A
            | Key::B
            | Key::C
            | Key::D
            | Key::E
            | Key::F
            | Key::G
            | Key::H
            | Key::I
            | Key::J
            | Key::K
            | Key::L
            | Key::M
            | Key::N
            | Key::O
            | Key::P
            | Key::Q
            | Key::R
            | Key::S
            | Key::T
            | Key::U
            | Key::V
            | Key::W
            | Key::X
            | Key::Y
            | Key::Z
            | Key::Num0
            | Key::Num1
            | Key::Num2
            | Key::Num3
            | Key::Num4
            | Key::Num5
            | Key::Num6
            | Key::Num7
            | Key::Num8
            | Key::Num9
            | Key::Minus
            | Key::Equal
            | Key::LeftBracket
            | Key::RightBracket
            | Key::Backslash
            | Key::Semicolon
            | Key::Quote
            | Key::Comma
            | Key::Period
            | Key::Slash
            | Key::Grave
            | Key::Section
            | Key::JisYen
            | Key::JisUnderscore
            | Key::Space
            | Key::Keypad0
            | Key::Keypad1
            | Key::Keypad2
            | Key::Keypad3
            | Key::Keypad4
            | Key::Keypad5
            | Key::Keypad6
            | Key::Keypad7
            | Key::Keypad8
            | Key::Keypad9
            | Key::KeypadDecimal
            | Key::KeypadComma
            | Key::KeypadPlus
            | Key::KeypadMinus
            | Key::KeypadMultiply
            | Key::KeypadDivide
            | Key::KeypadEquals
    )
}

/// Applique les 8 contrôles de la spec §3, dans l'ordre : le premier refus
/// l'emporte.
pub fn validate_custom_shortcut(raw: &str, os: TargetOs) -> Result<(), ShortcutRejection> {
    // 1. Chaîne vide.
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(ShortcutRejection::Empty);
    }

    // 2. Jeton inconnu de handy-keys.
    let parsed = classify_tokens(trimmed)?;

    // 3. Modificateurs seuls.
    let Some(key) = parsed.key else {
        return Err(ShortcutRejection::NoKey);
    };

    // 4. Plusieurs touches.
    if parsed.key_count > 1 {
        return Err(ShortcutRejection::MultipleKeys);
    }

    // 5. Aucun modificateur (`fn` seul ne compte pas).
    let real_modifiers = Modifiers::CTRL | Modifiers::OPT | Modifiers::SHIFT | Modifiers::CMD;
    if !parsed.modifiers.intersects(real_modifiers) {
        return Err(ShortcutRejection::NoModifier);
    }

    // 6. Maj seul + caractère imprimable.
    let shift_only = parsed.modifiers.intersects(Modifiers::SHIFT)
        && !parsed
            .modifiers
            .intersects(Modifiers::CTRL | Modifiers::OPT | Modifiers::CMD);
    if shift_only && is_printable(key) {
        return Err(ShortcutRejection::ShiftOnlyWithPrintable);
    }

    // 7. Échap, quels que soient les modificateurs.
    if key == Key::Escape {
        return Err(ShortcutRejection::EscapeKey);
    }

    // 8. Combinaison réservée par l'OS.
    let combo = canonical_form(parsed.modifiers, key);
    if reserved_list(os)
        .iter()
        .any(|entry| canonical_of(entry) == combo)
    {
        return Err(ShortcutRejection::ReservedBySystem { combo });
    }

    // Garde-fou : la chaîne acceptée sera persistée puis relue par handy-keys
    // à chaque démarrage — elle doit rester parsable telle quelle.
    trimmed
        .parse::<Hotkey>()
        .map(|_| ())
        .map_err(|e| ShortcutRejection::Unparseable {
            detail: e.to_string(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::SHORTCUT_PRESETS;

    const ALL_OS: [TargetOs; 3] = [TargetOs::MacOs, TargetOs::Windows, TargetOs::Linux];

    /// Graphie des 4 préréglages sur chaque OS (`settings.rs`, cfg par plateforme).
    const PRESETS_MACOS: [&str; 4] = [
        "ctrl+option+space",
        "ctrl+shift+space",
        "ctrl+option+d",
        "ctrl+shift+d",
    ];
    const PRESETS_OTHER: [&str; 4] = [
        "ctrl+alt+space",
        "ctrl+shift+space",
        "ctrl+alt+d",
        "ctrl+shift+d",
    ];

    #[test]
    fn one_case_per_rejection_variant() {
        // (raccourci, OS, refus attendu) — un cas au moins par variante,
        // dans l'ordre des contrôles de la spec §3.
        let cases: [(&str, TargetOs, ShortcutRejection); 14] = [
            ("", TargetOs::MacOs, ShortcutRejection::Empty),
            ("   ", TargetOs::Linux, ShortcutRejection::Empty),
            ("ctrl+option", TargetOs::MacOs, ShortcutRejection::NoKey),
            ("shift", TargetOs::Windows, ShortcutRejection::NoKey),
            ("ctrl+a+b", TargetOs::MacOs, ShortcutRejection::MultipleKeys),
            ("f13", TargetOs::MacOs, ShortcutRejection::NoModifier),
            ("space", TargetOs::Windows, ShortcutRejection::NoModifier),
            // `fn` seul n'est pas un modificateur au sens de la règle 5.
            ("fn+d", TargetOs::MacOs, ShortcutRejection::NoModifier),
            (
                "shift+a",
                TargetOs::MacOs,
                ShortcutRejection::ShiftOnlyWithPrintable,
            ),
            (
                "shift+1",
                TargetOs::Windows,
                ShortcutRejection::ShiftOnlyWithPrintable,
            ),
            (
                "shift+comma",
                TargetOs::Linux,
                ShortcutRejection::ShiftOnlyWithPrintable,
            ),
            // La règle Échap passe AVANT la liste noire : `cmd+option+escape`
            // et `ctrl+shift+escape` y figurent mais sortent en `EscapeKey`.
            (
                "cmd+option+escape",
                TargetOs::MacOs,
                ShortcutRejection::EscapeKey,
            ),
            (
                "ctrl+shift+escape",
                TargetOs::Windows,
                ShortcutRejection::EscapeKey,
            ),
            (
                "cmd+q",
                TargetOs::MacOs,
                ShortcutRejection::ReservedBySystem {
                    combo: "cmd+q".to_string(),
                },
            ),
        ];

        for (raw, os, expected) in cases {
            assert_eq!(
                validate_custom_shortcut(raw, os),
                Err(expected.clone()),
                "raccourci « {raw} » sur {os:?}"
            );
        }
    }

    #[test]
    fn unparseable_carries_a_detail() {
        let err = validate_custom_shortcut("ctrl+option+banane", TargetOs::MacOs).unwrap_err();
        match err {
            ShortcutRejection::Unparseable { detail } => {
                assert!(detail.contains("banane"), "détail inattendu : {detail}");
            }
            other => panic!("attendu Unparseable, obtenu {other:?}"),
        }
    }

    #[test]
    fn accepts_usable_combinations_on_every_os() {
        for os in ALL_OS {
            for raw in [
                "ctrl+option+d",
                "cmd+shift+space",
                "ctrl+f8",
                "ctrl+alt+enter",
                "shift+f5",
                // Modificateurs latéralisés produits par la capture handy-keys.
                "ctrl_left+option_left+d",
            ] {
                assert_eq!(
                    validate_custom_shortcut(raw, os),
                    Ok(()),
                    "raccourci « {raw} » refusé sur {os:?}"
                );
            }
        }
    }

    #[test]
    fn the_four_presets_pass_on_every_os() {
        for os in ALL_OS {
            for preset in PRESETS_MACOS.iter().chain(PRESETS_OTHER.iter()) {
                assert_eq!(
                    validate_custom_shortcut(preset, os),
                    Ok(()),
                    "préréglage « {preset} » refusé sur {os:?}"
                );
            }
        }
    }

    #[test]
    fn preset_table_matches_the_crate_constants() {
        let expected = if cfg!(target_os = "macos") {
            PRESETS_MACOS
        } else {
            PRESETS_OTHER
        };
        assert_eq!(SHORTCUT_PRESETS, expected);
    }

    #[test]
    fn blacklists_are_per_os() {
        // `alt+tab` est réservé sous Windows et Linux, pas sous macOS.
        assert!(validate_custom_shortcut("alt+tab", TargetOs::Windows).is_err());
        assert!(validate_custom_shortcut("alt+tab", TargetOs::Linux).is_err());
        assert_eq!(validate_custom_shortcut("alt+tab", TargetOs::MacOs), Ok(()));

        // `cmd+q` n'est réservé que sous macOS.
        assert_eq!(validate_custom_shortcut("cmd+q", TargetOs::Windows), Ok(()));

        // `win+l` (Windows) et `super+l` (Linux) désignent la même combinaison.
        assert!(validate_custom_shortcut("win+l", TargetOs::Windows).is_err());
        assert!(validate_custom_shortcut("super+l", TargetOs::Linux).is_err());
    }

    #[test]
    fn blacklist_entries_all_canonicalise() {
        // Garde-fou : une faute de frappe dans une liste noire la rendrait
        // inopérante sans que rien ne le signale.
        for os in ALL_OS {
            for entry in reserved_list(os) {
                assert!(
                    !canonical_of(entry).is_empty(),
                    "entrée « {entry} » de la liste {os:?} non analysable"
                );
                // L'entrée doit se refuser elle-même (ou sortir plus tôt
                // sur la règle Échap, qui la précède).
                assert!(
                    validate_custom_shortcut(entry, os).is_err(),
                    "entrée « {entry} » de la liste {os:?} acceptée"
                );
            }
        }
    }

    #[test]
    fn modifier_order_and_aliases_do_not_matter() {
        // `option`/`alt` et `cmd`/`command`/`super`/`win` sont des alias ;
        // l'ordre des modificateurs est normalisé avant comparaison.
        assert_eq!(
            validate_custom_shortcut("option+cmd+escape", TargetOs::MacOs),
            Err(ShortcutRejection::EscapeKey)
        );
        assert_eq!(
            validate_custom_shortcut("shift+cmd+3", TargetOs::MacOs),
            Err(ShortcutRejection::ReservedBySystem {
                combo: "shift+cmd+3".to_string()
            })
        );
        assert_eq!(
            validate_custom_shortcut("command+comma", TargetOs::MacOs),
            Err(ShortcutRejection::ReservedBySystem {
                combo: "cmd+,".to_string()
            })
        );
    }

    #[test]
    fn current_os_matches_the_host() {
        let expected = if cfg!(target_os = "macos") {
            TargetOs::MacOs
        } else if cfg!(target_os = "windows") {
            TargetOs::Windows
        } else {
            TargetOs::Linux
        };
        assert_eq!(current_os(), expected);
    }
    #[test]
    fn windows_reserves_ctrl_alt_suppr_whatever_its_spelling() {
        // handy-keys mappe « delete »/« backspace » sur Key::Delete (retour
        // arrière) et « del »/« forwarddelete » sur Key::ForwardDelete (Suppr) :
        // la liste noire doit couvrir les deux graphies, sinon Ctrl+Alt+Suppr
        // passe sous le nom « ctrl+alt+del ».
        for (raw, combo) in [
            ("ctrl+alt+del", "ctrl+alt+forwarddelete"),
            ("ctrl+alt+forwarddelete", "ctrl+alt+forwarddelete"),
            ("ctrl+alt+delete", "ctrl+alt+delete"),
        ] {
            assert_eq!(
                validate_custom_shortcut(raw, TargetOs::Windows),
                Err(ShortcutRejection::ReservedBySystem {
                    combo: combo.to_string()
                }),
                "raccourci « {raw} » accepté sous Windows"
            );
        }
    }

    #[test]
    fn shift_only_also_covers_space_and_the_keypad() {
        // Espace et le pavé numérique produisent un caractère : Maj seul + ces
        // touches capturerait une frappe courante (règle 6).
        for raw in ["shift+space", "shift+keypad1"] {
            assert_eq!(
                validate_custom_shortcut(raw, TargetOs::MacOs),
                Err(ShortcutRejection::ShiftOnlyWithPrintable),
                "raccourci « {raw} » accepté"
            );
        }

        // Maj accompagné d'un autre modificateur reste valable (préréglage),
        // et une touche de fonction n'est pas imprimable.
        assert_eq!(
            validate_custom_shortcut("ctrl+shift+space", TargetOs::MacOs),
            Ok(())
        );
        assert_eq!(
            validate_custom_shortcut("shift+f5", TargetOs::MacOs),
            Ok(())
        );
    }
}
