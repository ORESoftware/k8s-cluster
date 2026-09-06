//! t2v-core — zero-dependency DSP core for the t2v-v2t platform.
//!
//! Everything in this crate is hand-rolled (no rustfft/num-complex/hound):
//! custom FFT implementations (naive DFT, recursive and iterative
//! Cooley-Tukey), Goertzel single-bin detection with a DTMF decoder on top,
//! a WAV codec, G.711 mu-law, linear resampling, energy-based VAD, and
//! STFT/spectral features built on the FFTs.

pub mod audio;
pub mod complex;
pub mod dtmf;
pub mod fft;
pub mod goertzel;
pub mod spectrum;
pub mod vad;
pub mod window;

pub use complex::Complex;
pub use fft::{
    dft_naive, fft_real_padded, fft_recursive, magnitude_spectrum, FftError, FftPlanner,
    SpectrumBin,
};
