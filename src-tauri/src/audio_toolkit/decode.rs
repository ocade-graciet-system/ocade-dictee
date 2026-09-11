use std::fs::File;
use std::path::Path;

use anyhow::{anyhow, Context, Result};
use rubato::{FftFixedIn, Resampler};
use symphonia::core::audio::{AudioBufferRef, SampleBuffer, SignalSpec};
use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

use super::constants::WHISPER_SAMPLE_RATE;

// Same fixed chunk size used by `audio_toolkit::audio::resampler::FrameResampler`.
const RESAMPLE_CHUNK_SIZE: usize = 1024;

/// Decode an audio/video file (MP3, MP4/AAC, M4A, WAV, ...) to mono `f32` samples
/// resampled to [`WHISPER_SAMPLE_RATE`] (16 kHz), in the range `[-1, 1]`.
pub fn decode_to_samples(path: &Path) -> Result<Vec<f32>> {
    let file =
        File::open(path).with_context(|| format!("ouverture du fichier {}", path.display()))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .context("format audio/vidéo non reconnu")?;

    let mut format = probed.format;

    // `sample_rate` is only populated for audio tracks, so this also skips
    // any video track present in the container (e.g. an MP4 screen recording).
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL && t.codec_params.sample_rate.is_some())
        .ok_or_else(|| anyhow!("aucune piste audio décodable trouvée"))?;
    let track_id = track.id;
    // Guaranteed by the `.find(...)` filter above, which already checks `is_some()`.
    let source_rate = track.codec_params.sample_rate.unwrap();

    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .context("codec audio non supporté")?;

    let mut mono: Vec<f32> = Vec::new();
    let mut sample_buf: Option<(SignalSpec, SampleBuffer<f32>)> = None;

    loop {
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            Err(SymphoniaError::IoError(err))
                if err.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break;
            }
            Err(SymphoniaError::ResetRequired) => {
                log::warn!("Décodeur : reset requis, arrêt du décodage du flux");
                break;
            }
            Err(err) => return Err(err).context("erreur de lecture du flux audio"),
        };

        if packet.track_id() != track_id {
            continue;
        }

        let decoded = match decoder.decode(&packet) {
            Ok(decoded) => decoded,
            // Recoverable decode errors: skip this packet and keep going.
            Err(SymphoniaError::DecodeError(_)) => {
                log::warn!(
                    "Paquet audio ignoré (erreur de décodage) dans {}",
                    path.display()
                );
                continue;
            }
            Err(err) => return Err(err).context("erreur de décodage audio"),
        };

        push_downmixed(decoded, &mut sample_buf, &mut mono);
    }

    Ok(resample_to_16k(&mono, source_rate))
}

/// Converts a decoded `AudioBufferRef` to interleaved f32 samples (lazily
/// (re)sizing `sample_buf` as needed), then downmixes to mono by averaging
/// channels, appending the result to `out`.
fn push_downmixed(
    decoded: AudioBufferRef,
    sample_buf: &mut Option<(SignalSpec, SampleBuffer<f32>)>,
    out: &mut Vec<f32>,
) {
    let spec = *decoded.spec();
    let channels = spec.channels.count().max(1);
    let duration = decoded.capacity() as u64;

    let buf = match sample_buf {
        Some((buf_spec, buf))
            if *buf_spec == spec && buf.capacity() >= (duration as usize) * channels =>
        {
            buf
        }
        _ => {
            *sample_buf = Some((spec, SampleBuffer::<f32>::new(duration, spec)));
            &mut sample_buf.as_mut().unwrap().1
        }
    };
    buf.copy_interleaved_ref(decoded);

    let interleaved = buf.samples();
    if channels == 1 {
        out.extend_from_slice(interleaved);
        return;
    }

    for frame in interleaved.chunks_exact(channels) {
        let sum: f32 = frame.iter().sum();
        out.push(sum / channels as f32);
    }
}

/// Resample mono `f32` samples from `source_rate` to [`WHISPER_SAMPLE_RATE`].
fn resample_to_16k(mono: &[f32], source_rate: u32) -> Vec<f32> {
    if mono.is_empty() || source_rate == WHISPER_SAMPLE_RATE {
        return mono.to_vec();
    }

    let expected_len =
        ((mono.len() as u64 * WHISPER_SAMPLE_RATE as u64) / source_rate as u64) as usize;

    let mut resampler = FftFixedIn::<f32>::new(
        source_rate as usize,
        WHISPER_SAMPLE_RATE as usize,
        RESAMPLE_CHUNK_SIZE,
        1,
        1,
    )
    .expect("échec de création du rééchantillonneur");

    let mut out: Vec<f32> = Vec::with_capacity(expected_len);
    let mut chunk: Vec<f32> = Vec::with_capacity(RESAMPLE_CHUNK_SIZE);

    for &sample in mono {
        chunk.push(sample);
        if chunk.len() == RESAMPLE_CHUNK_SIZE {
            if let Ok(processed) = resampler.process(&[&chunk[..]], None) {
                out.extend_from_slice(&processed[0]);
            }
            chunk.clear();
        }
    }

    if !chunk.is_empty() {
        // Pad the final partial chunk with silence to reach the fixed input size.
        chunk.resize(RESAMPLE_CHUNK_SIZE, 0.0);
        if let Ok(processed) = resampler.process(&[&chunk[..]], None) {
            out.extend_from_slice(&processed[0]);
        }
    }

    // Trim the tail produced by zero-padding the last chunk.
    out.truncate(expected_len);
    out
}

#[cfg(test)]
mod tests {
    use super::decode_to_samples;
    use std::path::{Path, PathBuf};

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name)
    }

    fn assert_two_seconds_16k(samples: &[f32], name: &str) {
        // ~2 s à 16 kHz → au moins 1 s de contenu, valeurs bornées
        assert!(
            samples.len() >= 16_000,
            "{name}: trop peu d'échantillons: {}",
            samples.len()
        );
        assert!(
            samples.iter().all(|s| s.abs() <= 1.5),
            "{name}: échantillon hors bornes"
        );
    }

    #[test]
    fn decodes_native_formats_to_16k_mono() {
        for name in [
            "sample.mp3",
            "sample.flac",
            "sample.ogg",
            "sample.m4a",
            "sample.aiff",
            "sample.caf",
            "sample.mkv",
        ] {
            let samples =
                decode_to_samples(&fixture(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_two_seconds_16k(&samples, name);
        }
    }

    #[test]
    fn opus_is_not_native() {
        // Opus n'est pas décodé par symphonia : c'est le rôle du repli ffmpeg (voir `audio_toolkit::ffmpeg`).
        assert!(decode_to_samples(&fixture("sample.opus")).is_err());
        assert!(decode_to_samples(&fixture("sample.webm")).is_err());
    }
}
