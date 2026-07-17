//! Goertzel algorithm — single-bin DFT power at a target frequency.
//!
//! O(n) per probed frequency with a trivial inner loop, which beats a full FFT
//! when only a handful of frequencies matter (DTMF probes 8). This is the
//! second "custom FFT-family" implementation in the crate alongside the
//! radix-2 transforms in [`crate::fft`].

use std::f64::consts::PI;

/// Raw Goertzel power of `samples` at `freq_hz`.
///
/// For a pure tone of amplitude A and length n this is approximately
/// (A*n/2)^2. Use [`goertzel_normalized`] for an amplitude-like quantity.
pub fn goertzel_power(samples: &[f64], sample_rate: u32, freq_hz: f64) -> f64 {
    let n = samples.len();
    if n == 0 || sample_rate == 0 {
        return 0.0;
    }
    // Nearest integer bin, the classic formulation.
    let k = (0.5 + n as f64 * freq_hz / sample_rate as f64).floor();
    let omega = 2.0 * PI * k / n as f64;
    let coeff = 2.0 * omega.cos();

    let mut s_prev = 0.0f64;
    let mut s_prev2 = 0.0f64;
    for &x in samples {
        let s = x + coeff * s_prev - s_prev2;
        s_prev2 = s_prev;
        s_prev = s;
    }
    s_prev * s_prev + s_prev2 * s_prev2 - coeff * s_prev * s_prev2
}

/// Goertzel power normalized to tone amplitude: ~A^2 for a pure tone of
/// amplitude A regardless of frame length.
pub fn goertzel_normalized(samples: &[f64], sample_rate: u32, freq_hz: f64) -> f64 {
    let n = samples.len();
    if n == 0 {
        return 0.0;
    }
    let scale = (n as f64 / 2.0) * (n as f64 / 2.0);
    goertzel_power(samples, sample_rate, freq_hz) / scale
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(freq: f64, amp: f64, rate: u32, n: usize) -> Vec<f64> {
        (0..n)
            .map(|i| amp * (2.0 * PI * freq * i as f64 / rate as f64).sin())
            .collect()
    }

    #[test]
    fn detects_present_tone_and_rejects_absent() {
        let rate = 8000;
        let samples = tone(770.0, 1.0, rate, 205);
        let present = goertzel_normalized(&samples, rate, 770.0);
        let absent = goertzel_normalized(&samples, rate, 1477.0);
        assert!(present > 0.5, "present tone power {present}");
        assert!(absent < 0.05, "absent tone power {absent}");
        assert!(present > absent * 20.0);
    }

    #[test]
    fn amplitude_normalization_is_stable_across_lengths() {
        // 500 Hz lands exactly on an integer bin for each of these lengths at
        // 8 kHz, so there is no scalloping loss and p ~ A^2 = 0.25.
        let rate = 8000;
        for &n in &[160usize, 400, 800] {
            let p = goertzel_normalized(&tone(500.0, 0.5, rate, n), rate, 500.0);
            assert!((p - 0.25).abs() < 0.05, "n={n} p={p}");
        }
    }

    #[test]
    fn off_bin_tone_suffers_bounded_scalloping_loss() {
        // 697 Hz at n=800 sits 3 Hz off bin 70 (700 Hz); the detector must
        // still see most of the energy — this is what DTMF detection relies on.
        let rate = 8000;
        let p = goertzel_normalized(&tone(697.0, 0.5, rate, 800), rate, 697.0);
        assert!(p > 0.12 && p < 0.25, "p={p}");
    }

    #[test]
    fn empty_input_is_zero() {
        assert_eq!(goertzel_power(&[], 8000, 440.0), 0.0);
    }
}
