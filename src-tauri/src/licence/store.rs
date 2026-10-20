//! Persistance locale de la licence : `licence.json` dans le dossier de données.
//!
//! La clé est un identifiant, pas un secret fort : un fichier en clair dans le
//! dossier de l'utilisateur suffit. Le jeton est signé : le modifier à la main
//! l'invalide. L'écriture est atomique (fichier temporaire puis renommage) pour
//! qu'une coupure ne laisse jamais un fichier tronqué.

use serde::{Deserialize, Serialize};
use specta::Type;
use std::io;
use std::path::Path;

/// Mode d'application, piloté par le serveur.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "lowercase")]
pub enum LicenceMode {
    /// L'état s'affiche, rien n'est bloqué.
    #[default]
    Soft,
    /// Sans licence valide, la dictée est bloquée.
    Strict,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Persisted {
    /// Clé normalisée (`OCAD-…`), une fois activée sur ce poste.
    pub key: Option<String>,
    /// Dernier jeton reçu. Effacé après un refus définitif.
    pub token: Option<String>,
    /// Code du dernier refus définitif (`revoked`, `expired`, `archived`,
    /// `unknown_key`, `not_activated`), pour qu'un redémarrage hors ligne ne
    /// fasse pas « revivre » une licence retirée.
    pub last_refusal: Option<String>,
    /// Dernier mode connu ; `None` tant que le serveur n'a jamais répondu.
    pub mode_cache: Option<LicenceMode>,
    /// Plus grande heure vue (système ou serveur) : elle n'avance jamais en
    /// arrière, ce qui neutralise un recul volontaire de l'horloge.
    pub last_seen_ts: i64,
    /// Heure (serveur) de la dernière vérification réussie.
    pub last_refresh_ok_ts: Option<i64>,
    /// La dernière tentative de vérification a échoué (réseau, serveur).
    pub last_refresh_failed: bool,
}

impl Persisted {
    /// Fait avancer `last_seen_ts`, jamais reculer.
    pub fn observe_time(&mut self, ts: i64) {
        self.last_seen_ts = self.last_seen_ts.max(ts);
    }

    /// Heure de référence : la plus grande entre l'horloge du poste et la
    /// dernière heure vue.
    pub fn effective_now(&self, system_now: i64) -> i64 {
        system_now.max(self.last_seen_ts)
    }
}

/// Lit le fichier ; un fichier absent ou illisible donne un état vierge.
pub fn load(path: &Path) -> Persisted {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            log::warn!("licence.json illisible, état de licence réinitialisé : {e}");
            Persisted::default()
        }),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Persisted::default(),
        Err(e) => {
            log::warn!("Lecture de licence.json impossible : {e}");
            Persisted::default()
        }
    }
}

/// Écrit le fichier de façon atomique.
pub fn save(path: &Path, state: &Persisted) -> io::Result<()> {
    let json = serde_json::to_vec_pretty(state).map_err(io::Error::other)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// Heure système en secondes Unix.
pub fn system_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn save_then_load_round_trips() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("licence.json");
        let state = Persisted {
            key: Some("OCAD-7XQ2-M9KD-T4HW-3RCP".into()),
            token: Some("a.b.c".into()),
            mode_cache: Some(LicenceMode::Strict),
            last_seen_ts: 42,
            ..Default::default()
        };
        save(&path, &state).unwrap();
        assert_eq!(load(&path), state);
        // Réécriture par-dessus un fichier existant (renommage atomique).
        let mut next = state.clone();
        next.token = None;
        save(&path, &next).unwrap();
        assert_eq!(load(&path), next);
        assert!(!path.with_extension("json.tmp").exists());
    }

    #[test]
    fn missing_or_corrupt_file_gives_a_blank_state() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("licence.json");
        assert_eq!(load(&path), Persisted::default());
        std::fs::write(&path, b"{pas du json").unwrap();
        assert_eq!(load(&path), Persisted::default());
    }

    #[test]
    fn the_clock_never_goes_backwards() {
        let mut state = Persisted::default();
        state.observe_time(1_000);
        state.observe_time(500);
        assert_eq!(state.last_seen_ts, 1_000);
        assert_eq!(state.effective_now(800), 1_000, "heure reculée ignorée");
        assert_eq!(state.effective_now(1_200), 1_200);
    }
}
