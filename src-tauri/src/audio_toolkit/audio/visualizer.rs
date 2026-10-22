use rustfft::{num_complex::Complex32, Fft, FftPlanner};
use std::sync::Arc;

const DB_MIN: f32 = -55.0;
const DB_MAX: f32 = -8.0;
const GAIN: f32 = 1.3;
const CURVE_POWER: f32 = 0.7;

// Sensibilité automatique (OCADE) : un micro peu sensible, fréquent sous
// Windows, place la voix sous la plage fixe ci-dessus et les barres restent
// plates. La plage adaptative part du bruit de fond mesuré et monte jusqu'au
// pic récent de la voix ; on retient la plus réactive des deux plages, si bien
// qu'un micro déjà bien réglé (Mac) n'y perd rien.
/// Marge au-dessus du bruit de fond avant de faire bouger une barre.
const ADAPTIVE_FLOOR_MARGIN_DB: f32 = 6.0;
/// Plage minimale de la sensibilité automatique : en dessous, le moindre
/// souffle remplirait les barres.
const ADAPTIVE_MIN_SPAN_DB: f32 = 18.0;
/// Décroissance du pic de voix retenu, par trame (~30 trames par seconde).
const PEAK_DECAY_DB: f32 = 0.05;
/// Plancher du bruit de fond mesuré : aucun micro réel ne descend plus bas, et
/// sans cette limite un silence numérique ferait réagir la moindre poussière.
const NOISE_FLOOR_MIN_DB: f32 = -75.0;

pub struct AudioVisualiser {
    fft: Arc<dyn Fft<f32>>,
    window: Vec<f32>,
    bucket_ranges: Vec<(usize, usize)>,
    fft_input: Vec<Complex32>,
    noise_floor: Vec<f32>,
    /// Pic récent du signal (dB), haut de la plage adaptative.
    peak_db: f32,
    buffer: Vec<f32>,
    window_size: usize,
    buckets: usize,
}

impl AudioVisualiser {
    pub fn new(
        sample_rate: u32,
        window_size: usize,
        buckets: usize,
        freq_min: f32,
        freq_max: f32,
    ) -> Self {
        let mut planner = FftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(window_size);

        // Pre-compute Hann window
        let window: Vec<f32> = (0..window_size)
            .map(|i| {
                0.5 * (1.0 - (2.0 * std::f32::consts::PI * i as f32 / window_size as f32).cos())
            })
            .collect();

        // Pre-compute bucket frequency ranges
        let nyquist = sample_rate as f32 / 2.0;
        let freq_min = freq_min.min(nyquist);
        let freq_max = freq_max.min(nyquist);

        let mut bucket_ranges = Vec::with_capacity(buckets);

        for b in 0..buckets {
            // Use logarithmic spacing for better perceptual representation
            let log_start = (b as f32 / buckets as f32).powi(2);
            let log_end = ((b + 1) as f32 / buckets as f32).powi(2);

            let start_hz = freq_min + (freq_max - freq_min) * log_start;
            let end_hz = freq_min + (freq_max - freq_min) * log_end;

            let start_bin = ((start_hz * window_size as f32) / sample_rate as f32) as usize;
            let mut end_bin = ((end_hz * window_size as f32) / sample_rate as f32) as usize;

            // Ensure each bucket has at least one bin
            if end_bin <= start_bin {
                end_bin = start_bin + 1;
            }

            // Clamp to valid range
            let start_bin = start_bin.min(window_size / 2);
            let end_bin = end_bin.min(window_size / 2);

            bucket_ranges.push((start_bin, end_bin));
        }

        Self {
            fft,
            window,
            bucket_ranges,
            fft_input: vec![Complex32::new(0.0, 0.0); window_size],
            noise_floor: vec![-40.0; buckets], // Initialize to reasonable noise floor
            peak_db: DB_MIN,
            buffer: Vec::with_capacity(window_size * 2),
            window_size,
            buckets,
        }
    }

    pub fn feed(&mut self, samples: &[f32]) -> Option<Vec<f32>> {
        // Add new samples to buffer
        self.buffer.extend_from_slice(samples);

        // Only process if we have enough samples
        if self.buffer.len() < self.window_size {
            return None;
        }

        // Take the required window of samples
        let window_samples = &self.buffer[..self.window_size];

        // Remove DC component
        let mean = window_samples.iter().sum::<f32>() / self.window_size as f32;

        // Apply window function and prepare FFT input
        for (i, &sample) in window_samples.iter().enumerate() {
            let windowed_sample = (sample - mean) * self.window[i];
            self.fft_input[i] = Complex32::new(windowed_sample, 0.0);
        }

        // Perform FFT
        self.fft.process(&mut self.fft_input);

        // Compute power spectrum and bucket levels
        let mut buckets = vec![0.0; self.buckets];
        let mut frame_peak_db = f32::MIN;

        for (bucket_idx, &(start_bin, end_bin)) in self.bucket_ranges.iter().enumerate() {
            if start_bin >= end_bin || end_bin > self.fft_input.len() / 2 {
                continue;
            }

            // Calculate average power in this frequency range
            let mut power_sum = 0.0;
            for bin_idx in start_bin..end_bin {
                let magnitude = self.fft_input[bin_idx].norm();
                power_sum += magnitude * magnitude;
            }

            let avg_power = power_sum / (end_bin - start_bin) as f32;

            // Convert to dB with proper scaling
            let db = if avg_power > 1e-12 {
                20.0 * (avg_power.sqrt() / self.window_size as f32).log10()
            } else {
                -80.0 // Very low floor for zero power
            };

            // Only update noise floor when signal is quiet (below current floor + 10dB).
            // A quieter signal pulls it down quickly: a low-gain mic sits far below
            // the initial -40 dB guess, and the adaptive range starts from here.
            if db < self.noise_floor[bucket_idx] {
                const NOISE_DROP_ALPHA: f32 = 0.1;
                self.noise_floor[bucket_idx] = (NOISE_DROP_ALPHA * db
                    + (1.0 - NOISE_DROP_ALPHA) * self.noise_floor[bucket_idx])
                    .max(NOISE_FLOOR_MIN_DB);
            } else if db < self.noise_floor[bucket_idx] + 10.0 {
                const NOISE_ALPHA: f32 = 0.001; // Very slow adaptation
                self.noise_floor[bucket_idx] =
                    NOISE_ALPHA * db + (1.0 - NOISE_ALPHA) * self.noise_floor[bucket_idx];
            }
            frame_peak_db = frame_peak_db.max(db);

            // Map configurable dB range to 0-1 with gain and curve shaping, or the
            // adaptive range (noise floor → recent peak) when it reacts more.
            let fixed = ((db - DB_MIN) / (DB_MAX - DB_MIN)).clamp(0.0, 1.0);
            let low = self.noise_floor[bucket_idx] + ADAPTIVE_FLOOR_MARGIN_DB;
            let high = self.peak_db.max(low + ADAPTIVE_MIN_SPAN_DB);
            let adaptive = ((db - low) / (high - low)).clamp(0.0, 1.0);
            let normalized = fixed.max(adaptive);
            buckets[bucket_idx] = (normalized * GAIN).powf(CURVE_POWER).clamp(0.0, 1.0);
        }

        // Le pic retenu suit la voix aussitôt et redescend lentement.
        self.peak_db = (self.peak_db - PEAK_DECAY_DB).max(frame_peak_db);

        // Apply light smoothing to reduce jitter
        for i in 1..buckets.len() - 1 {
            buckets[i] = buckets[i] * 0.7 + buckets[i - 1] * 0.15 + buckets[i + 1] * 0.15;
        }

        // Clear processed samples from buffer
        self.buffer.clear();

        Some(buckets)
    }

    pub fn reset(&mut self) {
        self.buffer.clear();
        // Reset noise floor to initial values
        self.noise_floor.fill(-40.0);
        self.peak_db = DB_MIN;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;
    const WINDOW: usize = 2048;

    /// Signal de parole schématique : quelques harmoniques vocales, d'amplitude
    /// crête `amplitude`.
    fn voice(amplitude: f32, offset: usize) -> Vec<f32> {
        (0..WINDOW)
            .map(|i| {
                let t = (offset + i) as f32 / RATE as f32;
                let s: f32 = [220.0f32, 440.0, 880.0, 1760.0]
                    .iter()
                    .map(|f| (2.0 * std::f32::consts::PI * f * t).sin())
                    .sum();
                amplitude * s / 4.0
            })
            .collect()
    }

    /// Bruit blanc uniforme (générateur congruentiel), reproductible.
    fn noise(amplitude: f32, seed: usize) -> Vec<f32> {
        let mut state = (seed as u64).wrapping_mul(6_364_136_223_846_793_005) | 1;
        (0..WINDOW)
            .map(|_| {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                let u = (state >> 40) as f32 / (1u64 << 24) as f32;
                amplitude * (2.0 * u - 1.0)
            })
            .collect()
    }

    fn max_level(levels: &[f32]) -> f32 {
        levels.iter().cloned().fold(0.0, f32::max)
    }

    #[test]
    fn quiet_microphone_still_moves_the_bars() {
        let mut v = AudioVisualiser::new(RATE, WINDOW, 16, 80.0, 4000.0);
        // Une seconde de silence ambiant très bas (micro peu sensible)…
        for k in 0..30 {
            v.feed(&noise(0.0005, k * WINDOW));
        }
        // …puis une voix faible, que la plage fixe laissait complètement à plat
        // (chaque harmonique vers -58 dB, sous DB_MIN).
        let mut last = Vec::new();
        for k in 0..15 {
            last = v.feed(&voice(0.02, k * WINDOW)).unwrap();
        }
        assert!(
            max_level(&last) > 0.4,
            "voix faible : barres trop basses ({:?})",
            max_level(&last)
        );
        // Retour au silence : les barres retombent.
        let mut quiet = Vec::new();
        for k in 0..30 {
            quiet = v.feed(&noise(0.0005, k * WINDOW)).unwrap();
        }
        assert!(
            max_level(&quiet) < 0.35,
            "silence : {:?}",
            max_level(&quiet)
        );
    }

    #[test]
    fn loud_microphone_is_not_less_reactive_than_before() {
        let mut v = AudioVisualiser::new(RATE, WINDOW, 16, 80.0, 4000.0);
        // Référence : 0,48 avec la seule plage fixe (comportement d'avant).
        let levels = v.feed(&voice(0.3, 0)).unwrap();
        assert!(max_level(&levels) >= 0.48, "{:?}", max_level(&levels));
    }
}
