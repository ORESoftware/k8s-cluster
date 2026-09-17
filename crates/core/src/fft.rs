//! Hand-rolled FFT implementations — no rustfft, no num-complex.
//!
//! Three algorithms, all exercised against each other in tests:
//!
//! * [`dft_naive`] — textbook O(n^2) DFT, the correctness reference.
//! * [`fft_recursive`] — allocating radix-2 Cooley-Tukey, the readable one.
//! * [`FftPlanner`] — iterative in-place radix-2 with bit-reversal permutation
//!   and a precomputed twiddle table. This is the production path: O(n log n),
//!   zero allocation per transform after planning.
//!
//! Sizes must be powers of two for the fast paths; [`fft_real_padded`] zero-pads
//! arbitrary-length real signals up to the next power of two.

use crate::complex::Complex;
use std::f64::consts::PI;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FftError {
    /// The fast transforms only handle power-of-two lengths.
    NonPowerOfTwo(usize),
    /// Planner was built for one size but handed a buffer of another.
    SizeMismatch {
        planned: usize,
        got: usize,
    },
    Empty,
}

impl std::fmt::Display for FftError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FftError::NonPowerOfTwo(n) => write!(f, "fft length {n} is not a power of two"),
            FftError::SizeMismatch { planned, got } => {
                write!(
                    f,
                    "planner built for length {planned}, buffer has length {got}"
                )
            }
            FftError::Empty => write!(f, "fft input is empty"),
        }
    }
}

impl std::error::Error for FftError {}

/// Smallest power of two >= n (n >= 1).
pub fn next_pow2(n: usize) -> usize {
    n.next_power_of_two()
}

/// Textbook O(n^2) discrete Fourier transform. Reference implementation used
/// by the test suite to validate the fast paths; also fine for tiny inputs.
pub fn dft_naive(input: &[Complex]) -> Vec<Complex> {
    let n = input.len();
    let mut out = vec![Complex::ZERO; n];
    for (k, out_k) in out.iter_mut().enumerate() {
        let mut acc = Complex::ZERO;
        for (t, &x) in input.iter().enumerate() {
            let angle = -2.0 * PI * (k as f64) * (t as f64) / (n as f64);
            acc += x * Complex::from_polar(1.0, angle);
        }
        *out_k = acc;
    }
    out
}

/// Recursive radix-2 Cooley-Tukey. Allocates O(n log n) intermediates; kept for
/// clarity and as a second independent implementation to cross-check the
/// iterative one.
pub fn fft_recursive(input: &[Complex]) -> Result<Vec<Complex>, FftError> {
    let n = input.len();
    if n == 0 {
        return Err(FftError::Empty);
    }
    if !n.is_power_of_two() {
        return Err(FftError::NonPowerOfTwo(n));
    }
    Ok(fft_recursive_inner(input))
}

fn fft_recursive_inner(input: &[Complex]) -> Vec<Complex> {
    let n = input.len();
    if n == 1 {
        return vec![input[0]];
    }
    let even: Vec<Complex> = input.iter().step_by(2).copied().collect();
    let odd: Vec<Complex> = input.iter().skip(1).step_by(2).copied().collect();
    let even_fft = fft_recursive_inner(&even);
    let odd_fft = fft_recursive_inner(&odd);

    let mut out = vec![Complex::ZERO; n];
    for k in 0..n / 2 {
        let twiddle = Complex::from_polar(1.0, -2.0 * PI * (k as f64) / (n as f64));
        let t = twiddle * odd_fft[k];
        out[k] = even_fft[k] + t;
        out[k + n / 2] = even_fft[k] - t;
    }
    out
}

/// Iterative in-place radix-2 FFT with a precomputed twiddle table.
///
/// Build once per size, reuse for every frame — this is what the STFT and the
/// analysis endpoints use. Twiddles are computed directly from sin/cos per
/// index (not by repeated multiplication), so there is no accumulated phase
/// drift across large transforms.
pub struct FftPlanner {
    n: usize,
    /// twiddles[k] = exp(-2*pi*i*k/n) for k in 0..n/2
    twiddles: Vec<Complex>,
}

impl FftPlanner {
    pub fn new(n: usize) -> Result<Self, FftError> {
        if n == 0 {
            return Err(FftError::Empty);
        }
        if !n.is_power_of_two() {
            return Err(FftError::NonPowerOfTwo(n));
        }
        let twiddles = (0..n / 2)
            .map(|k| Complex::from_polar(1.0, -2.0 * PI * (k as f64) / (n as f64)))
            .collect();
        Ok(Self { n, twiddles })
    }

    pub fn len(&self) -> usize {
        self.n
    }

    pub fn is_empty(&self) -> bool {
        self.n == 0
    }

    /// Forward transform, in place.
    pub fn forward(&self, buf: &mut [Complex]) -> Result<(), FftError> {
        self.check(buf)?;
        bit_reverse_permute(buf);
        self.butterflies(buf, false);
        Ok(())
    }

    /// Inverse transform, in place, including the 1/n scaling.
    pub fn inverse(&self, buf: &mut [Complex]) -> Result<(), FftError> {
        self.check(buf)?;
        bit_reverse_permute(buf);
        self.butterflies(buf, true);
        let scale = 1.0 / (self.n as f64);
        for v in buf.iter_mut() {
            *v = v.scale(scale);
        }
        Ok(())
    }

    fn check(&self, buf: &[Complex]) -> Result<(), FftError> {
        if buf.len() != self.n {
            return Err(FftError::SizeMismatch {
                planned: self.n,
                got: buf.len(),
            });
        }
        Ok(())
    }

    fn butterflies(&self, buf: &mut [Complex], inverse: bool) {
        let n = self.n;
        let mut len = 2;
        while len <= n {
            let half = len / 2;
            let stride = n / len; // index step into the size-n twiddle table
            for start in (0..n).step_by(len) {
                for k in 0..half {
                    let mut w = self.twiddles[k * stride];
                    if inverse {
                        w = w.conj();
                    }
                    let a = buf[start + k];
                    let b = buf[start + k + half] * w;
                    buf[start + k] = a + b;
                    buf[start + k + half] = a - b;
                }
            }
            len <<= 1;
        }
    }
}

/// In-place bit-reversal permutation (n must be a power of two).
fn bit_reverse_permute(buf: &mut [Complex]) {
    let n = buf.len();
    if n <= 1 {
        return;
    }
    let bits = n.trailing_zeros();
    for i in 0..n {
        let j = i.reverse_bits() >> (usize::BITS - bits);
        if j > i {
            buf.swap(i, j);
        }
    }
}

/// FFT of a real signal, zero-padded to the next power of two.
/// Returns the full complex spectrum (length = padded size).
pub fn fft_real_padded(samples: &[f64]) -> Result<Vec<Complex>, FftError> {
    if samples.is_empty() {
        return Err(FftError::Empty);
    }
    let n = next_pow2(samples.len());
    let mut buf: Vec<Complex> = Vec::with_capacity(n);
    buf.extend(samples.iter().map(|&s| Complex::new(s, 0.0)));
    buf.resize(n, Complex::ZERO);
    let planner = FftPlanner::new(n)?;
    planner.forward(&mut buf)?;
    Ok(buf)
}

/// One bin of a single-sided magnitude spectrum.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpectrumBin {
    pub freq_hz: f64,
    pub magnitude: f64,
}

/// Single-sided magnitude spectrum of a real signal (bins 0..=n/2), with
/// magnitudes normalized by n so amplitude is independent of transform size.
pub fn magnitude_spectrum(samples: &[f64], sample_rate: u32) -> Result<Vec<SpectrumBin>, FftError> {
    let spectrum = fft_real_padded(samples)?;
    let n = spectrum.len();
    let bin_width = sample_rate as f64 / n as f64;
    Ok(spectrum
        .iter()
        .take(n / 2 + 1)
        .enumerate()
        .map(|(k, c)| SpectrumBin {
            freq_hz: k as f64 * bin_width,
            magnitude: 2.0 * c.abs() / n as f64,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic pseudo-random signal (no rand dependency).
    fn lcg_signal(len: usize, seed: u64) -> Vec<Complex> {
        let mut state = seed.max(1);
        (0..len)
            .map(|_| {
                state = state
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                let re = ((state >> 33) as f64 / (1u64 << 31) as f64) - 1.0;
                state = state
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                let im = ((state >> 33) as f64 / (1u64 << 31) as f64) - 1.0;
                Complex::new(re, im)
            })
            .collect()
    }

    fn max_err(a: &[Complex], b: &[Complex]) -> f64 {
        a.iter()
            .zip(b.iter())
            .map(|(x, y)| (*x - *y).abs())
            .fold(0.0, f64::max)
    }

    #[test]
    fn all_three_implementations_agree() {
        for &n in &[1usize, 2, 4, 8, 64, 256, 1024] {
            let signal = lcg_signal(n, 42);
            let reference = dft_naive(&signal);
            let recursive = fft_recursive(&signal).unwrap();

            let planner = FftPlanner::new(n).unwrap();
            let mut iterative = signal.clone();
            planner.forward(&mut iterative).unwrap();

            let tol = 1e-8 * (n as f64);
            assert!(
                max_err(&reference, &recursive) < tol,
                "recursive diverges at n={n}"
            );
            assert!(
                max_err(&reference, &iterative) < tol,
                "iterative diverges at n={n}"
            );
        }
    }

    #[test]
    fn forward_then_inverse_roundtrips() {
        let n = 512;
        let signal = lcg_signal(n, 7);
        let planner = FftPlanner::new(n).unwrap();
        let mut buf = signal.clone();
        planner.forward(&mut buf).unwrap();
        planner.inverse(&mut buf).unwrap();
        assert!(max_err(&signal, &buf) < 1e-9, "ifft(fft(x)) != x");
    }

    #[test]
    fn impulse_transforms_to_flat_spectrum() {
        let n = 64;
        let mut buf = vec![Complex::ZERO; n];
        buf[0] = Complex::ONE;
        let planner = FftPlanner::new(n).unwrap();
        planner.forward(&mut buf).unwrap();
        for c in &buf {
            assert!((c.re - 1.0).abs() < 1e-12 && c.im.abs() < 1e-12);
        }
    }

    #[test]
    fn parseval_energy_is_conserved() {
        let n = 256;
        let signal = lcg_signal(n, 99);
        let time_energy: f64 = signal.iter().map(|c| c.norm_sq()).sum();
        let planner = FftPlanner::new(n).unwrap();
        let mut buf = signal;
        planner.forward(&mut buf).unwrap();
        let freq_energy: f64 = buf.iter().map(|c| c.norm_sq()).sum::<f64>() / n as f64;
        assert!(
            (time_energy - freq_energy).abs() < 1e-7 * time_energy,
            "parseval violated: {time_energy} vs {freq_energy}"
        );
    }

    #[test]
    fn sine_peaks_at_expected_bin() {
        let sample_rate = 8000u32;
        let n = 1024usize;
        let freq = 1000.0; // exactly bin 128 at 8 kHz / 1024
        let samples: Vec<f64> = (0..n)
            .map(|i| (2.0 * PI * freq * i as f64 / sample_rate as f64).sin())
            .collect();
        let bins = magnitude_spectrum(&samples, sample_rate).unwrap();
        let peak = bins
            .iter()
            .max_by(|a, b| a.magnitude.total_cmp(&b.magnitude))
            .unwrap();
        assert!((peak.freq_hz - freq).abs() < 1e-6);
        // Full-scale sine, normalized: peak magnitude ~1.0
        assert!(
            (peak.magnitude - 1.0).abs() < 1e-6,
            "peak mag {}",
            peak.magnitude
        );
    }

    #[test]
    fn rejects_non_power_of_two() {
        assert!(matches!(
            FftPlanner::new(48),
            Err(FftError::NonPowerOfTwo(48))
        ));
        assert!(matches!(
            fft_recursive(&[Complex::ONE; 3]),
            Err(FftError::NonPowerOfTwo(3))
        ));
    }

    #[test]
    fn planner_rejects_wrong_buffer_size() {
        let planner = FftPlanner::new(8).unwrap();
        let mut buf = vec![Complex::ZERO; 4];
        assert!(matches!(
            planner.forward(&mut buf),
            Err(FftError::SizeMismatch { planned: 8, got: 4 })
        ));
    }
}
