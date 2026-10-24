//! Format des fichiers de réunion : WAV PCM 16 bits mono à 16 kHz, le format
//! que lisent tous les modèles de transcription de l'application. Soit
//! 32 000 octets par seconde, ~115 Mo par heure, ~2,8 Go pour 24 h.
//!
//! Un WAV ne dépasse pas 4 Go (tailles sur 32 bits) : au-delà de
//! `MAX_PART_BYTES` (~37 h), l'enregistrement continue dans un fichier
//! suivant, `reunion-<id>-2.wav`, et ainsi de suite.

use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

pub const SAMPLE_RATE: u32 = 16_000;
pub const BYTES_PER_SECOND: u64 = SAMPLE_RATE as u64 * 2;

/// Taille maximale d'un fichier : 4 Gio moins une marge pour l'en-tête et le
/// tampon d'écriture.
pub const MAX_PART_BYTES: u64 = 4 * 1024 * 1024 * 1024 - 16 * 1024 * 1024;

pub fn spec() -> hound::WavSpec {
    hound::WavSpec {
        channels: 1,
        sample_rate: SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    }
}

/// Chemin du fichier `part` (1-based) de l'enregistrement `id`.
pub fn part_path(dir: &Path, id: i64, part: i64) -> PathBuf {
    if part <= 1 {
        dir.join(format!("reunion-{id}.wav"))
    } else {
        dir.join(format!("reunion-{id}-{part}.wav"))
    }
}

/// Durée (ms) correspondant à `data_bytes` octets d'audio.
pub fn duration_ms_for(data_bytes: u64) -> i64 {
    (data_bytes * 1000 / BYTES_PER_SECOND) as i64
}

/// Remet en cohérence l'en-tête d'un WAV dont l'écriture a été interrompue
/// (plantage, coupure de courant) : les tailles déclarées sont recalculées à
/// partir de la taille réelle du fichier, si bien que tout l'audio écrit sur
/// le disque redevient lisible. Renvoie le nombre d'octets d'audio.
pub fn repair_header(path: &Path) -> std::io::Result<u64> {
    let mut file = OpenOptions::new().read(true).write(true).open(path)?;
    let file_len = file.metadata()?.len();

    let mut riff = [0u8; 12];
    file.read_exact(&mut riff)?;
    if &riff[0..4] != b"RIFF" || &riff[8..12] != b"WAVE" {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "en-tête RIFF/WAVE absent",
        ));
    }

    // Parcours des blocs jusqu'à `data`.
    let mut offset = 12u64;
    loop {
        if offset + 8 > file_len {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "bloc data absent",
            ));
        }
        file.seek(SeekFrom::Start(offset))?;
        let mut header = [0u8; 8];
        file.read_exact(&mut header)?;
        let size = u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as u64;
        if &header[0..4] == b"data" {
            let data_start = offset + 8;
            // Échantillons entiers uniquement (16 bits mono).
            let data_len = (file_len.saturating_sub(data_start) & !1).min(u32::MAX as u64);
            if data_start + data_len < file_len {
                file.set_len(data_start + data_len)?;
            }
            let riff_size = (data_start + data_len - 8).min(u32::MAX as u64) as u32;
            file.seek(SeekFrom::Start(4))?;
            file.write_all(&riff_size.to_le_bytes())?;
            file.seek(SeekFrom::Start(offset + 4))?;
            file.write_all(&(data_len as u32).to_le_bytes())?;
            file.sync_all()?;
            return Ok(data_len);
        }
        // Les blocs sont alignés sur 2 octets.
        offset = offset + 8 + size + (size & 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_samples(path: &Path, count: usize) {
        let mut writer = hound::WavWriter::create(path, spec()).unwrap();
        for i in 0..count {
            writer.write_sample((i % 100) as i16).unwrap();
        }
        writer.finalize().unwrap();
    }

    #[test]
    fn part_paths_number_files_after_the_first() {
        let dir = Path::new("/tmp/m");
        assert_eq!(part_path(dir, 7, 1), dir.join("reunion-7.wav"));
        assert_eq!(part_path(dir, 7, 2), dir.join("reunion-7-2.wav"));
    }

    #[test]
    fn durations_follow_the_byte_rate() {
        assert_eq!(duration_ms_for(BYTES_PER_SECOND), 1000);
        assert_eq!(duration_ms_for(BYTES_PER_SECOND * 3600), 3_600_000);
        // ~37 h d'audio par fichier avant de passer au suivant.
        assert!(duration_ms_for(MAX_PART_BYTES) > 37 * 3_600_000);
    }

    #[test]
    fn repair_recovers_audio_written_after_the_last_header_update() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.wav");
        write_samples(&path, 1000);
        // Simule un plantage : de l'audio ajouté sans mise à jour de l'en-tête,
        // plus un octet orphelin (échantillon incomplet).
        let mut file = OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(&[1u8; 501]).unwrap();
        drop(file);

        let data = repair_header(&path).unwrap();
        assert_eq!(data, 2000 + 500);
        let reader = hound::WavReader::open(&path).unwrap();
        assert_eq!(reader.len(), 1250);
        assert_eq!(std::fs::metadata(&path).unwrap().len(), 44 + 2500);
    }

    #[test]
    fn repair_rejects_a_file_that_is_not_a_wav() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("b.wav");
        std::fs::write(&path, b"not a wav file at all").unwrap();
        assert!(repair_header(&path).is_err());
    }

    #[test]
    fn header_flush_keeps_the_file_readable_while_recording() {
        // `flush()` met l'en-tête à jour : le fichier reste lisible même si
        // l'application s'arrête avant `finalize()`.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("c.wav");
        let mut writer = hound::WavWriter::create(&path, spec()).unwrap();
        for _ in 0..320 {
            writer.write_sample(5i16).unwrap();
        }
        writer.flush().unwrap();
        let reader = hound::WavReader::open(&path).unwrap();
        assert_eq!(reader.len(), 320);
        drop(reader);
        writer.finalize().unwrap();
    }
}
