//! Transcription d'un enregistrement de réunion. Contrairement à l'onglet
//! Fichier, qui décode tout le fichier en mémoire, l'audio est lu par
//! tronçons de 30 s directement dans les WAV 16 kHz de la réunion : une
//! réunion de 10 h n'occupe jamais plus d'un tronçon en mémoire.

use std::path::PathBuf;

use crate::commands::file_transcription::{
    transcribe_chunk_resilient, CHUNK_LEN_SAMPLES, MAX_LEADING_FAILED_CHUNKS, UNTRANSCRIBED_MARK,
};

/// Tronçons regroupés par paragraphe dans la transcription (4 × 30 s =
/// 2 min) : plus lisible qu'un seul bloc, et des frontières de découpage
/// naturelles pour le résumé.
const CHUNKS_PER_PARAGRAPH: usize = 4;
/// Crête sous laquelle un tronçon est un silence numérique (micro coupé,
/// pause) : il n'est pas envoyé au modèle, qui y inventerait des phrases.
const SILENCE_PEAK: f32 = 0.003;

/// Erreur de transcription ; `Cancelled` quand l'utilisateur a annulé.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranscribeError {
    Cancelled,
    /// Aucun audio lisible.
    Unreadable(String),
    /// Échec systémique (modèle absent, moteur hors service) ou rien transcrit.
    Failed(String),
}

/// Nombre de tronçons de 30 s des fichiers donnés (pour la progression).
pub fn chunk_count(paths: &[PathBuf]) -> Result<u32, TranscribeError> {
    let mut total = 0u32;
    for path in paths {
        let reader = hound::WavReader::open(path)
            .map_err(|e| TranscribeError::Unreadable(format!("{}: {e}", path.display())))?;
        total += (reader.len() as usize).div_ceil(CHUNK_LEN_SAMPLES) as u32;
    }
    Ok(total)
}

/// Transcrit les fichiers dans l'ordre. `transcribe` reçoit des tronçons
/// mono 16 kHz ; `on_chunk(fait, total)` suit l'avancement.
pub fn transcribe_parts(
    paths: &[PathBuf],
    transcribe: &mut dyn FnMut(&[f32]) -> Result<String, String>,
    cancelled: &dyn Fn() -> bool,
    on_chunk: &mut dyn FnMut(u32, u32),
) -> Result<String, TranscribeError> {
    let total = chunk_count(paths)?;
    let mut done = 0u32;
    let mut texts: Vec<String> = Vec::new();
    let mut any_audio = false;
    let mut buffer: Vec<f32> = Vec::with_capacity(CHUNK_LEN_SAMPLES);

    for path in paths {
        let mut reader = hound::WavReader::open(path)
            .map_err(|e| TranscribeError::Unreadable(format!("{}: {e}", path.display())))?;
        let mut samples = reader.samples::<i16>();
        loop {
            buffer.clear();
            for sample in samples.by_ref().take(CHUNK_LEN_SAMPLES) {
                // Un échantillon illisible (fin de fichier tronquée) termine
                // ce fichier sans perdre ce qui précède.
                match sample {
                    Ok(value) => buffer.push(value as f32 / 32_768.0),
                    Err(_) => break,
                }
            }
            if buffer.is_empty() {
                break;
            }
            if cancelled() {
                return Err(TranscribeError::Cancelled);
            }
            let peak = buffer.iter().fold(0f32, |m, s| m.max(s.abs()));
            let text = if peak < SILENCE_PEAK {
                String::new()
            } else {
                any_audio = true;
                match transcribe_chunk_resilient(&buffer, transcribe, cancelled) {
                    Ok(text) => text,
                    Err(e)
                        if done as usize + 1 >= MAX_LEADING_FAILED_CHUNKS
                            && texts
                                .iter()
                                .all(|t| t.is_empty() || t == UNTRANSCRIBED_MARK) =>
                    {
                        return Err(TranscribeError::Failed(e));
                    }
                    Err(e) => {
                        log::warn!("Réunion : tronçon {} non transcrit ({e})", done + 1);
                        UNTRANSCRIBED_MARK.to_string()
                    }
                }
            };
            if cancelled() {
                return Err(TranscribeError::Cancelled);
            }
            texts.push(text);
            done += 1;
            on_chunk(done, total.max(done));
        }
    }

    if !any_audio {
        return Err(TranscribeError::Failed(
            "aucune parole : l'enregistrement est silencieux".into(),
        ));
    }
    if texts
        .iter()
        .all(|t| t.trim().is_empty() || t == UNTRANSCRIBED_MARK)
    {
        return Err(TranscribeError::Failed(
            "aucun passage n'a pu être transcrit".into(),
        ));
    }
    Ok(texts
        .chunks(CHUNKS_PER_PARAGRAPH)
        .map(crate::managers::document::assemble_document)
        .filter(|paragraph| !paragraph.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meeting::wav;

    fn write(path: &std::path::Path, seconds: usize, amplitude: i16) {
        let mut writer = hound::WavWriter::create(path, wav::spec()).unwrap();
        for i in 0..seconds * 16_000 {
            let value = if i % 2 == 0 { amplitude } else { -amplitude };
            writer.write_sample(value).unwrap();
        }
        writer.finalize().unwrap();
    }

    #[test]
    fn parts_are_read_in_30_second_chunks_and_silence_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("a.wav");
        let second = dir.path().join("b.wav");
        write(&first, 65, 8_000); // 3 tronçons : 30 s, 30 s, 5 s
        write(&second, 40, 0); // 2 tronçons silencieux
        let paths = vec![first, second];
        assert_eq!(chunk_count(&paths).unwrap(), 5);

        let mut lengths = Vec::new();
        let mut progress = Vec::new();
        let text = transcribe_parts(
            &paths,
            &mut |pcm| {
                lengths.push(pcm.len());
                Ok(format!("phrase{}", lengths.len()))
            },
            &|| false,
            &mut |done, total| progress.push((done, total)),
        )
        .unwrap();
        assert_eq!(lengths, vec![480_000, 480_000, 80_000]);
        assert_eq!(text, "phrase1 phrase2 phrase3");
        assert_eq!(progress.last(), Some(&(5, 5)));
    }

    #[test]
    fn paragraphs_group_four_chunks() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.wav");
        write(&path, 150, 8_000); // 5 tronçons
        let mut n = 0;
        let text = transcribe_parts(
            &[path],
            &mut |_| {
                n += 1;
                Ok(format!("t{n}."))
            },
            &|| false,
            &mut |_, _| {},
        )
        .unwrap();
        assert_eq!(text, "t1. t2. t3. t4.\n\nt5.");
    }

    #[test]
    fn silence_only_and_cancellation_are_errors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.wav");
        write(&path, 31, 0);
        assert!(matches!(
            transcribe_parts(
                std::slice::from_ref(&path),
                &mut |_| Ok("x".into()),
                &|| false,
                &mut |_, _| {}
            ),
            Err(TranscribeError::Failed(_))
        ));
        let loud = dir.path().join("b.wav");
        write(&loud, 31, 8_000);
        assert_eq!(
            transcribe_parts(&[loud], &mut |_| Ok("x".into()), &|| true, &mut |_, _| {}),
            Err(TranscribeError::Cancelled)
        );
    }

    #[test]
    fn a_systemic_failure_stops_early() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.wav");
        write(&path, 200, 8_000);
        let mut calls = 0;
        let result = transcribe_parts(
            &[path],
            &mut |_| {
                calls += 1;
                Err("modèle absent".into())
            },
            &|| false,
            &mut |_, _| {},
        );
        assert!(matches!(result, Err(TranscribeError::Failed(_))));
        // Arrêt après les premiers tronçons, pas après les 7.
        assert!(calls < 7 * 20, "{calls} appels");
    }
}
