//! Empreinte du poste : condensé de l'identifiant machine du système.
//!
//! Stable après une réinstallation de l'application (elle ne dépend que du
//! système) ; elle change si le système lui-même est réinstallé, auquel cas le
//! client libère l'ancien poste. L'identifiant brut ne quitte jamais le poste :
//! seul son condensé SHA-256, salé par un préfixe propre à l'application, est
//! envoyé au serveur.

use sha2::{Digest, Sha256};
use std::sync::OnceLock;

/// Empreinte hexadécimale (64 caractères) dérivée d'un identifiant machine.
pub fn from_machine_id(machine_id: &str) -> String {
    hex::encode(Sha256::digest(format!(
        "ocade-dictee/v1:{}",
        machine_id.trim()
    )))
}

/// Empreinte de ce poste, calculée une fois. `None` si le système ne fournit
/// aucun identifiant (cas anormal) : l'activation est alors impossible.
pub fn current() -> Option<String> {
    static CACHE: OnceLock<Option<String>> = OnceLock::new();
    CACHE
        .get_or_init(|| match machine_id() {
            Some(id) if !id.trim().is_empty() => Some(from_machine_id(&id)),
            _ => {
                log::error!("Identifiant machine introuvable : empreinte de licence indisponible");
                None
            }
        })
        .clone()
}

#[cfg(target_os = "windows")]
fn machine_id() -> Option<String> {
    use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_64KEY};
    // Vue 64 bits forcée : la clé n'existe que là sur un Windows 64 bits.
    winreg::RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey_with_flags(
            "SOFTWARE\\Microsoft\\Cryptography",
            KEY_READ | KEY_WOW64_64KEY,
        )
        .and_then(|key| key.get_value::<String, _>("MachineGuid"))
        .ok()
}

#[cfg(target_os = "macos")]
fn machine_id() -> Option<String> {
    let output = std::process::Command::new("/usr/sbin/ioreg")
        .args(["-rd1", "-c", "IOPlatformExpertDevice"])
        .output()
        .ok()?;
    parse_ioreg_uuid(&String::from_utf8_lossy(&output.stdout))
}

#[cfg(target_os = "linux")]
fn machine_id() -> Option<String> {
    ["/etc/machine-id", "/var/lib/dbus/machine-id"]
        .iter()
        .find_map(|path| std::fs::read_to_string(path).ok())
        .map(|s| s.trim().to_string())
}

#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
fn machine_id() -> Option<String> {
    None
}

/// Extrait `IOPlatformUUID` de la sortie de `ioreg` (macOS).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn parse_ioreg_uuid(ioreg_output: &str) -> Option<String> {
    ioreg_output.lines().find_map(|line| {
        let (_, rest) = line.split_once("\"IOPlatformUUID\"")?;
        let value = rest.split('"').nth(1)?;
        (!value.is_empty()).then(|| value.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_is_stable_salted_and_hex() {
        let a = from_machine_id("1234-ABCD");
        assert_eq!(a, from_machine_id("  1234-ABCD\n"));
        assert_eq!(a.len(), 64);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, from_machine_id("1234-ABCE"));
        // Le sel empêche de retrouver l'identifiant par un simple SHA-256.
        assert_ne!(a, hex::encode(Sha256::digest("1234-ABCD")));
    }

    #[test]
    fn ioreg_uuid_is_extracted() {
        let sample = r#"+-o J316sAP  <class IOPlatformExpertDevice, id 0x100000202>
    {
      "IOPlatformSerialNumber" = "C02XXXXX"
      "IOPlatformUUID" = "5A1B2C3D-0000-1111-2222-333344445555"
    }"#;
        assert_eq!(
            parse_ioreg_uuid(sample).as_deref(),
            Some("5A1B2C3D-0000-1111-2222-333344445555")
        );
        assert_eq!(parse_ioreg_uuid("rien"), None);
    }

    #[test]
    fn this_machine_has_a_fingerprint() {
        // Windows (CI et poste de dev) et macOS fournissent un identifiant.
        if cfg!(any(target_os = "windows", target_os = "macos")) {
            assert!(current().is_some());
        }
    }
}
