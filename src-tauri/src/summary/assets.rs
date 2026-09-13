//! Assets épinglés (vérifiés le 12 septembre 2026) : build llama.cpp b10930 et
//! modèle Ministral 3 3B Instruct 2512 (GGUF Q4_K_M). Constantes uniquement ;
//! aucune valeur n'est lue depuis les réglages.

/// Build llama.cpp épinglé ; les archives contiennent un dossier racine
/// `llama-<build>/` (tar.gz) ou sont à plat (zip Windows).
pub const LLAMA_BUILD: &str = "b10930";

#[cfg(test)]
const LLAMA_RELEASE_URL: &str = "https://github.com/ggml-org/llama.cpp/releases/download/b10930";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveKind {
    TarGz,
    Zip,
}

#[derive(Debug, Clone, Copy)]
pub struct EngineAsset {
    pub file_name: &'static str,
    pub url: &'static str,
    pub sha256: &'static str,
    pub size_bytes: u64,
    pub kind: ArchiveKind,
    /// Nom de l'exécutable une fois extrait dans `<bin>/llama-<build>/`.
    pub exe_name: &'static str,
}

pub const ENGINE_MACOS_ARM64: EngineAsset = EngineAsset {
    file_name: "llama-b10930-bin-macos-arm64.tar.gz",
    url: "https://github.com/ggml-org/llama.cpp/releases/download/b10930/llama-b10930-bin-macos-arm64.tar.gz",
    sha256: "0f3f18c106841b11fab65b039b2b7e05d83c74e3063b4de631b13c13ea4efc19",
    size_bytes: 11_155_557,
    kind: ArchiveKind::TarGz,
    exe_name: "llama-server",
};

pub const ENGINE_MACOS_X64: EngineAsset = EngineAsset {
    file_name: "llama-b10930-bin-macos-x64.tar.gz",
    url: "https://github.com/ggml-org/llama.cpp/releases/download/b10930/llama-b10930-bin-macos-x64.tar.gz",
    sha256: "eb9104c2b9d005e3e08dd79abc57417c8e20e8cecfc4435a946d204b2e289d5c",
    size_bytes: 11_200_719,
    kind: ArchiveKind::TarGz,
    exe_name: "llama-server",
};

pub const ENGINE_LINUX_X64: EngineAsset = EngineAsset {
    file_name: "llama-b10930-bin-ubuntu-x64.tar.gz",
    url: "https://github.com/ggml-org/llama.cpp/releases/download/b10930/llama-b10930-bin-ubuntu-x64.tar.gz",
    sha256: "f86ff7e5efe61e53a14663c91e2afc14c0bd74052778484369f0c6bbf5fce136",
    size_bytes: 16_817_743,
    kind: ArchiveKind::TarGz,
    exe_name: "llama-server",
};

pub const ENGINE_WINDOWS_X64: EngineAsset = EngineAsset {
    file_name: "llama-b10930-bin-win-cpu-x64.zip",
    url: "https://github.com/ggml-org/llama.cpp/releases/download/b10930/llama-b10930-bin-win-cpu-x64.zip",
    sha256: "a0c1bf04e7b7b4b6c7f280b6bef08ecaa170f2ba611830b2332bce9ddf352dab",
    size_bytes: 18_429_489,
    kind: ArchiveKind::Zip,
    exe_name: "llama-server.exe",
};

/// Asset du moteur pour un couple OS/architecture (valeurs de
/// `std::env::consts::{OS, ARCH}`). Windows ARM utilise le build x64 CPU via
/// l'émulation, comme ffmpeg ; Linux ARM n'a pas de build (ni d'AppImage).
pub fn engine_asset_for(os: &str, arch: &str) -> Option<&'static EngineAsset> {
    match (os, arch) {
        ("macos", "aarch64") => Some(&ENGINE_MACOS_ARM64),
        ("macos", "x86_64") => Some(&ENGINE_MACOS_X64),
        ("linux", "x86_64") => Some(&ENGINE_LINUX_X64),
        ("windows", _) => Some(&ENGINE_WINDOWS_X64),
        _ => None,
    }
}

/// Asset du moteur pour la plateforme courante.
pub fn engine_asset() -> Option<&'static EngineAsset> {
    engine_asset_for(std::env::consts::OS, std::env::consts::ARCH)
}

/// La version de macOS donnée (« 14.5 », « 13.3.1 », « 26.0 ») permet-elle de
/// lancer le moteur ? Les binaires llama.cpp b10930 épinglés ci-dessus (arm64
/// comme x64) portent `LC_BUILD_VERSION minos 13.3`, alors que l'application
/// démarre dès macOS 10.15. `None` quand la chaîne n'est pas analysable : la
/// garde est alors ignorée plutôt que bloquante.
#[cfg(target_os = "macos")]
pub(crate) fn macos_version_supports_engine(version: &str) -> Option<bool> {
    let mut parts = version.trim().split('.');
    let major: u32 = parts.next()?.trim().parse().ok()?;
    let minor: u32 = match parts.next() {
        Some(minor) => minor.trim().parse().ok()?,
        None => 0,
    };
    Some(major > 13 || (major == 13 && minor >= 3))
}

#[derive(Debug, Clone, Copy)]
pub struct ModelAsset {
    pub file_name: &'static str,
    pub url: &'static str,
    pub sha256: &'static str,
    pub size_bytes: u64,
    /// Nom passé dans le champ `model` des requêtes (informatif pour llama-server).
    pub model_name: &'static str,
    /// Arguments supplémentaires de `llama-server` propres au modèle
    /// (ex. `--reasoning-budget 0` pour un modèle à réflexion) ; vide pour Ministral.
    pub server_args: &'static [&'static str],
}

/// Modèle retenu par défaut (tâche d'essai §9 : figé après comparaison).
pub const MODEL: ModelAsset = ModelAsset {
    file_name: "Ministral-3-3B-Instruct-2512-Q4_K_M.gguf",
    url: "https://huggingface.co/unsloth/Ministral-3-3B-Instruct-2512-GGUF/resolve/main/Ministral-3-3B-Instruct-2512-Q4_K_M.gguf",
    sha256: "fd46fc371ff0509bfa8657ac956b7de8534d7d9baaa4947975c0648c3aa397f4",
    size_bytes: 2_146_497_824,
    model_name: "ministral-3-3b-instruct-2512",
    server_args: &[],
};

/// Taille de contexte demandée au serveur (tokens).
///
/// 12 288 plutôt que 16 384 : une tranche fait au plus 9 000 tokens et la
/// sortie au plus 1 800, soit 10 800 tokens au pire — largement sous 12 288 —
/// et la valeur basse économise environ 0,5 Go de cache KV sur PC.
pub const CTX_SIZE: u32 = 12_288;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selects_the_expected_asset_per_platform() {
        assert_eq!(
            engine_asset_for("macos", "aarch64").unwrap().file_name,
            "llama-b10930-bin-macos-arm64.tar.gz"
        );
        assert_eq!(
            engine_asset_for("macos", "x86_64").unwrap().file_name,
            "llama-b10930-bin-macos-x64.tar.gz"
        );
        assert_eq!(
            engine_asset_for("linux", "x86_64").unwrap().file_name,
            "llama-b10930-bin-ubuntu-x64.tar.gz"
        );
        let windows = engine_asset_for("windows", "x86_64").unwrap();
        assert_eq!(windows.kind, ArchiveKind::Zip);
        assert_eq!(windows.exe_name, "llama-server.exe");
        assert_eq!(
            engine_asset_for("windows", "aarch64").unwrap().file_name,
            windows.file_name
        );
        assert!(engine_asset_for("linux", "aarch64").is_none());
    }

    #[test]
    fn urls_and_hashes_are_well_formed() {
        for asset in [
            &ENGINE_MACOS_ARM64,
            &ENGINE_MACOS_X64,
            &ENGINE_LINUX_X64,
            &ENGINE_WINDOWS_X64,
        ] {
            assert_eq!(
                asset.url,
                format!("{LLAMA_RELEASE_URL}/{}", asset.file_name)
            );
            assert_eq!(asset.sha256.len(), 64);
            assert!(asset.sha256.chars().all(|c| c.is_ascii_hexdigit()));
            assert!(asset.file_name.contains(LLAMA_BUILD));
        }
        assert_eq!(MODEL.sha256.len(), 64);
        assert!(MODEL.url.ends_with(MODEL.file_name));
        assert!(
            MODEL.size_bytes < 2 * 1024 * 1024 * 1024,
            "Q4_K_M tient sous 2 Gio (miroir GitHub possible)"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_version_guard_matches_the_pinned_build() {
        assert_eq!(macos_version_supports_engine("12.7.6"), Some(false));
        assert_eq!(macos_version_supports_engine("13.2"), Some(false));
        assert_eq!(macos_version_supports_engine("13.3"), Some(true));
        assert_eq!(macos_version_supports_engine("13.3.1"), Some(true));
        assert_eq!(macos_version_supports_engine("14.5"), Some(true));
        assert_eq!(macos_version_supports_engine("26.0"), Some(true));
        assert_eq!(macos_version_supports_engine("abc"), None);
    }

    #[test]
    fn current_platform_has_an_asset_on_supported_targets() {
        if cfg!(any(
            target_os = "macos",
            target_os = "windows",
            all(target_os = "linux", target_arch = "x86_64")
        )) {
            assert!(engine_asset().is_some());
        }
    }
}
