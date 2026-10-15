//! DTMF (touch-tone) decoding on top of the Goertzel detector.
//!
//! Telephony-adjacent by design: Vapi calls ride the PSTN, so being able to
//! decode keypad digits from raw audio is a natural capability for the
//! analysis endpoint.

use crate::goertzel::goertzel_power;
use crate::vad::rms;

pub const ROW_FREQS: [f64; 4] = [697.0, 770.0, 852.0, 941.0];
pub const COL_FREQS: [f64; 4] = [1209.0, 1336.0, 1477.0, 1633.0];

const KEYPAD: [[char; 4]; 4] = [
    ['1', '2', '3', 'A'],
    ['4', '5', '6', 'B'],
    ['7', '8', '9', 'C'],
    ['*', '0', '#', 'D'],
];

/// Classic DTMF frame length at 8 kHz; scaled for other rates.
const BASE_FRAME: usize = 205;
const BASE_RATE: f64 = 8000.0;

/// Decode a single frame. Returns the digit only when exactly one row and one
/// column tone clearly dominate.
pub fn detect_digit(frame: &[f64], sample_rate: u32) -> Option<char> {
    let n = frame.len();
    if n < 64 {
        return None;
    }
    if rms(frame) < 1e-4 {
        return None; // effectively silence
    }

    let powers = |freqs: &[f64; 4]| -> Vec<f64> {
        freqs
            .iter()
            .map(|&f| goertzel_power(frame, sample_rate, f))
            .collect()
    };
    let row_p = powers(&ROW_FREQS);
    let col_p = powers(&COL_FREQS);

    let argmax = |p: &[f64]| -> (usize, f64, f64) {
        let mut best = 0usize;
        for (i, &v) in p.iter().enumerate() {
            if v > p[best] {
                best = i;
            }
        }
        let second = p
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != best)
            .map(|(_, &v)| v)
            .fold(0.0f64, f64::max);
        (best, p[best], second)
    };

    let (ri, rp, r2) = argmax(&row_p);
    let (ci, cp, c2) = argmax(&col_p);

    // A pure dual tone of amplitude A per component has Goertzel power
    // ~ (A*n/2)^2 and frame energy ~ n*A^2, so power/(energy*n) ~ 1/4 per
    // component. Requiring > 0.1 means each tone carries a dominant share.
    let energy: f64 = frame.iter().map(|x| x * x).sum();
    let floor = energy * n as f64 * 0.1;
    if rp < floor || cp < floor {
        return None;
    }
    // Reject when a second tone in the same group is comparable (no clean key).
    if r2 > rp * 0.5 || c2 > cp * 0.5 {
        return None;
    }
    Some(KEYPAD[ri][ci])
}

/// Decode a digit sequence from an audio clip.
///
/// Frames of ~25.6 ms with a requirement of two consecutive agreeing frames
/// (~50 ms, matching the spec's minimum tone duration) debounce the detector;
/// a non-digit frame re-arms it so repeated digits separated by silence are
/// kept distinct.
pub fn detect_sequence(samples: &[f64], sample_rate: u32) -> String {
    let frame_len = ((BASE_FRAME as f64) * (sample_rate as f64) / BASE_RATE) as usize;
    let frame_len = frame_len.max(64);
    let mut out = String::new();
    let mut pending: Option<char> = None;
    let mut emitted = false;

    for frame in samples.chunks(frame_len) {
        if frame.len() < frame_len / 2 {
            break;
        }
        match detect_digit(frame, sample_rate) {
            Some(d) => {
                if pending == Some(d) {
                    if !emitted {
                        out.push(d);
                        emitted = true;
                    }
                } else {
                    pending = Some(d);
                    emitted = false;
                }
            }
            None => {
                pending = None;
                emitted = false;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    fn dtmf_tone(digit: char, rate: u32, ms: usize) -> Vec<f64> {
        let (mut row, mut col) = (0usize, 0usize);
        for (r, line) in KEYPAD.iter().enumerate() {
            for (c, &k) in line.iter().enumerate() {
                if k == digit {
                    row = r;
                    col = c;
                }
            }
        }
        let n = rate as usize * ms / 1000;
        (0..n)
            .map(|i| {
                let t = i as f64 / rate as f64;
                0.5 * (2.0 * PI * ROW_FREQS[row] * t).sin()
                    + 0.5 * (2.0 * PI * COL_FREQS[col] * t).sin()
            })
            .collect()
    }

    fn silence(rate: u32, ms: usize) -> Vec<f64> {
        vec![0.0; rate as usize * ms / 1000]
    }

    #[test]
    fn decodes_single_digit() {
        let rate = 8000;
        let audio = dtmf_tone('5', rate, 120);
        assert_eq!(detect_sequence(&audio, rate), "5");
    }

    #[test]
    fn decodes_sequence_with_gaps() {
        let rate = 8000;
        let mut audio = Vec::new();
        for d in ['1', '8', '#', 'A'] {
            audio.extend(dtmf_tone(d, rate, 120));
            audio.extend(silence(rate, 80));
        }
        assert_eq!(detect_sequence(&audio, rate), "18#A");
    }

    #[test]
    fn repeated_digit_with_gap_counts_twice() {
        let rate = 8000;
        let mut audio = Vec::new();
        audio.extend(dtmf_tone('7', rate, 120));
        audio.extend(silence(rate, 100));
        audio.extend(dtmf_tone('7', rate, 120));
        assert_eq!(detect_sequence(&audio, rate), "77");
    }

    #[test]
    fn plain_speechlike_tone_is_not_a_digit() {
        let rate = 8000;
        let n = rate as usize / 5;
        let audio: Vec<f64> = (0..n)
            .map(|i| (2.0 * PI * 440.0 * i as f64 / rate as f64).sin())
            .collect();
        assert_eq!(detect_sequence(&audio, rate), "");
    }

    #[test]
    fn works_at_16k_sample_rate() {
        let rate = 16000;
        let audio = dtmf_tone('9', rate, 120);
        assert_eq!(detect_sequence(&audio, rate), "9");
    }
}
