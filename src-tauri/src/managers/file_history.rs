use anyhow::Result;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use specta::Type;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::AppHandle;

/// Entrée complète de l'historique des transcriptions de fichiers.
#[derive(Clone, Debug, Serialize, Deserialize, Type)]
pub struct FileHistoryEntry {
    pub id: i64,
    /// Horodatage unix (secondes).
    pub created_at: i64,
    /// Nom lisible de la source (nom du fichier ou titre de la vidéo).
    pub source_name: String,
    /// "local" (fichier déposé) ou "url" (vidéo téléchargée).
    pub source_kind: String,
    /// Chemin d'origine du fichier, ou URL de la vidéo.
    pub source_ref: String,
    pub raw_text: String,
    /// Compte-rendu Markdown produit par « Résumer » (plan 09) ; `None` tant
    /// qu'aucun résumé n'a été calculé pour cette entrée.
    pub summary_markdown: Option<String>,
    /// Vidéo MP4 conservée dans les données de l'app (entrées "url"
    /// uniquement, téléchargée à la demande).
    pub video_path: Option<String>,
}

/// Ligne allégée pour l'affichage de la liste : les textes complets restent en
/// base et se chargent à la demande via `get` (une entrée peut faire des
/// centaines de Ko pour une longue vidéo).
#[derive(Clone, Debug, Serialize, Deserialize, Type)]
pub struct FileHistoryItem {
    pub id: i64,
    pub created_at: i64,
    pub source_name: String,
    pub source_kind: String,
    /// Premiers caractères du texte brut, pour l'aperçu.
    pub snippet: String,
    /// Une vidéo est déjà conservée pour cette entrée.
    pub has_video: bool,
}

/// Historique persistant des transcriptions de fichiers/URL. Base SQLite
/// séparée de `history.db` (dictées) : cycles de vie indépendants — les
/// dictées ont une limite de rétention et des fichiers audio associés, les
/// transcriptions de fichiers sont du texte pur conservé sans limite.
pub struct FileHistoryManager {
    db_path: PathBuf,
}

impl FileHistoryManager {
    pub fn new(app_handle: &AppHandle) -> Result<Self> {
        let db_path = crate::portable::app_data_dir(app_handle)?.join("file_history.db");
        let manager = Self { db_path };
        manager.init()?;
        Ok(manager)
    }

    #[cfg(test)]
    fn new_at(db_path: PathBuf) -> Result<Self> {
        let manager = Self { db_path };
        manager.init()?;
        Ok(manager)
    }

    /// Connexion neuve par opération (même approche que `HistoryManager`) ;
    /// le trafic est minuscule, et le busy timeout absorbe un éventuel
    /// chevauchement d'écritures.
    fn open(&self) -> Result<Connection> {
        let conn = Connection::open(&self.db_path)?;
        conn.busy_timeout(Duration::from_secs(5))?;
        Ok(conn)
    }

    fn init(&self) -> Result<()> {
        let conn = self.open()?;
        conn.execute(
            "CREATE TABLE IF NOT EXISTS file_transcription_history (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                created_at INTEGER NOT NULL,
                source_name TEXT NOT NULL,
                source_kind TEXT NOT NULL,
                source_ref TEXT NOT NULL,
                raw_text TEXT NOT NULL,
                formatted_text TEXT
            )",
            [],
        )?;
        // Migration douce (v0.9.13) : colonne ajoutée après coup ; l'erreur
        // « duplicate column » des bases déjà migrées est attendue.
        if let Err(e) = conn.execute(
            "ALTER TABLE file_transcription_history ADD COLUMN video_path TEXT",
            [],
        ) {
            if !e.to_string().contains("duplicate column") {
                return Err(e.into());
            }
        }
        // Migration douce (plan 09) : compte-rendu local. La colonne
        // `formatted_text` (ancien bouton « Mettre en forme ») reste en base,
        // plus jamais relue.
        if let Err(e) = conn.execute(
            "ALTER TABLE file_transcription_history ADD COLUMN summary_markdown TEXT",
            [],
        ) {
            if !e.to_string().contains("duplicate column") {
                return Err(e.into());
            }
        }
        Ok(())
    }

    pub fn insert(
        &self,
        source_name: &str,
        source_kind: &str,
        source_ref: &str,
        raw_text: &str,
    ) -> Result<i64> {
        let created_at = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as i64;
        let conn = self.open()?;
        conn.execute(
            "INSERT INTO file_transcription_history
                (created_at, source_name, source_kind, source_ref, raw_text)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![created_at, source_name, source_kind, source_ref, raw_text],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// Mémorise le compte-rendu Markdown d'une entrée (recalculable : la
    /// nouvelle valeur remplace l'ancienne).
    // Appelée par `summary::commands::summarize_document` (tâche 15) : pas
    // encore de site d'appel dans le crate tant que cette tâche n'est pas
    // faite, d'où l'allow ciblé plutôt qu'un avertissement `dead_code`.
    #[allow(dead_code)]
    pub fn update_summary(&self, id: i64, summary_markdown: &str) -> Result<()> {
        let updated = self.open()?.execute(
            "UPDATE file_transcription_history SET summary_markdown = ?1 WHERE id = ?2",
            rusqlite::params![summary_markdown, id],
        )?;
        anyhow::ensure!(updated == 1, "entrée d'historique {id} introuvable");
        Ok(())
    }

    pub fn list(&self) -> Result<Vec<FileHistoryItem>> {
        let conn = self.open()?;
        let mut stmt = conn.prepare(
            // substr() de SQLite compte des caractères (pas des octets) sur du
            // TEXT : sûr pour les textes multi-octets.
            "SELECT id, created_at, source_name, source_kind, substr(raw_text, 1, 200),
                    video_path IS NOT NULL
             FROM file_transcription_history
             ORDER BY created_at DESC, id DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(FileHistoryItem {
                id: row.get(0)?,
                created_at: row.get(1)?,
                source_name: row.get(2)?,
                source_kind: row.get(3)?,
                snippet: row.get(4)?,
                has_video: row.get(5)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn get(&self, id: i64) -> Result<Option<FileHistoryEntry>> {
        let conn = self.open()?;
        let mut stmt = conn.prepare(
            "SELECT id, created_at, source_name, source_kind, source_ref, raw_text,
                    summary_markdown, video_path
             FROM file_transcription_history WHERE id = ?1",
        )?;
        let mut rows = stmt.query_map([id], |row| {
            Ok(FileHistoryEntry {
                id: row.get(0)?,
                created_at: row.get(1)?,
                source_name: row.get(2)?,
                source_kind: row.get(3)?,
                source_ref: row.get(4)?,
                raw_text: row.get(5)?,
                summary_markdown: row.get(6)?,
                video_path: row.get(7)?,
            })
        })?;
        Ok(rows.next().transpose()?)
    }

    /// Mémorise le chemin de la vidéo conservée pour une entrée.
    pub fn set_video_path(&self, id: i64, video_path: &str) -> Result<()> {
        let updated = self.open()?.execute(
            "UPDATE file_transcription_history SET video_path = ?1 WHERE id = ?2",
            rusqlite::params![video_path, id],
        )?;
        anyhow::ensure!(updated == 1, "entrée d'historique {id} introuvable");
        Ok(())
    }

    pub fn delete(&self, id: i64) -> Result<()> {
        // Supprime aussi la vidéo conservée, pour ne pas laisser d'orphelin
        // dans les données de l'app (best-effort).
        if let Some(entry) = self.get(id)? {
            if let Some(video_path) = entry.video_path {
                let _ = std::fs::remove_file(video_path);
            }
        }
        let deleted = self.open()?.execute(
            "DELETE FROM file_transcription_history WHERE id = ?1",
            rusqlite::params![id],
        )?;
        anyhow::ensure!(deleted == 1, "entrée d'historique {id} introuvable");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn manager() -> (TempDir, FileHistoryManager) {
        let dir = TempDir::new().unwrap();
        let manager = FileHistoryManager::new_at(dir.path().join("file_history.db")).unwrap();
        (dir, manager)
    }

    #[test]
    fn test_file_history_round_trip() {
        let (_dir, m) = manager();

        let id = m
            .insert("réunion.mp3", "local", "/tmp/réunion.mp3", "Bonjour à tous")
            .unwrap();
        let id2 = m
            .insert(
                "Conférence Rust",
                "url",
                "https://www.youtube.com/watch?v=abc123",
                "Bienvenue à cette conférence",
            )
            .unwrap();
        assert_ne!(id, id2);

        // Liste : plus récent d'abord (créés dans la même seconde -> id DESC).
        let items = m.list().unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].id, id2);
        assert_eq!(items[0].source_kind, "url");
        assert_eq!(items[1].snippet, "Bonjour à tous");

        // Détail complet + enregistrement du compte-rendu (plan 09).
        let entry = m.get(id).unwrap().unwrap();
        assert_eq!(entry.source_ref, "/tmp/réunion.mp3");
        assert_eq!(entry.summary_markdown, None);
        m.update_summary(id, "# Réunion\n\n## Résumé\nBonjour à tous")
            .unwrap();
        let entry = m.get(id).unwrap().unwrap();
        assert_eq!(
            entry.summary_markdown.as_deref(),
            Some("# Réunion\n\n## Résumé\nBonjour à tous")
        );

        // Vidéo conservée : chemin mémorisé, drapeau dans la liste, fichier
        // supprimé avec l'entrée.
        let video = _dir.path().join("Réunion [abc].mp4");
        std::fs::write(&video, b"fake mp4").unwrap();
        assert!(!m.list().unwrap()[1].has_video);
        m.set_video_path(id, video.to_str().unwrap()).unwrap();
        assert!(m.list().unwrap()[1].has_video);
        assert_eq!(
            m.get(id).unwrap().unwrap().video_path.as_deref(),
            video.to_str()
        );

        // Suppression.
        m.delete(id).unwrap();
        assert!(m.get(id).unwrap().is_none());
        assert!(!video.exists(), "la vidéo conservée est supprimée aussi");
        assert_eq!(m.list().unwrap().len(), 1);
        assert!(m.delete(id).is_err(), "double suppression signalée");
        assert!(m.update_summary(id, "x").is_err());
    }

    /// Une base créée avant le plan 09 (sans `summary_markdown`, avec
    /// `formatted_text`) est migrée en douceur à l'ouverture.
    #[test]
    fn summary_column_is_added_to_existing_databases() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("file_history.db");
        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch(
                "CREATE TABLE file_transcription_history (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    created_at INTEGER NOT NULL,
                    source_name TEXT NOT NULL,
                    source_kind TEXT NOT NULL,
                    source_ref TEXT NOT NULL,
                    raw_text TEXT NOT NULL,
                    formatted_text TEXT,
                    video_path TEXT
                );
                INSERT INTO file_transcription_history
                    (created_at, source_name, source_kind, source_ref, raw_text, formatted_text)
                VALUES (1, 'ancien.mp3', 'local', '/tmp/ancien.mp3', 'Texte brut', 'Texte mis en forme');",
            )
            .unwrap();
        }
        let m = FileHistoryManager::new_at(db_path.clone()).unwrap();
        let entry = m.get(1).unwrap().expect("entrée conservée");
        assert_eq!(entry.raw_text, "Texte brut");
        assert_eq!(entry.summary_markdown, None);
        m.update_summary(1, "# Ancien").unwrap();
        assert_eq!(
            m.get(1).unwrap().unwrap().summary_markdown.as_deref(),
            Some("# Ancien")
        );
        // Réouverture : la migration douce est idempotente.
        let again = FileHistoryManager::new_at(db_path).unwrap();
        assert_eq!(
            again.get(1).unwrap().unwrap().summary_markdown.as_deref(),
            Some("# Ancien")
        );
    }
}
