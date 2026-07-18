//! Decoding of request audio bodies into the core working format
//! (mono f64 + sample rate), with format negotiation via query params.

use crate::error::ApiError;
use serde::Deserialize;
use t2v_core::audio::{decode_mulaw, parse_wav, AudioClip};

/// Accepted sample-rate window for any decoded clip. Bounds the FFT/resample
/// work and keeps a crafted header from wrapping the `i32` column or tripping
/// the database CHECK constraint (which would surface as a 500).
const MIN_SAMPLE_RATE: u32 = 4000;
const MAX_SAMPLE_RATE: u32 = 384_000;

/// Query params shared by the audio-accepting endpoints.
#[derive(Debug, Clone, Deserialize)]
pub struct AudioParams {
    /// "wav" (default) or "mulaw" (raw G.711 bytes, telephony).
    pub format: Option<String>,
    /// Sample rate for raw formats without a header (mulaw). Default 8000.
    pub rate: Option<u32>,
}

pub fn decode_body(params: &AudioParams, body: &[u8]) -> Result<AudioClip, ApiError> {
    if body.is_empty() {
        return Err(ApiError::bad_request("empty audio body"));
    }
    match params.format.as_deref().unwrap_or("wav") {
        "wav" => Ok(parse_wav(body)?),
        "mulaw" | "ulaw" | "g711" => {
            let sample_rate = params.rate.unwrap_or(8000);
            if !(4000..=48000).contains(&sample_rate) {
                return Err(ApiError::bad_request("rate must be within 4000..=48000"));
            }
            Ok(AudioClip {
                sample_rate,
                samples: decode_mulaw(body),
                source_channels: 1,
            })
        }
        other => Err(ApiError::bad_request(format!(
            "unsupported audio format '{other}' (wav|mulaw)"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use t2v_core::audio::{encode_mulaw, encode_wav_pcm16};

    #[test]
    fn decodes_wav_by_default() {
        let samples = vec![0.1f64; 800];
        let body = encode_wav_pcm16(&samples, 8000);
        let params = AudioParams {
            format: None,
            rate: None,
        };
        let clip = decode_body(&params, &body).unwrap();
        assert_eq!(clip.sample_rate, 8000);
        assert_eq!(clip.samples.len(), 800);
    }

    #[test]
    fn decodes_mulaw_with_rate() {
        let samples = vec![0.2f64; 160];
        let body = encode_mulaw(&samples);
        let params = AudioParams {
            format: Some("mulaw".into()),
            rate: Some(8000),
        };
        let clip = decode_body(&params, &body).unwrap();
        assert_eq!(clip.sample_rate, 8000);
        assert_eq!(clip.samples.len(), 160);
    }

    #[test]
    fn rejects_unknown_format_and_empty_body() {
        let params = AudioParams {
            format: Some("flac".into()),
            rate: None,
        };
        assert!(decode_body(&params, &[1, 2, 3]).is_err());
        let wav_params = AudioParams {
            format: None,
            rate: None,
        };
        assert!(decode_body(&wav_params, &[]).is_err());
    }
}
