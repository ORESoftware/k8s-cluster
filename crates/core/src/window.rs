//! Analysis window functions for the STFT / spectral paths.

use std::f64::consts::PI;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Window {
    Rect,
    Hann,
    Hamming,
    Blackman,
}

impl Window {
    /// Window coefficients of length `n` (symmetric form).
    pub fn coefficients(self, n: usize) -> Vec<f64> {
        if n == 0 {
            return Vec::new();
        }
        if n == 1 {
            return vec![1.0];
        }
        let denom = (n - 1) as f64;
        (0..n)
            .map(|i| {
                let x = i as f64 / denom;
                match self {
                    Window::Rect => 1.0,
                    Window::Hann => 0.5 - 0.5 * (2.0 * PI * x).cos(),
                    Window::Hamming => 0.54 - 0.46 * (2.0 * PI * x).cos(),
                    Window::Blackman => {
                        0.42 - 0.5 * (2.0 * PI * x).cos() + 0.08 * (4.0 * PI * x).cos()
                    }
                }
            })
            .collect()
    }

    /// Multiply `frame` by the window in place. Frames longer than the
    /// coefficient table are an error at the call site; this asserts equality.
    pub fn apply(self, frame: &mut [f64]) {
        let coeffs = self.coefficients(frame.len());
        for (s, w) in frame.iter_mut().zip(coeffs.iter()) {
            *s *= w;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hann_endpoints_are_zero_and_center_is_one() {
        let w = Window::Hann.coefficients(101);
        assert!(w[0].abs() < 1e-12);
        assert!(w[100].abs() < 1e-12);
        assert!((w[50] - 1.0).abs() < 1e-12);
    }

    #[test]
    fn rect_is_all_ones() {
        assert!(Window::Rect.coefficients(16).iter().all(|&x| x == 1.0));
    }

    #[test]
    fn degenerate_lengths() {
        assert!(Window::Hann.coefficients(0).is_empty());
        assert_eq!(Window::Blackman.coefficients(1), vec![1.0]);
    }
}
