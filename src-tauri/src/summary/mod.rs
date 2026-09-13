//! Compte-rendu local d'un enregistrement (plan 09) : sidecar llama.cpp +
//! modèle GGUF téléchargés au premier usage, résumé en français selon un
//! gabarit unique. Aucune donnée ne quitte l'ordinateur.

pub mod assets;
pub mod chunking;
pub mod commands;
pub mod engine;
pub mod install;
pub mod prompt;
pub mod server;
pub mod summarize;
pub mod system;

use serde::{Deserialize, Serialize};
use specta::Type;

/// Erreurs remontées au front, qui les traduit (`settings.file.summary.errors.*`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum SummaryError {
    /// Téléchargement impossible faute de réseau (rien n'a été reçu).
    Offline,
    DiskSpace {
        #[serde(rename = "neededBytes")]
        needed_bytes: u64,
        #[serde(rename = "freeBytes")]
        free_bytes: u64,
    },
    Memory {
        #[serde(rename = "neededBytes")]
        needed_bytes: u64,
        #[serde(rename = "freeBytes")]
        free_bytes: u64,
    },
    DownloadFailed {
        detail: String,
    },
    ChecksumMismatch,
    EngineStartFailed {
        detail: String,
    },
    IncompleteOutput,
    Cancelled,
    /// Un résumé est déjà en cours.
    Busy,
}

impl std::fmt::Display for SummaryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SummaryError::Offline => write!(f, "hors ligne"),
            SummaryError::DiskSpace {
                needed_bytes,
                free_bytes,
            } => {
                write!(
                    f,
                    "espace disque insuffisant ({free_bytes} libres, {needed_bytes} requis)"
                )
            }
            SummaryError::Memory {
                needed_bytes,
                free_bytes,
            } => {
                write!(
                    f,
                    "mémoire insuffisante ({free_bytes} disponibles, {needed_bytes} requis)"
                )
            }
            SummaryError::DownloadFailed { detail } => write!(f, "téléchargement échoué: {detail}"),
            SummaryError::ChecksumMismatch => write!(f, "fichier téléchargé corrompu"),
            SummaryError::EngineStartFailed { detail } => write!(f, "moteur non démarré: {detail}"),
            SummaryError::IncompleteOutput => write!(f, "compte-rendu incomplet"),
            SummaryError::Cancelled => write!(f, "annulé"),
            SummaryError::Busy => write!(f, "un résumé est déjà en cours"),
        }
    }
}

impl std::error::Error for SummaryError {}

/// Phase en cours, pour l'encart de progression de l'onglet Fichier.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "lowercase")]
pub enum SummaryPhase {
    /// Téléchargement du moteur ; `current` = pourcentage (0-100), `total` = 100.
    Engine,
    /// Téléchargement du modèle ; `current` = pourcentage (0-100), `total` = 100.
    Model,
    /// Démarrage du serveur (chargement du modèle en mémoire) ; 0/0.
    Starting,
    /// Résumé ; `current` = partie en cours, `total` = nombre de parties.
    Summarizing,
}

/// Événement `summary-progress`, émis à chaque étape (téléchargements,
/// démarrage, tranche en cours).
#[derive(Clone, Debug, Serialize, Deserialize, Type, tauri_specta::Event)]
pub struct SummaryProgress {
    pub phase: SummaryPhase,
    pub current: u32,
    pub total: u32,
}

/// État des fichiers nécessaires, pour annoncer le téléchargement unique.
#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SummaryStatus {
    pub engine_ready: bool,
    pub model_ready: bool,
    /// Octets restant à télécharger (0 si tout est installé).
    pub download_size_bytes: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_serialize_with_camel_case_tag_and_fields() {
        let json = serde_json::to_value(SummaryError::DiskSpace {
            needed_bytes: 3,
            free_bytes: 1,
        })
        .unwrap();
        assert_eq!(
            json,
            serde_json::json!({ "kind": "diskSpace", "neededBytes": 3, "freeBytes": 1 })
        );
        assert_eq!(
            serde_json::to_value(SummaryError::Offline).unwrap(),
            serde_json::json!({ "kind": "offline" })
        );
        assert_eq!(
            serde_json::to_value(SummaryError::EngineStartFailed { detail: "x".into() }).unwrap(),
            serde_json::json!({ "kind": "engineStartFailed", "detail": "x" })
        );
        assert_eq!(
            serde_json::to_value(SummaryPhase::Summarizing).unwrap(),
            serde_json::json!("summarizing")
        );
        let status = SummaryStatus {
            engine_ready: true,
            model_ready: false,
            download_size_bytes: 5,
        };
        assert_eq!(
            serde_json::to_value(status).unwrap(),
            serde_json::json!({ "engineReady": true, "modelReady": false, "downloadSizeBytes": 5 })
        );
    }

    #[test]
    fn types_implement_specta_type() {
        fn assert_type<T: specta::Type>() {}
        assert_type::<SummaryError>();
        assert_type::<SummaryProgress>();
        assert_type::<SummaryStatus>();
    }
}
