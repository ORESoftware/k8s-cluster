//! STFT and spectral features built on the custom FFT planner.

use crate::complex::Complex;
use crate::dtmf;
use crate::fft::{magnitude_spectrum, FftError, FftPlanner};
use crate::vad::{rms_dbfs, trim_silence};
use crate::window::Window;

/// Magnitude spectrogram: frames of single-sided FFT magnitudes.
#[derive(Debug, Clone, PartialEq)]
pub struct Spectrogram {
    pub fft_size: usize,
    pub hop: usize,
    pub sample_rate: u32,
    /// frames[t][k] = magnitude of bin k (k in 0..=fft_size/2) at frame t.
    pub frames: Vec<Vec<f64>>,
}

impl Spectrogram {
    pub fn bin_freq(&self, k: usize) -> f64 {
        k as f64 * self.sample_rate as f64 / self.fft_size as f64
    }
}

/// Short-time Fourier transform using one reusable [`FftPlanner`].
pub fn stft(
    samples: &[f64],
    sample_rate: u32,
    fft_size: usize,
    hop: usize,
    window: Window,
) -> Result<Spectrogram, FftError> {
    if fft_size == 0 || !fft_size.is_power_of_two() {
        return Err(FftError::NonPowerOfTwo(fft_size));
    }
    let hop = hop.max(1);
    let planner = FftPlanner::new(fft_size)?;
    let coeffs = window.coefficients(fft_size);
    let mut frames = Vec::new();
    let mut buf = vec![Complex::ZERO; fft_size];

    let mut start = 0usize;
    while start < samples.len() {
        let end = (start + fft_size).min(samples.len());
        for (i, slot) in buf.iter_mut().enumerate() {
            let s = if start + i < end {
                samples[start + i]
            } else {
                0.0
            };
            *slot = Complex::new(s * coeffs[i], 0.0);
        }
        planner.forward(&mut buf)?;
        let mags: Vec<f64> = buf
            .iter()
            .take(fft_size / 2 + 1)
            .map(|c| 2.0 * c.abs() / fft_size as f64)
            .collect();
        frames.push(mags);
        start += hop;
    }

    Ok(Spectrogram {
        fft_size,
        hop,
        sample_rate,
        frames,
    })
}

/// Frequency of the strongest non-DC bin.
pub fn dominant_frequency(samples: &[f64], sample_rate: u32) -> Result<(f64, f64), FftError> {
    let bins = magnitude_spectrum(samples, sample_rate)?;
    let peak = bins
        .iter()
        .skip(1) // ignore DC
        .max_by(|a, b| a.magnitude.total_cmp(&b.magnitude))
        .ok_or(FftError::Empty)?;
    Ok((peak.freq_hz, peak.magnitude))
}

/// Amplitude-weighted mean frequency of the spectrum.
pub fn spectral_centroid(samples: &[f64], sample_rate: u32) -> Result<f64, FftError> {
    let bins = magnitude_spectrum(samples, sample_rate)?;
    let total: f64 = bins.iter().map(|b| b.magnitude).sum();
    if total <= 0.0 {
        return Ok(0.0);
    }
    Ok(bins.iter().map(|b| b.freq_hz * b.magnitude).sum::<f64>() / total)
}

/// Full spectral/temporal summary of a clip — what the API's `/analyze`
/// endpoint returns. Plain data; serialization happens at the API layer.
#[derive(Debug, Clone, PartialEq)]
pub struct AudioAnalysis {
    pub sample_rate: u32,
    pub num_samples: usize,
    pub duration_secs: f64,
    pub rms_dbfs: f64,
    pub dominant_freq_hz: f64,
    pub dominant_magnitude: f64,
    pub spectral_centroid_hz: f64,
    pub dtmf_digits: String,
    /// Duration after VAD trimming at -40 dBFS.
    pub voiced_duration_secs: f64,
    /// Downsampled single-sided spectrum for visualization (freq, magnitude).
    pub spectrum_preview: Vec<(f64, f64)>,
}

/// Number of bins in `spectrum_preview`.
const PREVIEW_BINS: usize = 64;

pub fn analyze(samples: &[f64], sample_rate: u32) -> Result<AudioAnalysis, FftError> {
    if samples.is_empty() {
        return Err(FftError::Empty);
    }
    let (dominant_freq_hz, dominant_magnitude) = dominant_frequency(samples, sample_rate)?;
    let centroid = spectral_centroid(samples, sample_rate)?;
    let trimmed = trim_silence(samples, sample_rate, -40.0, 20, 0);

    let bins = magnitude_spectrum(samples, sample_rate)?;
    let chunk = (bins.len().div_ceil(PREVIEW_BINS)).max(1);
    let spectrum_preview: Vec<(f64, f64)> = bins
        .chunks(chunk)
        .map(|group| {
            let freq = group[group.len() / 2].freq_hz;
            let mag = group.iter().map(|b| b.magnitude).fold(0.0f64, f64::max);
            (freq, mag)
        })
        .collect();

    Ok(AudioAnalysis {
        sample_rate,
        num_samples: samples.len(),
        duration_secs: samples.len() as f64 / sample_rate as f64,
        rms_dbfs: rms_dbfs(samples),
        dominant_freq_hz,
        dominant_magnitude,
        spectral_centroid_hz: centroid,
        dtmf_digits: dtmf::detect_sequence(samples, sample_rate),
        voiced_duration_secs: trimmed.len() as f64 / sample_rate as f64,
        spectrum_preview,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    fn sine(freq: f64, rate: u32, n: usize) -> Vec<f64> {
        (0..n)
            .map(|i| 0.8 * (2.0 * PI * freq * i as f64 / rate as f64).sin())
            .collect()
    }

    #[test]
    fn stft_frame_count_and_peak_bin() {
        let rate = 8000;
        let samples = sine(1000.0, rate, 4096);
        let spec = stft(&samples, rate, 512, 256, Window::Hann).unwrap();
        assert_eq!(spec.frames.len(), 4096usize.div_ceil(256));
        // Peak bin of a mid-clip frame should sit at ~1000 Hz.
        let frame = &spec.frames[4];
        let peak = frame
            .iter()
            .enumerate()
            .skip(1)
            .max_by(|a, b| a.1.total_cmp(b.1))
            .unwrap()
            .0;
        assert!((spec.bin_freq(peak) - 1000.0).abs() < spec.bin_freq(1) + 1.0);
    }

    #[test]
    fn dominant_frequency_finds_the_sine() {
        let (f, m) = dominant_frequency(&sine(440.0, 16000, 16384), 16000).unwrap();
        assert!((f - 440.0).abs() < 1.5, "dominant {f}");
        assert!(m > 0.5);
    }

    #[test]
    fn centroid_sits_between_two_tones() {
        let rate = 8000;
        let mut samples = sine(500.0, rate, 8192);
        for (i, s) in samples.iter_mut().enumerate() {
            *s += 0.8 * (2.0 * PI * 1500.0 * i as f64 / rate as f64).sin();
        }
        let c = spectral_centroid(&samples, rate).unwrap();
        assert!(c > 600.0 && c < 1400.0, "centroid {c}");
    }

    #[test]
    fn analyze_summarizes_a_clip() {
        let rate = 8000;
        let samples = sine(700.0, rate, 8000);
        let a = analyze(&samples, rate).unwrap();
        assert_eq!(a.sample_rate, rate);
        assert!((a.duration_secs - 1.0).abs() < 1e-9);
        assert!((a.dominant_freq_hz - 700.0).abs() < 3.0);
        assert!(a.rms_dbfs > -6.0 && a.rms_dbfs < 0.0);
        assert!(!a.spectrum_preview.is_empty() && a.spectrum_preview.len() <= 65);
        assert_eq!(a.dtmf_digits, "");
        assert!(a.voiced_duration_secs > 0.9);
    }
}
