//! Energy-based voice activity detection: RMS levels and silence trimming.
//! Used to strip dead air before shipping audio to the STT provider.

/// Root-mean-square level of a frame.
pub fn rms(samples: &[f64]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples.iter().map(|x| x * x).sum::<f64>() / samples.len() as f64).sqrt()
}

/// RMS expressed in dBFS (0 dB = full-scale sine RMS would be -3 dB; here
/// full-scale square = 0 dB). Silence clamps to -120.
pub fn rms_dbfs(samples: &[f64]) -> f64 {
    let r = rms(samples);
    if r <= 1e-6 {
        -120.0
    } else {
        20.0 * r.log10()
    }
}

/// Trim leading and trailing silence.
///
/// Frames of `frame_ms` are classified against `threshold_db` (dBFS); the clip
/// is cut at the first/last active frame with `padding_ms` of context kept on
/// each side. A fully-silent clip returns an empty vec.
pub fn trim_silence(
    samples: &[f64],
    sample_rate: u32,
    threshold_db: f64,
    frame_ms: u32,
    padding_ms: u32,
) -> Vec<f64> {
    if samples.is_empty() || sample_rate == 0 {
        return Vec::new();
    }
    let frame_len = ((sample_rate * frame_ms) as usize / 1000).max(1);
    let n_frames = samples.len().div_ceil(frame_len);

    let mut first_active: Option<usize> = None;
    let mut last_active: Option<usize> = None;
    for i in 0..n_frames {
        let start = i * frame_len;
        let end = (start + frame_len).min(samples.len());
        if rms_dbfs(&samples[start..end]) > threshold_db {
            if first_active.is_none() {
                first_active = Some(i);
            }
            last_active = Some(i);
        }
    }

    let (Some(first), Some(last)) = (first_active, last_active) else {
        return Vec::new();
    };

    let pad = (sample_rate * padding_ms) as usize / 1000;
    let start = (first * frame_len).saturating_sub(pad);
    let end = (((last + 1) * frame_len) + pad).min(samples.len());
    samples[start..end].to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    #[test]
    fn trims_leading_and_trailing_silence() {
        let rate = 8000u32;
        let silence = vec![0.0f64; 8000]; // 1 s
        let tone: Vec<f64> = (0..8000)
            .map(|i| 0.5 * (2.0 * PI * 440.0 * i as f64 / rate as f64).sin())
            .collect();
        let mut clip = silence.clone();
        clip.extend(&tone);
        clip.extend(&silence);

        let trimmed = trim_silence(&clip, rate, -40.0, 20, 100);
        // ~1 s of tone + <=100 ms padding either side, frame-quantized.
        assert!(
            trimmed.len() >= 8000 && trimmed.len() <= 8000 + 2 * 800 + 2 * 160,
            "trimmed to {}",
            trimmed.len()
        );
        assert!(rms_dbfs(&trimmed) > -20.0);
    }

    #[test]
    fn all_silence_trims_to_empty() {
        assert!(trim_silence(&vec![0.0; 4000], 8000, -40.0, 20, 100).is_empty());
    }

    #[test]
    fn all_signal_is_untouched_except_bounds() {
        let rate = 8000u32;
        let tone: Vec<f64> = (0..4000)
            .map(|i| 0.5 * (2.0 * PI * 300.0 * i as f64 / rate as f64).sin())
            .collect();
        let trimmed = trim_silence(&tone, rate, -40.0, 20, 100);
        assert_eq!(trimmed.len(), tone.len());
    }

    #[test]
    fn dbfs_of_full_scale_square_is_zero() {
        let square = vec![1.0f64; 1000];
        assert!(rms_dbfs(&square).abs() < 1e-9);
    }
}
