//! Repli ffmpeg pour les formats que symphonia ne décode pas (Opus, AMR, WMA,
//! WebM…) : conversion en WAV 16 kHz mono dans un fichier temporaire, puis
//! décodage par la voie native. ffmpeg est un exécutable séparé (GPL), jamais
//! lié au binaire — voir THIRD_PARTY.md.

use std::path::{Path, PathBuf};
use std::process::Command;

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
/// fourni, convertit d'abord en WAV 16 kHz mono.
pub fn decode_to_samples_with_fallback(path: &Path, ffmpeg: Option<&Path>) -> Result<Vec<f32>> {
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
    // bas, succès comme échec.
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
        .arg(&temp_wav);
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);

    // .output() (plutôt que .status()) pour récupérer stderr : le message
    // d'erreur reste en français, mais embarque l'extrait de sortie ffmpeg
    // utile au support.
    let output = cmd
        .output()
        .with_context(|| format!("lancement de ffmpeg ({})", ffmpeg.display()))?;

    if !output.status.success() {
        let _ = std::fs::remove_file(&temp_wav);
        let stderr_excerpt: String = String::from_utf8_lossy(&output.stderr)
            .chars()
            .take(STDERR_EXCERPT_LEN)
            .collect::<String>()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let detail = if stderr_excerpt.is_empty() {
            format!("code {}", output.status.code().unwrap_or(-1))
        } else {
            format!(
                "code {} : {stderr_excerpt}",
                output.status.code().unwrap_or(-1)
            )
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
    use super::{decode_to_samples_with_fallback, ffmpeg_on_path};
    use std::path::{Path, PathBuf};

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name)
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
}
