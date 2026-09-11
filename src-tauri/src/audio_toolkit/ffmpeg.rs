//! Repli ffmpeg pour les formats que symphonia ne décode pas (Opus, AMR, WMA,
//! WebM…) : conversion en WAV 16 kHz mono dans un fichier temporaire, puis
//! décodage par la voie native. ffmpeg est un exécutable séparé (GPL), jamais
//! lié au binaire — voir THIRD_PARTY.md.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

use anyhow::{anyhow, Context, Result};

use super::decode::decode_to_samples;

/// Indicateur Windows `CREATE_NO_WINDOW` : sans lui, lancer un `Command`
/// depuis l'application graphique ouvre une fenêtre console (comme pour
/// yt-dlp, cf. `commands::file_transcription`, ici géré directement car ce
/// module appelle `std::process::Command` sans passer par tauri-plugin-shell).
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Nombre de caractères de la sortie d'erreur de ffmpeg conservés dans le
/// message (utile au support, sans noyer l'utilisateur sous le détail).
const STDERR_EXCERPT_LEN: usize = 200;

/// `ffmpeg` disponible dans le PATH de l'utilisateur (dernier recours).
pub fn ffmpeg_on_path() -> Option<PathBuf> {
    let exe = if cfg!(windows) {
        "ffmpeg.exe"
    } else {
        "ffmpeg"
    };
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|dir| dir.join(exe))
            .find(|candidate| candidate.is_file())
    })
}

/// Décode via symphonia ; si le format n'est pas natif et qu'un `ffmpeg` est
/// fourni, convertit d'abord en WAV 16 kHz mono. Non annulable : équivalent à
/// [`decode_to_samples_with_fallback_cancellable`] avec un drapeau toujours à
/// `false` (pour les appelants qui n'ont pas de mécanisme d'annulation).
pub fn decode_to_samples_with_fallback(path: &Path, ffmpeg: Option<&Path>) -> Result<Vec<f32>> {
    decode_to_samples_with_fallback_cancellable(path, ffmpeg, &|| false)
}

/// Variante annulable de [`decode_to_samples_with_fallback`] : `is_cancelled`
/// est sondée pendant que ffmpeg tourne (toutes les 100 ms), sur le même
/// principe que le téléchargement yt-dlp (`commands::file_transcription::download_media`,
/// qui sonde `CANCEL_FILE_TRANSCRIPTION` et tue le process) — un ffmpeg lancé
/// sur un fichier corrompu ou très volumineux ne bloque plus indéfiniment
/// l'annulation demandée par l'utilisateur.
pub fn decode_to_samples_with_fallback_cancellable(
    path: &Path,
    ffmpeg: Option<&Path>,
    is_cancelled: &dyn Fn() -> bool,
) -> Result<Vec<f32>> {
    let native_error = match decode_to_samples(path) {
        Ok(samples) => return Ok(samples),
        Err(e) => e,
    };

    let Some(ffmpeg) = ffmpeg else {
        return Err(anyhow!(
            "format non pris en charge nativement ({native_error}) et outil de conversion ffmpeg indisponible"
        ));
    };

    // Nom unique (pid + horodatage ms) : deux conversions concurrentes ne se
    // marchent jamais dessus ; le fichier est de toute façon supprimé plus
    // bas, succès, échec ou annulation.
    let temp_wav = std::env::temp_dir().join(format!(
        "ocade-dictee-{}-{}.wav",
        std::process::id(),
        chrono::Utc::now().timestamp_millis()
    ));

    let mut cmd = Command::new(ffmpeg);
    cmd.args(["-y", "-loglevel", "error", "-nostdin", "-i"])
        .arg(path)
        .args([
            "-vn",
            "-ac",
            "1",
            "-ar",
            "16000",
            "-c:a",
            "pcm_s16le",
            "-f",
            "wav",
        ])
        .arg(&temp_wav)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);

    let mut child = cmd
        .spawn()
        .with_context(|| format!("lancement de ffmpeg ({})", ffmpeg.display()))?;

    // stderr lu dans un thread dédié pendant tout le cycle de vie du process :
    // un tube plein (sortie ffmpeg abondante) bloquerait sinon indéfiniment,
    // qu'on soit en train de sonder `try_wait` ou d'attendre après un `kill`.
    let stderr_pipe = child.stderr.take();
    let stderr_reader = std::thread::spawn(move || {
        let mut buf = String::new();
        if let Some(mut pipe) = stderr_pipe {
            let _ = pipe.read_to_string(&mut buf);
        }
        buf
    });

    // Sondage toutes les 100 ms, `is_cancelled` vérifiée avant `try_wait` à
    // chaque tour : une annulation déjà demandée est honorée immédiatement,
    // sans dépendre de la rapidité de ffmpeg sur le fichier en cours.
    let status = loop {
        if is_cancelled() {
            let _ = child.kill();
            let _ = child.wait();
            let _ = stderr_reader.join();
            let _ = std::fs::remove_file(&temp_wav);
            return Err(anyhow!("Conversion annulée"));
        }
        if let Some(status) = child.try_wait().context("attente de ffmpeg")? {
            break status;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    };

    let stderr_output = stderr_reader.join().unwrap_or_default();

    if !status.success() {
        let _ = std::fs::remove_file(&temp_wav);
        let stderr_excerpt: String = stderr_output
            .chars()
            .take(STDERR_EXCERPT_LEN)
            .collect::<String>()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let detail = if stderr_excerpt.is_empty() {
            format!("code {}", status.code().unwrap_or(-1))
        } else {
            format!("code {} : {stderr_excerpt}", status.code().unwrap_or(-1))
        };
        return Err(anyhow!(
            "ffmpeg n'a pas pu convertir ce fichier ({detail}) — format non pris en charge"
        ));
    }

    let result = decode_to_samples(&temp_wav).context("décodage du WAV converti par ffmpeg");
    let _ = std::fs::remove_file(&temp_wav);
    result
}

#[cfg(test)]
mod tests {
    use super::{
        decode_to_samples_with_fallback, decode_to_samples_with_fallback_cancellable,
        ffmpeg_on_path,
    };
    use std::path::{Path, PathBuf};

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name)
    }

    /// Nombre de fichiers `ocade-dictee-*.wav` actuellement dans le dossier
    /// temporaire du système (utilisé pour vérifier l'absence de résidu après
    /// une annulation).
    fn count_ocade_dictee_temp_files() -> usize {
        std::fs::read_dir(std::env::temp_dir())
            .map(|entries| {
                entries
                    .filter_map(|e| e.ok())
                    .filter(|e| {
                        let name = e.file_name();
                        let name = name.to_string_lossy();
                        name.starts_with("ocade-dictee-") && name.ends_with(".wav")
                    })
                    .count()
            })
            .unwrap_or(0)
    }

    #[test]
    fn falls_back_to_ffmpeg_for_opus_and_webm() {
        let Some(ffmpeg) = ffmpeg_on_path() else {
            eprintln!("ffmpeg absent du PATH : test de repli ignoré");
            return;
        };
        for name in ["sample.opus", "sample.webm"] {
            let samples = decode_to_samples_with_fallback(&fixture(name), Some(&ffmpeg))
                .unwrap_or_else(|e| panic!("{name}: {e}"));
            assert!(samples.len() >= 16_000, "{name}: trop peu d'échantillons");
        }
    }

    #[test]
    fn native_formats_do_not_need_ffmpeg() {
        let samples = decode_to_samples_with_fallback(&fixture("sample.flac"), None).unwrap();
        assert!(samples.len() >= 16_000);
    }

    #[test]
    fn unsupported_without_ffmpeg_gives_clear_error() {
        let err = decode_to_samples_with_fallback(&fixture("sample.opus"), None).unwrap_err();
        assert!(err.to_string().contains("ffmpeg"), "message: {err}");
    }

    #[test]
    fn cancelled_conversion_is_reported() {
        let Some(ffmpeg) = ffmpeg_on_path() else {
            eprintln!("ffmpeg absent du PATH : test d'annulation ignoré");
            return;
        };
        let err = decode_to_samples_with_fallback_cancellable(
            &fixture("sample.opus"),
            Some(&ffmpeg),
            &|| true,
        )
        .unwrap_err();
        assert!(err.to_string().contains("annul"), "message: {err}");
        assert_eq!(
            count_ocade_dictee_temp_files(),
            0,
            "fichier temporaire ocade-dictee-*.wav résiduel après annulation"
        );
    }
}
