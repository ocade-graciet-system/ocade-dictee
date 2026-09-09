//! Tray menu internationalization
//!
//! Everything is auto-generated at compile time by build.rs from the
//! frontend locale files (src/i18n/locales/*/translation.json).
//!
//! The French translation.json is the single source of truth:
//! - TrayStrings struct fields are derived from the French "tray" keys
//! - All languages are auto-discovered from the locales directory
//!
//! To add a new tray menu item:
//! 1. Add the key to fr/translation.json under "tray"
//! 2. Add translations to other locale files
//! 3. Update tray.rs to use the new field (e.g., strings.new_field)

use once_cell::sync::Lazy;
use std::collections::HashMap;

// Include the auto-generated TrayStrings struct and TRANSLATIONS static
include!(concat!(env!("OUT_DIR"), "/tray_translations.rs"));

/// Chaînes du menu tray. Une seule locale est embarquée (fr) : tout paramètre
/// se résout sur elle.
pub fn get_tray_translations(locale: Option<String>) -> TrayStrings {
    let locale_str = locale.as_deref().unwrap_or("fr");
    let lang_code = locale_str.split(['-', '_']).next().unwrap_or("fr");

    TRANSLATIONS
        .get(locale_str)
        .or_else(|| TRANSLATIONS.get(lang_code))
        .or_else(|| TRANSLATIONS.get("fr"))
        .cloned()
        .expect("French tray translations must exist")
}
