//! Stockage des réunions : base `meetings.db` (liste des enregistrements,
//! éléments du compte rendu) et dossier `meetings/` (fichiers WAV, logo).
//! Base séparée de `history.db` et `file_history.db` : cycle de vie propre,
//! aucune limite de rétention.

use rusqlite::{Connection, OptionalExtension};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tauri::AppHandle;

use super::wav;
use super::{now_unix, MeetingError, MeetingRecording, MeetingReport, RecordingState};

/// Longueur maximale d'un nom d'enregistrement (caractères).
pub const MAX_NAME_CHARS: usize = 200;

pub struct MeetingStore {
    db_path: PathBuf,
    audio_dir: PathBuf,
    charter_dir: PathBuf,
}

/// Ligne brute de la table `meeting_assets`.
pub struct StoredAsset {
    pub file_name: Option<String>,
    pub content: String,
    pub updated_at: i64,
}

impl MeetingStore {
    pub fn new(app_handle: &AppHandle) -> Result<Self, MeetingError> {
        let root = crate::portable::app_data_dir(app_handle).map_err(MeetingError::storage)?;
        Self::new_at(&root)
    }

    pub fn new_at(root: &Path) -> Result<Self, MeetingError> {
        let audio_dir = root.join("meetings");
        let charter_dir = audio_dir.join("charte");
        std::fs::create_dir_all(&charter_dir).map_err(MeetingError::storage)?;
        let store = Self {
            db_path: root.join("meetings.db"),
            audio_dir,
            charter_dir,
        };
        store.init()?;
        Ok(store)
    }

    pub fn audio_dir(&self) -> &Path {
        &self.audio_dir
    }

    pub fn charter_dir(&self) -> &Path {
        &self.charter_dir
    }

    /// Connexion neuve par opération (même approche que `FileHistoryManager`).
    fn open(&self) -> Result<Connection, MeetingError> {
        let conn = Connection::open(&self.db_path).map_err(MeetingError::storage)?;
        conn.busy_timeout(Duration::from_secs(5))
            .map_err(MeetingError::storage)?;
        Ok(conn)
    }

    fn init(&self) -> Result<(), MeetingError> {
        self.open()?
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS meeting_recordings (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    created_at INTEGER NOT NULL,
                    name TEXT NOT NULL,
                    duration_ms INTEGER NOT NULL DEFAULT 0,
                    size_bytes INTEGER NOT NULL DEFAULT 0,
                    parts INTEGER NOT NULL DEFAULT 1,
                    state TEXT NOT NULL,
                    ended_at INTEGER
                );
                CREATE TABLE IF NOT EXISTS meeting_assets (
                    kind TEXT PRIMARY KEY,
                    file_name TEXT,
                    content TEXT NOT NULL,
                    updated_at INTEGER NOT NULL
                );",
            )
            .map_err(MeetingError::storage)?;
        // Migration douce : colonnes du compte rendu, ajoutées après la
        // première version de l'onglet. « duplicate column » sur une base
        // déjà migrée est attendu.
        let conn = self.open()?;
        for column in [
            "transcript TEXT",
            "report_markdown TEXT",
            "report_updated_at INTEGER",
        ] {
            if let Err(e) = conn.execute(
                &format!("ALTER TABLE meeting_recordings ADD COLUMN {column}"),
                [],
            ) {
                if !e.to_string().contains("duplicate column") {
                    return Err(MeetingError::storage(e));
                }
            }
        }
        Ok(())
    }

    /// Transcription d'un enregistrement, conservée pour rédiger à nouveau le
    /// compte rendu sans retranscrire.
    pub fn set_transcript(&self, id: i64, transcript: &str) -> Result<(), MeetingError> {
        self.open()?
            .execute(
                "UPDATE meeting_recordings SET transcript = ?1 WHERE id = ?2",
                rusqlite::params![transcript, id],
            )
            .map_err(MeetingError::storage)?;
        Ok(())
    }

    pub fn set_report(&self, id: i64, markdown: &str) -> Result<(), MeetingError> {
        self.open()?
            .execute(
                "UPDATE meeting_recordings SET report_markdown = ?1, report_updated_at = ?2
                 WHERE id = ?3",
                rusqlite::params![markdown, now_unix(), id],
            )
            .map_err(MeetingError::storage)?;
        Ok(())
    }

    pub fn report(&self, id: i64) -> Result<MeetingReport, MeetingError> {
        self.open()?
            .query_row(
                "SELECT transcript, report_markdown, report_updated_at
                 FROM meeting_recordings WHERE id = ?1",
                [id],
                |row| {
                    Ok(MeetingReport {
                        recording_id: id,
                        transcript: row.get(0)?,
                        markdown: row.get(1)?,
                        updated_at: row.get(2)?,
                    })
                },
            )
            .optional()
            .map_err(MeetingError::storage)?
            .ok_or(MeetingError::NotFound)
    }

    pub fn insert_recording(&self, name: &str, created_at: i64) -> Result<i64, MeetingError> {
        let conn = self.open()?;
        conn.execute(
            "INSERT INTO meeting_recordings (created_at, name, state) VALUES (?1, ?2, ?3)",
            rusqlite::params![created_at, name, RecordingState::InProgress.as_db()],
        )
        .map_err(MeetingError::storage)?;
        Ok(conn.last_insert_rowid())
    }

    /// Avancement d'un enregistrement en cours, mémorisé régulièrement : la
    /// liste l'affiche, et un plantage ne fait pas perdre la trace du fichier.
    pub fn update_progress(
        &self,
        id: i64,
        duration_ms: i64,
        size_bytes: i64,
        parts: i64,
    ) -> Result<(), MeetingError> {
        self.open()?
            .execute(
                "UPDATE meeting_recordings SET duration_ms = ?1, size_bytes = ?2, parts = ?3
                 WHERE id = ?4",
                rusqlite::params![duration_ms, size_bytes, parts, id],
            )
            .map_err(MeetingError::storage)?;
        Ok(())
    }

    pub fn finish(
        &self,
        id: i64,
        duration_ms: i64,
        size_bytes: i64,
        parts: i64,
        state: RecordingState,
    ) -> Result<(), MeetingError> {
        self.open()?
            .execute(
                "UPDATE meeting_recordings
                 SET duration_ms = ?1, size_bytes = ?2, parts = ?3, state = ?4, ended_at = ?5
                 WHERE id = ?6",
                rusqlite::params![
                    duration_ms,
                    size_bytes,
                    parts,
                    state.as_db(),
                    now_unix(),
                    id
                ],
            )
            .map_err(MeetingError::storage)?;
        Ok(())
    }

    fn row_to_recording(row: &rusqlite::Row<'_>) -> rusqlite::Result<MeetingRecording> {
        let state: String = row.get(6)?;
        Ok(MeetingRecording {
            id: row.get(0)?,
            created_at: row.get(1)?,
            name: row.get(2)?,
            duration_ms: row.get(3)?,
            size_bytes: row.get(4)?,
            parts: row.get(5)?,
            state: RecordingState::from_db(&state),
            has_report: row.get(7)?,
        })
    }

    /// Tous les enregistrements, plus récent d'abord (le tri affiché est fait
    /// côté interface).
    pub fn list(&self) -> Result<Vec<MeetingRecording>, MeetingError> {
        let conn = self.open()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, created_at, name, duration_ms, size_bytes, parts, state,
                        report_markdown IS NOT NULL
                 FROM meeting_recordings ORDER BY created_at DESC, id DESC",
            )
            .map_err(MeetingError::storage)?;
        let rows = stmt
            .query_map([], Self::row_to_recording)
            .map_err(MeetingError::storage)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(MeetingError::storage)?;
        Ok(rows)
    }

    pub fn get(&self, id: i64) -> Result<MeetingRecording, MeetingError> {
        self.open()?
            .query_row(
                "SELECT id, created_at, name, duration_ms, size_bytes, parts, state,
                        report_markdown IS NOT NULL
                 FROM meeting_recordings WHERE id = ?1",
                [id],
                Self::row_to_recording,
            )
            .optional()
            .map_err(MeetingError::storage)?
            .ok_or(MeetingError::NotFound)
    }

    pub fn rename(&self, id: i64, name: &str) -> Result<(), MeetingError> {
        let name = normalize_name(name).ok_or(MeetingError::InvalidValue)?;
        let updated = self
            .open()?
            .execute(
                "UPDATE meeting_recordings SET name = ?1 WHERE id = ?2",
                rusqlite::params![name, id],
            )
            .map_err(MeetingError::storage)?;
        if updated == 0 {
            return Err(MeetingError::NotFound);
        }
        Ok(())
    }

    /// Fichiers WAV de l'enregistrement, dans l'ordre.
    pub fn part_paths(&self, id: i64, parts: i64) -> Vec<PathBuf> {
        (1..=parts.max(1))
            .map(|part| wav::part_path(&self.audio_dir, id, part))
            .collect()
    }

    /// Supprime l'enregistrement et ses fichiers. Un fichier déjà absent n'est
    /// pas une erreur : l'utilisateur a pu le supprimer à la main.
    pub fn delete(&self, id: i64) -> Result<(), MeetingError> {
        let recording = self.get(id)?;
        for path in self.part_paths(id, recording.parts) {
            if let Err(e) = std::fs::remove_file(&path) {
                if e.kind() != std::io::ErrorKind::NotFound {
                    return Err(MeetingError::storage(e));
                }
            }
        }
        self.delete_row(id)
    }

    pub fn delete_row(&self, id: i64) -> Result<(), MeetingError> {
        self.open()?
            .execute("DELETE FROM meeting_recordings WHERE id = ?1", [id])
            .map_err(MeetingError::storage)?;
        Ok(())
    }

    /// Au démarrage : un enregistrement resté « en cours » vient d'une
    /// application arrêtée pendant la capture (plantage, extinction). Ses
    /// fichiers sont réparés pour rendre lisible tout l'audio écrit, et il
    /// passe « interrompu ». Sans audio, il est retiré.
    pub fn recover_interrupted(&self) -> Result<usize, MeetingError> {
        let pending: Vec<MeetingRecording> = self
            .list()?
            .into_iter()
            .filter(|r| r.state == RecordingState::InProgress)
            .collect();
        for recording in &pending {
            let mut data_bytes = 0u64;
            let mut size_bytes = 0u64;
            let mut parts = 0i64;
            for (index, path) in self
                .part_paths(recording.id, recording.parts)
                .iter()
                .enumerate()
            {
                if !path.exists() {
                    continue;
                }
                match wav::repair_header(path) {
                    Ok(bytes) => {
                        data_bytes += bytes;
                        size_bytes += std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
                        parts = index as i64 + 1;
                    }
                    Err(e) => log::warn!(
                        "Réunion {} : fichier {} irréparable ({e})",
                        recording.id,
                        path.display()
                    ),
                }
            }
            if data_bytes == 0 {
                log::warn!("Réunion {} interrompue sans audio : retirée", recording.id);
                for path in self.part_paths(recording.id, recording.parts) {
                    let _ = std::fs::remove_file(path);
                }
                self.delete_row(recording.id)?;
            } else {
                log::warn!(
                    "Réunion {} interrompue : {} ms d'audio récupérés",
                    recording.id,
                    wav::duration_ms_for(data_bytes)
                );
                self.finish(
                    recording.id,
                    wav::duration_ms_for(data_bytes),
                    size_bytes as i64,
                    parts.max(1),
                    RecordingState::Interrupted,
                )?;
            }
        }
        Ok(pending.len())
    }

    pub fn get_asset(&self, kind: &str) -> Result<Option<StoredAsset>, MeetingError> {
        self.open()?
            .query_row(
                "SELECT file_name, content, updated_at FROM meeting_assets WHERE kind = ?1",
                [kind],
                |row| {
                    Ok(StoredAsset {
                        file_name: row.get(0)?,
                        content: row.get(1)?,
                        updated_at: row.get(2)?,
                    })
                },
            )
            .optional()
            .map_err(MeetingError::storage)
    }

    pub fn set_asset(
        &self,
        kind: &str,
        file_name: Option<&str>,
        content: &str,
    ) -> Result<(), MeetingError> {
        self.open()?
            .execute(
                "INSERT INTO meeting_assets (kind, file_name, content, updated_at)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(kind) DO UPDATE SET
                    file_name = excluded.file_name,
                    content = excluded.content,
                    updated_at = excluded.updated_at",
                rusqlite::params![kind, file_name, content, now_unix()],
            )
            .map_err(MeetingError::storage)?;
        Ok(())
    }

    pub fn clear_asset(&self, kind: &str) -> Result<(), MeetingError> {
        self.open()?
            .execute("DELETE FROM meeting_assets WHERE kind = ?1", [kind])
            .map_err(MeetingError::storage)?;
        Ok(())
    }
}

/// Nom nettoyé (espaces de bord retirés, retours à la ligne aplatis), ou
/// `None` s'il est vide ou trop long.
pub fn normalize_name(name: &str) -> Option<String> {
    let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
    if name.is_empty() || name.chars().count() > MAX_NAME_CHARS {
        return None;
    }
    Some(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn store() -> (tempfile::TempDir, MeetingStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = MeetingStore::new_at(dir.path()).unwrap();
        (dir, store)
    }

    fn write_wav(path: &Path, samples: usize) {
        let mut writer = hound::WavWriter::create(path, wav::spec()).unwrap();
        for _ in 0..samples {
            writer.write_sample(1i16).unwrap();
        }
        writer.finalize().unwrap();
    }

    #[test]
    fn recordings_are_listed_newest_first_and_can_be_renamed() {
        let (_dir, store) = store();
        let a = store.insert_recording("Réunion A", 100).unwrap();
        let b = store.insert_recording("Réunion B", 200).unwrap();
        store
            .finish(a, 1000, 32044, 1, RecordingState::Done)
            .unwrap();
        let list = store.list().unwrap();
        assert_eq!(list.iter().map(|r| r.id).collect::<Vec<_>>(), vec![b, a]);
        assert_eq!(list[1].state, RecordingState::Done);
        assert_eq!(list[0].state, RecordingState::InProgress);

        store.rename(a, "  Comité\n de pilotage  ").unwrap();
        assert_eq!(store.get(a).unwrap().name, "Comité de pilotage");
        assert_eq!(store.rename(a, "   "), Err(MeetingError::InvalidValue));
        assert_eq!(store.rename(999, "x"), Err(MeetingError::NotFound));
    }

    #[test]
    fn deleting_removes_every_part_and_tolerates_missing_files() {
        let (_dir, store) = store();
        let id = store.insert_recording("R", 1).unwrap();
        store.update_progress(id, 0, 0, 2).unwrap();
        let paths = store.part_paths(id, 2);
        write_wav(&paths[0], 10);
        // La 2e partie manque : la suppression passe quand même.
        store.delete(id).unwrap();
        assert!(!paths[0].exists());
        assert_eq!(store.get(id).unwrap_err(), MeetingError::NotFound);
    }

    #[test]
    fn interrupted_recordings_are_repaired_or_removed_at_startup() {
        let (_dir, store) = store();
        // Plantage pendant la capture : 1 s d'audio, en-tête périmé.
        let crashed = store.insert_recording("Plantée", 1).unwrap();
        let path = store.part_paths(crashed, 1).remove(0);
        write_wav(&path, 8000);
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        file.write_all(&[0u8; 16_000]).unwrap();
        drop(file);
        // Plantage avant la première écriture : rien à garder.
        let empty = store.insert_recording("Vide", 2).unwrap();
        // Enregistrement terminé : intouché.
        let done = store.insert_recording("Finie", 3).unwrap();
        store.finish(done, 5, 6, 1, RecordingState::Done).unwrap();

        assert_eq!(store.recover_interrupted().unwrap(), 2);

        let recovered = store.get(crashed).unwrap();
        assert_eq!(recovered.state, RecordingState::Interrupted);
        assert_eq!(recovered.duration_ms, 1000);
        assert_eq!(hound::WavReader::open(&path).unwrap().len(), 16_000);
        assert_eq!(store.get(empty).unwrap_err(), MeetingError::NotFound);
        assert_eq!(store.get(done).unwrap().duration_ms, 5);
    }

    #[test]
    fn assets_are_upserted_and_cleared() {
        let (_dir, store) = store();
        assert!(store.get_asset("editorial").unwrap().is_none());
        store
            .set_asset("editorial", Some("charte.md"), "Vouvoyez.")
            .unwrap();
        store.set_asset("editorial", None, "Tutoyez.").unwrap();
        let asset = store.get_asset("editorial").unwrap().unwrap();
        assert_eq!(asset.content, "Tutoyez.");
        assert_eq!(asset.file_name, None);
        store.clear_asset("editorial").unwrap();
        assert!(store.get_asset("editorial").unwrap().is_none());
    }

    #[test]
    fn transcript_and_report_are_kept_and_survive_reopening() {
        let (dir, store) = store();
        let id = store.insert_recording("R", 1).unwrap();
        assert!(!store.get(id).unwrap().has_report);
        assert_eq!(store.report(id).unwrap().markdown, None);
        store.set_transcript(id, "Bonjour à tous.").unwrap();
        store.set_report(id, "# CR").unwrap();
        // Rouvrir la base rejoue la migration sans erreur.
        let reopened = MeetingStore::new_at(dir.path()).unwrap();
        let report = reopened.report(id).unwrap();
        assert_eq!(report.transcript.as_deref(), Some("Bonjour à tous."));
        assert_eq!(report.markdown.as_deref(), Some("# CR"));
        assert!(report.updated_at.is_some());
        assert!(reopened.list().unwrap()[0].has_report);
        assert_eq!(reopened.report(999), Err(MeetingError::NotFound));
    }

    #[test]
    fn names_are_normalized() {
        assert_eq!(normalize_name(" a  b "), Some("a b".into()));
        assert_eq!(normalize_name(""), None);
        assert_eq!(normalize_name(&"x".repeat(MAX_NAME_CHARS + 1)), None);
    }
}
