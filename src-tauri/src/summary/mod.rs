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

use crate::download::NetworkFailure;

/// Ressource téléchargée au premier résumé. Les deux ne viennent pas du même
/// hébergeur — le moteur de github.com, le modèle de huggingface.co — et un
/// réseau d'entreprise en filtre souvent une sans l'autre : dire laquelle a
/// échoué est la première information utile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum SummaryAsset {
    /// Le moteur llama.cpp (quelques dizaines de Mo).
    Engine,
    /// Le modèle GGUF (environ 2 Go).
    Model,
}

impl SummaryAsset {
    /// Nom français, pour les journaux (« moteur », « modèle »). Le front a
    /// ses propres libellés (`settings.file.summary.errors.asset.*`).
    pub fn label(self) -> &'static str {
        match self {
            SummaryAsset::Engine => "moteur",
            SummaryAsset::Model => "modèle",
        }
    }
}

/// Erreurs remontées au front, qui les traduit (`settings.file.summary.errors.*`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum SummaryError {
    /// Le serveur n'a pas pu être joint : `failure` classe la panne (c'est ce
    /// qui décide du conseil affiché), `asset` et `host` disent quoi et où, et
    /// `detail` porte la chaîne de causes — affichée repliée, et copiable,
    /// pour qu'une capture d'écran du client soit exploitable par le support.
    Network {
        asset: SummaryAsset,
        failure: NetworkFailure,
        host: String,
        detail: String,
    },
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
    /// Le serveur a répondu, mais le fichier n'est pas arrivé complet (statut
    /// HTTP inattendu, flux coupé, taille incohérente, écriture impossible).
    DownloadFailed {
        asset: SummaryAsset,
        host: String,
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
            SummaryError::Network {
                asset,
                failure,
                host,
                detail,
            } => write!(
                f,
                "téléchargement du {} depuis {host} impossible — {} : {detail}",
                asset.label(),
                failure.label()
            ),
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
            SummaryError::DownloadFailed {
                asset,
                host,
                detail,
            } => write!(
                f,
                "téléchargement du {} depuis {host} échoué: {detail}",
                asset.label()
            ),
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
    /// Démarrage du serveur (chargement du modèle en mémoire) ; 0/1 puis 1/1.
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
        // Une panne réseau porte tout ce qu'il faut au front pour écrire un
        // message actionnable — et au support pour lire une capture d'écran.
        let network = SummaryError::Network {
            asset: SummaryAsset::Model,
            failure: NetworkFailure::DnsFailed,
            host: "huggingface.co".into(),
            detail: "error sending request → no such host".into(),
        };
        assert_eq!(
            serde_json::to_value(&network).unwrap(),
            serde_json::json!({
                "kind": "network",
                "asset": "model",
                "failure": "dnsFailed",
                "host": "huggingface.co",
                "detail": "error sending request → no such host",
            })
        );
        assert!(
            network.to_string().contains("no such host"),
            "le journal de l'application passe par Display: {network}"
        );
        assert_eq!(
            serde_json::to_value(SummaryError::DownloadFailed {
                asset: SummaryAsset::Engine,
                host: "github.com".into(),
                detail: "HTTP 403 Forbidden".into(),
            })
            .unwrap(),
            serde_json::json!({
                "kind": "downloadFailed",
                "asset": "engine",
                "host": "github.com",
                "detail": "HTTP 403 Forbidden",
            })
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
