//! Enregistrement de réunion : capture du micro écrite au fil de l'eau sur le
//! disque, sans limite de durée (une réunion de 3 h, 10 h ou 24 h ne tient
//! jamais en mémoire), avec pause, reprise et reconnexion du micro. Les
//! enregistrements, leur transcription, leur compte rendu et ce qu'il applique
//! (charte éditoriale, charte graphique, skill) vivent dans `meetings.db` et le
//! dossier `meetings/` des données de l'application. Aucune donnée ne quitte
//! l'ordinateur.

pub mod assets;
pub mod commands;
pub mod keep_awake;
pub mod recorder;
pub mod report;
pub mod store;
pub mod wav;

use serde::{Deserialize, Serialize};
use specta::Type;

/// Phase de l'enregistreur.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum RecorderPhase {
    Idle,
    Recording,
    Paused,
    /// Le micro a cessé de répondre (débranché, casque Bluetooth éteint…) :
    /// l'enregistrement continue dès qu'un micro répond de nouveau.
    Reconnecting,
}

/// État courant de l'enregistreur, émis plusieurs fois par seconde pendant un
/// enregistrement (chronomètre, niveau du micro) et à chaque changement de
/// phase (événement `meeting-recorder-status`).
#[derive(Clone, Debug, Serialize, Deserialize, Type, tauri_specta::Event)]
#[serde(rename_all = "camelCase")]
pub struct MeetingRecorderStatus {
    pub phase: RecorderPhase,
    /// Enregistrement en cours (`None` au repos).
    pub recording_id: Option<i64>,
    /// Durée d'audio enregistrée, pauses exclues.
    pub duration_ms: i64,
    pub size_bytes: i64,
    /// Niveau du micro, de 0 (silence) à 1.
    pub level: f32,
    /// Nom du micro qui enregistre (`None` hors enregistrement).
    pub microphone: Option<String>,
}

impl MeetingRecorderStatus {
    pub fn idle() -> Self {
        Self {
            phase: RecorderPhase::Idle,
            recording_id: None,
            duration_ms: 0,
            size_bytes: 0,
            level: 0.0,
            microphone: None,
        }
    }
}

/// Pourquoi un enregistrement s'est terminé.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum FinishReason {
    /// « Terminer l'enregistrement ».
    User,
    /// Disque presque plein : arrêt avant d'écrire un fichier tronqué.
    DiskFull,
    /// Écriture impossible (disque retiré, droits…).
    WriteFailed,
    /// Fermeture de l'application pendant l'enregistrement.
    AppExit,
}

/// Événement `meeting-recording-finished` : l'enregistrement est terminé et
/// son fichier est complet.
#[derive(Clone, Debug, Serialize, Deserialize, Type, tauri_specta::Event)]
#[serde(rename_all = "camelCase")]
pub struct MeetingRecordingFinished {
    pub recording: MeetingRecording,
    pub reason: FinishReason,
}

/// État d'un enregistrement de la liste.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum RecordingState {
    InProgress,
    Done,
    /// L'application s'est arrêtée pendant l'enregistrement (plantage,
    /// extinction de l'ordinateur) : l'audio est conservé jusqu'à
    /// l'interruption.
    Interrupted,
}

impl RecordingState {
    pub fn as_db(self) -> &'static str {
        match self {
            RecordingState::InProgress => "in_progress",
            RecordingState::Done => "done",
            RecordingState::Interrupted => "interrupted",
        }
    }

    pub fn from_db(value: &str) -> Self {
        match value {
            "in_progress" => RecordingState::InProgress,
            "interrupted" => RecordingState::Interrupted,
            _ => RecordingState::Done,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct MeetingRecording {
    pub id: i64,
    pub name: String,
    /// Début de l'enregistrement, horodatage unix (secondes).
    pub created_at: i64,
    pub duration_ms: i64,
    pub size_bytes: i64,
    pub state: RecordingState,
    /// Nombre de fichiers WAV : 1, sauf au-delà de ~37 h d'audio (limite de
    /// 4 Go du format WAV), où l'enregistrement continue dans un fichier suivant.
    pub parts: i64,
    /// Un compte rendu a déjà été rédigé.
    pub has_report: bool,
}

/// Transcription et compte rendu d'un enregistrement.
#[derive(Clone, Debug, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MeetingReport {
    pub recording_id: i64,
    pub transcript: Option<String>,
    /// Compte rendu Markdown, `None` tant qu'il n'a pas été rédigé.
    pub markdown: Option<String>,
    pub updated_at: Option<i64>,
}

/// Élément texte du compte rendu : charte éditoriale ou skill.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum TextAssetKind {
    Editorial,
    Skill,
}

impl TextAssetKind {
    pub fn as_db(self) -> &'static str {
        match self {
            TextAssetKind::Editorial => "editorial",
            TextAssetKind::Skill => "skill",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct TextAsset {
    /// Nom du fichier importé ; `None` pour un texte collé.
    pub file_name: Option<String>,
    pub content: String,
    pub words: i64,
    pub updated_at: i64,
    /// Skill : `name` et `description` de son en-tête YAML, s'il en a un.
    pub title: Option<String>,
    pub description: Option<String>,
}

/// Charte graphique appliquée au document du compte rendu (par l'application,
/// pas par le modèle).
#[derive(Clone, Debug, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GraphicCharter {
    /// `#RRGGBB`.
    pub primary_color: String,
    pub secondary_color: String,
    pub font: String,
    /// Mention de pied de page (vide : aucune).
    pub footer: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct MeetingAssets {
    pub editorial: Option<TextAsset>,
    pub skill: Option<TextAsset>,
    pub graphic: Option<GraphicCharter>,
    /// Chemin absolu du logo, pour l'aperçu.
    pub logo_path: Option<String>,
    pub graphic_updated_at: Option<i64>,
}

/// Erreurs remontées au front, qui les traduit (`meeting.errors.*`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum MeetingError {
    AlreadyRecording,
    NotRecording,
    /// Accès au micro refusé par le système (réglages de confidentialité).
    MicrophonePermission,
    NoMicrophone,
    MicrophoneFailed {
        detail: String,
    },
    DiskSpace {
        #[serde(rename = "freeBytes")]
        free_bytes: u64,
    },
    /// Enregistrement en cours : ni suppression ni export.
    InProgress,
    NotFound,
    UnsupportedFile,
    FileTooLarge {
        #[serde(rename = "maxBytes")]
        max_bytes: u64,
    },
    InvalidFile {
        detail: String,
    },
    InvalidValue,
    Storage {
        detail: String,
    },
    /// Mode strict sans licence valide.
    LicenceRequired,
    /// Un compte rendu est déjà en cours de rédaction.
    ReportBusy,
    Cancelled,
    /// Transcription impossible (modèle de transcription absent, audio
    /// illisible ou sans parole).
    Transcription {
        detail: String,
    },
    /// Charte et skill trop longs pour le contexte du modèle local.
    AssetsTooLong,
    /// Étape de rédaction : erreur du moteur local (téléchargement, mémoire…).
    Summary {
        error: crate::summary::SummaryError,
    },
}

impl std::fmt::Display for MeetingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MeetingError::AlreadyRecording => write!(f, "un enregistrement est déjà en cours"),
            MeetingError::NotRecording => write!(f, "aucun enregistrement en cours"),
            MeetingError::MicrophonePermission => write!(f, "accès au micro refusé"),
            MeetingError::NoMicrophone => write!(f, "aucun micro trouvé"),
            MeetingError::MicrophoneFailed { detail } => write!(f, "micro indisponible: {detail}"),
            MeetingError::DiskSpace { free_bytes } => {
                write!(f, "espace disque insuffisant ({free_bytes} octets libres)")
            }
            MeetingError::InProgress => write!(f, "enregistrement en cours"),
            MeetingError::NotFound => write!(f, "enregistrement introuvable"),
            MeetingError::UnsupportedFile => write!(f, "type de fichier non pris en charge"),
            MeetingError::FileTooLarge { max_bytes } => {
                write!(f, "fichier trop volumineux (max {max_bytes} octets)")
            }
            MeetingError::InvalidFile { detail } => write!(f, "fichier illisible: {detail}"),
            MeetingError::InvalidValue => write!(f, "valeur invalide"),
            MeetingError::Storage { detail } => write!(f, "stockage: {detail}"),
            MeetingError::LicenceRequired => write!(f, "licence requise"),
            MeetingError::ReportBusy => write!(f, "un compte rendu est déjà en cours"),
            MeetingError::Cancelled => write!(f, "annulé"),
            MeetingError::Transcription { detail } => write!(f, "transcription: {detail}"),
            MeetingError::AssetsTooLong => write!(f, "charte et skill trop longs"),
            MeetingError::Summary { error } => write!(f, "rédaction: {error}"),
        }
    }
}

impl std::error::Error for MeetingError {}

impl MeetingError {
    pub fn storage(error: impl std::fmt::Display) -> Self {
        MeetingError::Storage {
            detail: error.to_string(),
        }
    }
}

/// Horodatage unix courant (secondes).
pub fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_serialize_with_camel_case_tag_and_fields() {
        assert_eq!(
            serde_json::to_value(MeetingError::DiskSpace { free_bytes: 7 }).unwrap(),
            serde_json::json!({ "kind": "diskSpace", "freeBytes": 7 })
        );
        assert_eq!(
            serde_json::to_value(MeetingError::MicrophoneFailed { detail: "x".into() }).unwrap(),
            serde_json::json!({ "kind": "microphoneFailed", "detail": "x" })
        );
        assert_eq!(
            serde_json::to_value(MeetingError::AlreadyRecording).unwrap(),
            serde_json::json!({ "kind": "alreadyRecording" })
        );
    }

    #[test]
    fn states_round_trip_through_the_database_value() {
        for state in [
            RecordingState::InProgress,
            RecordingState::Done,
            RecordingState::Interrupted,
        ] {
            assert_eq!(RecordingState::from_db(state.as_db()), state);
        }
        assert_eq!(
            serde_json::to_value(RecordingState::InProgress).unwrap(),
            serde_json::json!("inProgress")
        );
    }

    #[test]
    fn types_implement_specta_type() {
        fn assert_type<T: specta::Type>() {}
        assert_type::<MeetingRecorderStatus>();
        assert_type::<MeetingRecordingFinished>();
        assert_type::<MeetingAssets>();
        assert_type::<MeetingError>();
    }
}
