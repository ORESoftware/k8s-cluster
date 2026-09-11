//! Audio containers and codecs: WAV (RIFF) parse/encode, G.711 mu-law,
//! PCM conversions, and linear resampling. All hand-rolled — no hound.
//!
//! The internal working format everywhere in this crate is mono `f64` in
//! [-1.0, 1.0].

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AudioError {
    NotRiff,
    MissingChunk(&'static str),
    UnsupportedFormat(u16),
    UnsupportedBitDepth(u16),
    ZeroChannels,
    Truncated,
}

impl std::fmt::Display for AudioError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AudioError::NotRiff => write!(f, "not a RIFF/WAVE file"),
            AudioError::MissingChunk(c) => write!(f, "missing '{c}' chunk"),
            AudioError::UnsupportedFormat(t) => write!(f, "unsupported WAV format tag {t}"),
            AudioError::UnsupportedBitDepth(b) => write!(f, "unsupported bit depth {b}"),
            AudioError::ZeroChannels => write!(f, "fmt chunk declares zero channels"),
            AudioError::Truncated => write!(f, "file truncated mid-chunk"),
        }
    }
}

impl std::error::Error for AudioError {}

/// Decoded audio clip, always mono f64.
#[derive(Debug, Clone, PartialEq)]
pub struct AudioClip {
    pub sample_rate: u32,
    pub samples: Vec<f64>,
    /// Channel count of the source container before the mono mixdown.
    pub source_channels: u16,
}

impl AudioClip {
    pub fn duration_secs(&self) -> f64 {
        if self.sample_rate == 0 {
            return 0.0;
        }
        self.samples.len() as f64 / self.sample_rate as f64
    }
}

const FORMAT_PCM: u16 = 1;
const FORMAT_IEEE_FLOAT: u16 = 3;
const FORMAT_MULAW: u16 = 7;
const FORMAT_EXTENSIBLE: u16 = 0xFFFE;

/// Parse a WAV file into a mono clip. Supports PCM 8/16/24/32-bit,
/// 32-bit float, and G.711 mu-law payloads; multi-channel input is averaged
/// down to mono.
pub fn parse_wav(bytes: &[u8]) -> Result<AudioClip, AudioError> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(AudioError::NotRiff);
    }

    let mut fmt: Option<(u16, u16, u32, u16)> = None; // (format, channels, rate, bits)
    let mut data: Option<&[u8]> = None;

    let mut pos = 12usize;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = u32::from_le_bytes([
            bytes[pos + 4],
            bytes[pos + 5],
            bytes[pos + 6],
            bytes[pos + 7],
        ]) as usize;
        let body_start = pos + 8;
        let body_end = body_start.checked_add(size).ok_or(AudioError::Truncated)?;
        if body_end > bytes.len() {
            return Err(AudioError::Truncated);
        }
        let body = &bytes[body_start..body_end];

        match id {
            b"fmt " => {
                if body.len() < 16 {
                    return Err(AudioError::Truncated);
                }
                let mut format = u16::from_le_bytes([body[0], body[1]]);
                let channels = u16::from_le_bytes([body[2], body[3]]);
                let rate = u32::from_le_bytes([body[4], body[5], body[6], body[7]]);
                let bits = u16::from_le_bytes([body[14], body[15]]);
                // WAVE_FORMAT_EXTENSIBLE stores the real format code in the
                // first two bytes of the SubFormat GUID at offset 24.
                if format == FORMAT_EXTENSIBLE {
                    if body.len() < 26 {
                        return Err(AudioError::Truncated);
                    }
                    format = u16::from_le_bytes([body[24], body[25]]);
                }
                fmt = Some((format, channels, rate, bits));
            }
            b"data" => {
                data = Some(body);
            }
            _ => {}
        }
        // Chunks are word-aligned: odd sizes carry one pad byte.
        pos = body_end + (size & 1);
    }

    let (format, channels, sample_rate, bits) = fmt.ok_or(AudioError::MissingChunk("fmt "))?;
    let data = data.ok_or(AudioError::MissingChunk("data"))?;
    if channels == 0 {
        return Err(AudioError::ZeroChannels);
    }

    let interleaved: Vec<f64> = match (format, bits) {
        (FORMAT_PCM, 8) => data.iter().map(|&b| (b as f64 - 128.0) / 128.0).collect(),
        (FORMAT_PCM, 16) => data
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]) as f64 / 32768.0)
            .collect(),
        (FORMAT_PCM, 24) => data
            .chunks_exact(3)
            .map(|c| {
                let raw = ((c[2] as i32) << 16 | (c[1] as i32) << 8 | c[0] as i32) << 8 >> 8;
                raw as f64 / 8388608.0
            })
            .collect(),
        (FORMAT_PCM, 32) => data
            .chunks_exact(4)
            .map(|c| i32::from_le_bytes([c[0], c[1], c[2], c[3]]) as f64 / 2147483648.0)
            .collect(),
        (FORMAT_IEEE_FLOAT, 32) => data
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]) as f64)
            .collect(),
        (FORMAT_MULAW, 8) => data
            .iter()
            .map(|&b| mulaw_decode_sample(b) as f64 / 32768.0)
            .collect(),
        (FORMAT_PCM, other) | (FORMAT_IEEE_FLOAT, other) | (FORMAT_MULAW, other) => {
            return Err(AudioError::UnsupportedBitDepth(other))
        }
        (other, _) => return Err(AudioError::UnsupportedFormat(other)),
    };

    let samples = mix_to_mono(&interleaved, channels as usize);
    Ok(AudioClip {
        sample_rate,
        samples,
        source_channels: channels,
    })
}

/// Average interleaved channels down to mono.
pub fn mix_to_mono(interleaved: &[f64], channels: usize) -> Vec<f64> {
    if channels <= 1 {
        return interleaved.to_vec();
    }
    interleaved
        .chunks_exact(channels)
        .map(|frame| frame.iter().sum::<f64>() / channels as f64)
        .collect()
}

/// Encode mono f64 samples as a 16-bit PCM WAV file.
pub fn encode_wav_pcm16(samples: &[f64], sample_rate: u32) -> Vec<u8> {
    let data_len = samples.len() * 2;
    let mut out = Vec::with_capacity(44 + data_len);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&((36 + data_len) as u32).to_le_bytes());
    out.extend_from_slice(b"WAVE");

    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&FORMAT_PCM.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&(sample_rate * 2).to_le_bytes()); // byte rate
    out.extend_from_slice(&2u16.to_le_bytes()); // block align
    out.extend_from_slice(&16u16.to_le_bytes()); // bits

    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data_len as u32).to_le_bytes());
    for &s in samples {
        out.extend_from_slice(&f64_to_pcm16(s).to_le_bytes());
    }
    out
}

#[inline]
pub fn f64_to_pcm16(x: f64) -> i16 {
    (x.clamp(-1.0, 1.0) * 32767.0).round() as i16
}

// ---------------------------------------------------------------------------
// G.711 mu-law — the PSTN codec (Vapi/Twilio phone audio is mu-law at 8 kHz).
// ---------------------------------------------------------------------------

const MULAW_BIAS: i32 = 0x84;
const MULAW_CLIP: i32 = 32635;

pub fn mulaw_encode_sample(pcm: i16) -> u8 {
    let mut value = pcm as i32;
    let sign: u8 = if value < 0 {
        value = -value;
        0x80
    } else {
        0
    };
    if value > MULAW_CLIP {
        value = MULAW_CLIP;
    }
    value += MULAW_BIAS;

    let mut exponent: u8 = 7;
    let mut mask = 0x4000;
    while exponent > 0 && (value & mask) == 0 {
        exponent -= 1;
        mask >>= 1;
    }
    let mantissa = ((value >> (exponent as i32 + 3)) & 0x0F) as u8;
    !(sign | (exponent << 4) | mantissa)
}

pub fn mulaw_decode_sample(mu: u8) -> i16 {
    let mu = !mu;
    let sign = mu & 0x80;
    let exponent = (mu >> 4) & 0x07;
    let mantissa = (mu & 0x0F) as i32;
    let mut sample = (((mantissa << 3) + MULAW_BIAS) << exponent) - MULAW_BIAS;
    if sign != 0 {
        sample = -sample;
    }
    sample as i16
}

/// Decode a raw G.711 mu-law byte stream (e.g. 8 kHz telephony audio) to mono f64.
pub fn decode_mulaw(bytes: &[u8]) -> Vec<f64> {
    bytes
        .iter()
        .map(|&b| mulaw_decode_sample(b) as f64 / 32768.0)
        .collect()
}

/// Encode mono f64 samples as raw G.711 mu-law bytes.
pub fn encode_mulaw(samples: &[f64]) -> Vec<u8> {
    samples
        .iter()
        .map(|&s| mulaw_encode_sample(f64_to_pcm16(s)))
        .collect()
}

// ---------------------------------------------------------------------------
// Resampling
// ---------------------------------------------------------------------------

/// Linear-interpolation resampler. Fine for speech-band conversions
/// (8 kHz <-> 16 kHz <-> 24 kHz); not meant for hi-fi program material.
pub fn resample_linear(input: &[f64], from_rate: u32, to_rate: u32) -> Vec<f64> {
    if from_rate == to_rate || input.is_empty() || from_rate == 0 || to_rate == 0 {
        return input.to_vec();
    }
    let ratio = from_rate as f64 / to_rate as f64;
    let out_len = ((input.len() as f64) * (to_rate as f64) / (from_rate as f64)).round() as usize;
    let out_len = out_len.max(1);
    (0..out_len)
        .map(|i| {
            let pos = i as f64 * ratio;
            let i0 = pos.floor() as usize;
            if i0 + 1 >= input.len() {
                input[input.len() - 1]
            } else {
                let frac = pos - i0 as f64;
                input[i0] * (1.0 - frac) + input[i0 + 1] * frac
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    fn sine(freq: f64, rate: u32, n: usize) -> Vec<f64> {
        (0..n)
            .map(|i| (2.0 * PI * freq * i as f64 / rate as f64).sin() * 0.8)
            .collect()
    }

    #[test]
    fn wav_pcm16_roundtrip() {
        let samples = sine(440.0, 16000, 1600);
        let bytes = encode_wav_pcm16(&samples, 16000);
        let clip = parse_wav(&bytes).unwrap();
        assert_eq!(clip.sample_rate, 16000);
        assert_eq!(clip.source_channels, 1);
        assert_eq!(clip.samples.len(), samples.len());
        let max_err = clip
            .samples
            .iter()
            .zip(samples.iter())
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f64, f64::max);
        // One LSB of 16-bit quantization plus the 32767/32768 asymmetry.
        assert!(max_err < 1.0 / 16000.0, "max_err {max_err}");
    }

    #[test]
    fn stereo_mixes_down_to_mono() {
        // Hand-build a 2-channel PCM16 wav: L = 0.5, R = -0.5 -> mono 0.0
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        let n_frames = 100u32;
        let data_len = n_frames * 4;
        wav.extend_from_slice(&(36 + data_len).to_le_bytes());
        wav.extend_from_slice(b"WAVE");
        wav.extend_from_slice(b"fmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&8000u32.to_le_bytes());
        wav.extend_from_slice(&32000u32.to_le_bytes());
        wav.extend_from_slice(&4u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data_len.to_le_bytes());
        for _ in 0..n_frames {
            wav.extend_from_slice(&16384i16.to_le_bytes());
            wav.extend_from_slice(&(-16384i16).to_le_bytes());
        }
        let clip = parse_wav(&wav).unwrap();
        assert_eq!(clip.source_channels, 2);
        assert_eq!(clip.samples.len(), 100);
        assert!(clip.samples.iter().all(|&s| s.abs() < 1e-9));
    }

    #[test]
    fn wav_parser_skips_unknown_chunks() {
        let samples = sine(300.0, 8000, 200);
        let bytes = encode_wav_pcm16(&samples, 8000);
        // Splice a LIST chunk between fmt and data.
        let mut spliced = bytes[..36].to_vec();
        spliced.extend_from_slice(b"LIST");
        spliced.extend_from_slice(&5u32.to_le_bytes());
        spliced.extend_from_slice(b"INFOx");
        spliced.push(0); // pad byte for odd size
        spliced.extend_from_slice(&bytes[36..]);
        let total = (spliced.len() - 8) as u32;
        spliced[4..8].copy_from_slice(&total.to_le_bytes());
        let clip = parse_wav(&spliced).unwrap();
        assert_eq!(clip.samples.len(), 200);
    }

    #[test]
    fn rejects_garbage() {
        assert!(matches!(
            parse_wav(b"not a wav at all"),
            Err(AudioError::NotRiff)
        ));
    }

    #[test]
    fn truncated_data_chunk_errors() {
        let samples = sine(300.0, 8000, 100);
        let mut bytes = encode_wav_pcm16(&samples, 8000);
        bytes.truncate(bytes.len() - 50);
        assert!(matches!(parse_wav(&bytes), Err(AudioError::Truncated)));
    }

    #[test]
    fn mulaw_roundtrip_is_close() {
        for &v in &[
            0i16,
            1,
            -1,
            100,
            -100,
            1000,
            -1000,
            8000,
            -8000,
            30000,
            -30000,
            i16::MAX,
            i16::MIN + 1,
        ] {
            let decoded = mulaw_decode_sample(mulaw_encode_sample(v)) as i32;
            let err = (decoded - v as i32).abs();
            // mu-law quantization error grows with the segment; allow ~4%.
            let allowed = (v as i32).abs() / 16 + 64;
            assert!(err <= allowed, "v={v} decoded={decoded} err={err}");
        }
    }

    #[test]
    fn mulaw_silence_is_stable() {
        let z = mulaw_decode_sample(mulaw_encode_sample(0));
        assert!(z.abs() <= 8, "zero decodes to {z}");
    }

    #[test]
    fn mulaw_stream_roundtrip() {
        let samples = sine(440.0, 8000, 800);
        let encoded = encode_mulaw(&samples);
        let decoded = decode_mulaw(&encoded);
        assert_eq!(decoded.len(), samples.len());
        let max_err = decoded
            .iter()
            .zip(samples.iter())
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f64, f64::max);
        assert!(max_err < 0.05, "max_err {max_err}");
    }

    #[test]
    fn resample_changes_length_and_keeps_pitch() {
        let rate_in = 8000u32;
        let rate_out = 16000u32;
        let samples = sine(440.0, rate_in, 8000);
        let out = resample_linear(&samples, rate_in, rate_out);
        assert!((out.len() as i64 - 16000).abs() <= 2, "len {}", out.len());

        let bins = crate::fft::magnitude_spectrum(&out, rate_out).unwrap();
        let peak = bins
            .iter()
            .skip(1)
            .max_by(|a, b| a.magnitude.total_cmp(&b.magnitude))
            .unwrap();
        assert!(
            (peak.freq_hz - 440.0).abs() < 5.0,
            "peak at {}",
            peak.freq_hz
        );
    }

    #[test]
    fn resample_identity_when_rates_match() {
        let samples = sine(100.0, 8000, 100);
        assert_eq!(resample_linear(&samples, 8000, 8000), samples);
    }
}
