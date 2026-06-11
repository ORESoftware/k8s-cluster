//! Paid third-party OCR backends, called over HTTPS with `reqwest`.
//!
//! Three providers are wired up; each is enabled only when its credentials are
//! present in the environment (sourced from the `dd-ocr-rs-secrets` bundle):
//!
//!   * [`google_vision`]  — Google Cloud Vision `images:annotate`
//!     (`DOCUMENT_TEXT_DETECTION`), API-key auth.
//!   * [`aws_textract`]    — AWS Textract `DetectDocumentText`, signed with a
//!     self-contained SigV4 implementation (no AWS SDK).
//!   * [`azure_read`]      — Azure AI Vision 4.0 Image Analysis `read` feature,
//!     subscription-key auth.
//!
//! Every backend returns a normalised [`CloudOcr`] (plain text + optional mean
//! confidence + recognised line count) so the service can treat them uniformly.

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;

/// Normalised OCR result shared by every cloud backend.
#[derive(Debug)]
pub struct CloudOcr {
    pub text: String,
    /// Mean confidence in 0.0..=1.0 when the provider reports one.
    pub confidence: Option<f64>,
    /// Number of recognised text lines/blocks (provider-dependent granularity).
    pub lines: usize,
}

#[derive(Debug)]
pub enum CloudError {
    /// The backend has no credentials configured.
    NotConfigured,
    /// Transport failure (DNS/TLS/timeout/connection).
    Transport(String),
    /// Non-2xx HTTP status with the (truncated) provider message.
    Status(u16, String),
    /// 2xx response we could not parse into text.
    Parse(String),
}

impl std::fmt::Display for CloudError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CloudError::NotConfigured => write!(f, "backend is not configured"),
            CloudError::Transport(msg) => write!(f, "transport error: {msg}"),
            CloudError::Status(code, msg) => write!(f, "provider returned HTTP {code}: {msg}"),
            CloudError::Parse(msg) => write!(f, "could not parse provider response: {msg}"),
        }
    }
}

impl std::error::Error for CloudError {}

/// Trim a provider error body so it never floods logs / responses.
fn truncate(body: &str) -> String {
    body.chars().take(500).collect()
}

/// Strip any `key=<value>` query parameter that might survive in a URL embedded
/// in a transport error, so an API key can never reach a log line or response.
fn redact(message: String) -> String {
    let mut out = String::with_capacity(message.len());
    let mut rest = message.as_str();
    while let Some(pos) = rest.find("key=") {
        out.push_str(&rest[..pos + 4]);
        out.push_str("REDACTED");
        rest = &rest[pos + 4..];
        let end = rest
            .find(|c: char| c == '&' || c.is_whitespace())
            .unwrap_or(rest.len());
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

// ---------------------------------------------------------------------------
// Google Cloud Vision
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct GoogleConfig {
    pub api_key: String,
}

/// OCR `image` with Google Cloud Vision. `document` selects dense-document
/// detection (`DOCUMENT_TEXT_DETECTION`) over sparse `TEXT_DETECTION`.
pub async fn google_vision(
    http: &reqwest::Client,
    cfg: &GoogleConfig,
    image: &[u8],
    document: bool,
) -> Result<CloudOcr, CloudError> {
    let feature = if document {
        "DOCUMENT_TEXT_DETECTION"
    } else {
        "TEXT_DETECTION"
    };
    // Pass the API key in the header, not the query string: a key in the URL
    // leaks into reqwest error messages, request logs, and any intermediary.
    let url = "https://vision.googleapis.com/v1/images:annotate";
    let body = json!({
        "requests": [{
            "image": { "content": BASE64.encode(image) },
            "features": [{ "type": feature }],
        }]
    });

    let resp = http
        .post(url)
        .header("X-goog-api-key", &cfg.api_key)
        .json(&body)
        .send()
        .await
        .map_err(|e| CloudError::Transport(redact(e.to_string())))?;
    let status = resp.status();
    let text = resp
        .text()
        .await
        .map_err(|e| CloudError::Transport(redact(e.to_string())))?;
    if !status.is_success() {
        return Err(CloudError::Status(status.as_u16(), truncate(&text)));
    }

    let value: Value = serde_json::from_str(&text).map_err(|e| CloudError::Parse(e.to_string()))?;
    let response = value
        .get("responses")
        .and_then(|r| r.get(0))
        .ok_or_else(|| CloudError::Parse("missing responses[0]".to_string()))?;
    if let Some(err) = response.get("error").and_then(|e| e.get("message")).and_then(|m| m.as_str()) {
        return Err(CloudError::Status(status.as_u16(), truncate(err)));
    }

    // Prefer the structured fullTextAnnotation; fall back to the first
    // textAnnotations entry (whole-image text).
    let full = response
        .get("fullTextAnnotation")
        .and_then(|f| f.get("text"))
        .and_then(|t| t.as_str());
    let recognised = full
        .map(|s| s.to_string())
        .or_else(|| {
            response
                .get("textAnnotations")
                .and_then(|a| a.get(0))
                .and_then(|a| a.get("description"))
                .and_then(|d| d.as_str())
                .map(|s| s.to_string())
        })
        .unwrap_or_default();

    // Mean word confidence across pages/blocks/paragraphs/words, when present.
    let confidence = google_mean_confidence(response);
    let lines = recognised.lines().filter(|l| !l.trim().is_empty()).count();
    Ok(CloudOcr {
        text: recognised,
        confidence,
        lines,
    })
}

fn google_mean_confidence(response: &Value) -> Option<f64> {
    let pages = response.get("fullTextAnnotation")?.get("pages")?.as_array()?;
    let mut sum = 0.0;
    let mut count = 0usize;
    for page in pages {
        if let Some(blocks) = page.get("blocks").and_then(|b| b.as_array()) {
            for block in blocks {
                if let Some(c) = block.get("confidence").and_then(|c| c.as_f64()) {
                    sum += c;
                    count += 1;
                }
            }
        }
    }
    if count == 0 {
        None
    } else {
        Some(sum / count as f64)
    }
}

// ---------------------------------------------------------------------------
// AWS Textract (DetectDocumentText) — signed with a compact SigV4.
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct AwsConfig {
    pub access_key_id: String,
    pub secret_access_key: String,
    pub session_token: Option<String>,
    pub region: String,
}

/// OCR `image` with AWS Textract `DetectDocumentText`.
pub async fn aws_textract(
    http: &reqwest::Client,
    cfg: &AwsConfig,
    image: &[u8],
) -> Result<CloudOcr, CloudError> {
    let host = format!("textract.{}.amazonaws.com", cfg.region);
    let url = format!("https://{host}/");
    let target = "Textract.DetectDocumentText";
    let content_type = "application/x-amz-json-1.1";
    let payload = json!({ "Document": { "Bytes": BASE64.encode(image) } }).to_string();

    let headers = sigv4_headers(cfg, &host, target, content_type, payload.as_bytes());

    let mut request = http.post(&url).body(payload);
    for (name, value) in &headers {
        request = request.header(name.as_str(), value.as_str());
    }
    let resp = request
        .send()
        .await
        .map_err(|e| CloudError::Transport(redact(e.to_string())))?;
    let status = resp.status();
    let text = resp
        .text()
        .await
        .map_err(|e| CloudError::Transport(redact(e.to_string())))?;
    if !status.is_success() {
        return Err(CloudError::Status(status.as_u16(), truncate(&text)));
    }

    let value: Value = serde_json::from_str(&text).map_err(|e| CloudError::Parse(e.to_string()))?;
    let blocks = value
        .get("Blocks")
        .and_then(|b| b.as_array())
        .ok_or_else(|| CloudError::Parse("missing Blocks".to_string()))?;

    let mut out_lines = Vec::new();
    let mut conf_sum = 0.0;
    let mut conf_count = 0usize;
    for block in blocks {
        if block.get("BlockType").and_then(|t| t.as_str()) == Some("LINE") {
            if let Some(line) = block.get("Text").and_then(|t| t.as_str()) {
                out_lines.push(line.to_string());
            }
            if let Some(c) = block.get("Confidence").and_then(|c| c.as_f64()) {
                conf_sum += c;
                conf_count += 1;
            }
        }
    }
    let lines = out_lines.len();
    Ok(CloudOcr {
        text: out_lines.join("\n"),
        // Textract confidence is a 0..100 percentage.
        confidence: (conf_count > 0).then(|| conf_sum / conf_count as f64 / 100.0),
        lines,
    })
}

/// Build the SigV4-signed headers for a Textract POST. Returns
/// (header-name, header-value) pairs to attach to the request.
fn sigv4_headers(
    cfg: &AwsConfig,
    host: &str,
    target: &str,
    content_type: &str,
    payload: &[u8],
) -> Vec<(String, String)> {
    let service = "textract";
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let (amz_date, date_stamp) = amz_timestamps(now);

    let payload_hash = hex::encode(Sha256::digest(payload));

    // Canonical request. Header names must be lowercase + sorted; we include
    // the session token in the signed set when present.
    let mut signed_pairs: Vec<(String, String)> = vec![
        ("content-type".to_string(), content_type.to_string()),
        ("host".to_string(), host.to_string()),
        ("x-amz-content-sha256".to_string(), payload_hash.clone()),
        ("x-amz-date".to_string(), amz_date.clone()),
        ("x-amz-target".to_string(), target.to_string()),
    ];
    if let Some(token) = &cfg.session_token {
        signed_pairs.push(("x-amz-security-token".to_string(), token.clone()));
    }
    signed_pairs.sort_by(|a, b| a.0.cmp(&b.0));

    let canonical_headers: String = signed_pairs
        .iter()
        .map(|(k, v)| format!("{k}:{v}\n"))
        .collect();
    let signed_headers = signed_pairs
        .iter()
        .map(|(k, _)| k.as_str())
        .collect::<Vec<_>>()
        .join(";");

    let canonical_request = format!(
        "POST\n/\n\n{canonical_headers}\n{signed_headers}\n{payload_hash}"
    );

    let scope = format!("{date_stamp}/{}/{service}/aws4_request", cfg.region);
    let signature = sigv4_signature(
        &cfg.secret_access_key,
        &cfg.region,
        service,
        &amz_date,
        &date_stamp,
        &canonical_request,
    );

    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed_headers}, Signature={signature}",
        cfg.access_key_id
    );

    // Return every header reqwest must send (the signed set + Authorization).
    let mut out = signed_pairs;
    out.retain(|(k, _)| k != "host"); // reqwest sets Host from the URL.
    out.push(("Authorization".to_string(), authorization));
    out
}

/// SigV4 signing-key derivation: AWS4-secret -> date -> region -> service ->
/// `aws4_request`, chained HMAC-SHA256.
fn sigv4_signing_key(secret: &str, date_stamp: &str, region: &str, service: &str) -> Vec<u8> {
    let k_date = hmac(format!("AWS4{secret}").as_bytes(), date_stamp.as_bytes());
    let k_region = hmac(&k_date, region.as_bytes());
    let k_service = hmac(&k_region, service.as_bytes());
    hmac(&k_service, b"aws4_request")
}

/// Hex SigV4 signature over an already-built canonical request.
fn sigv4_signature(
    secret: &str,
    region: &str,
    service: &str,
    amz_date: &str,
    date_stamp: &str,
    canonical_request: &str,
) -> String {
    let scope = format!("{date_stamp}/{region}/{service}/aws4_request");
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}",
        hex::encode(Sha256::digest(canonical_request.as_bytes()))
    );
    let key = sigv4_signing_key(secret, date_stamp, region, service);
    hex::encode(hmac(&key, string_to_sign.as_bytes()))
}

fn hmac(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

/// Format a unix timestamp as the SigV4 (`YYYYMMDDTHHMMSSZ`, `YYYYMMDD`) pair,
/// in UTC, without pulling in a date library.
fn amz_timestamps(unix_secs: u64) -> (String, String) {
    let days = unix_secs / 86_400;
    let secs_of_day = unix_secs % 86_400;
    let (hh, mm, ss) = (secs_of_day / 3600, (secs_of_day % 3600) / 60, secs_of_day % 60);
    let (year, month, day) = civil_from_days(days as i64);
    (
        format!("{year:04}{month:02}{day:02}T{hh:02}{mm:02}{ss:02}Z"),
        format!("{year:04}{month:02}{day:02}"),
    )
}

/// Convert a count of days since the Unix epoch to a (year, month, day) tuple.
/// Howard Hinnant's well-known `civil_from_days` algorithm.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

// ---------------------------------------------------------------------------
// Azure AI Vision 4.0 — Image Analysis `read` feature.
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct AzureConfig {
    /// Resource endpoint, e.g. `https://my-vision.cognitiveservices.azure.com`.
    pub endpoint: String,
    pub key: String,
}

/// OCR `image` with Azure AI Vision 4.0 (synchronous `read` feature).
pub async fn azure_read(
    http: &reqwest::Client,
    cfg: &AzureConfig,
    image: &[u8],
) -> Result<CloudOcr, CloudError> {
    let base = cfg.endpoint.trim_end_matches('/');
    let url = format!("{base}/computervision/imageanalysis:analyze?api-version=2024-02-01&features=read");

    let resp = http
        .post(url)
        .header("Ocp-Apim-Subscription-Key", &cfg.key)
        .header("Content-Type", "application/octet-stream")
        .body(image.to_vec())
        .send()
        .await
        .map_err(|e| CloudError::Transport(redact(e.to_string())))?;
    let status = resp.status();
    let text = resp
        .text()
        .await
        .map_err(|e| CloudError::Transport(redact(e.to_string())))?;
    if !status.is_success() {
        return Err(CloudError::Status(status.as_u16(), truncate(&text)));
    }

    let value: Value = serde_json::from_str(&text).map_err(|e| CloudError::Parse(e.to_string()))?;
    let blocks = value
        .get("readResult")
        .and_then(|r| r.get("blocks"))
        .and_then(|b| b.as_array())
        .ok_or_else(|| CloudError::Parse("missing readResult.blocks".to_string()))?;

    let mut out_lines = Vec::new();
    for block in blocks {
        if let Some(lines) = block.get("lines").and_then(|l| l.as_array()) {
            for line in lines {
                if let Some(t) = line.get("text").and_then(|t| t.as_str()) {
                    out_lines.push(t.to_string());
                }
            }
        }
    }
    let lines = out_lines.len();
    Ok(CloudOcr {
        text: out_lines.join("\n"),
        // The 4.0 read feature does not return a per-image confidence scalar.
        confidence: None,
        lines,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_from_days_matches_known_dates() {
        // 1970-01-01 is day 0.
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        // 2000-03-01 is 11017 days after the epoch.
        assert_eq!(civil_from_days(11017), (2000, 3, 1));
        // 2026-06-10 00:00:00 UTC.
        let (a, b) = amz_timestamps(1_781_049_600);
        assert!(a.starts_with("20260610T"), "got {a}");
        assert_eq!(b, "20260610");
    }

    #[test]
    fn sigv4_matches_aws_known_answer_vector() {
        // Official AWS `aws-sig-v4-test-suite` "get-vanilla" case: a GET with
        // only host + x-amz-date signed. If our key derivation + string-to-sign
        // composition reproduces AWS's published signature, the signer is
        // correct (the Textract path uses the same primitives).
        let canonical_request = concat!(
            "GET\n",
            "/\n",
            "\n",
            "host:example.amazonaws.com\n",
            "x-amz-date:20150830T123600Z\n",
            "\n",
            "host;x-amz-date\n",
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        );
        let signature = sigv4_signature(
            "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
            "us-east-1",
            "service",
            "20150830T123600Z",
            "20150830",
            canonical_request,
        );
        assert_eq!(
            signature,
            "5fa00fa31553b73ebf1942676e86291e8372ff2a2260956d9b8aae1d763fbf31"
        );
    }

    #[test]
    fn redact_strips_api_keys() {
        let leaked = "error sending request for url (https://vision.googleapis.com/v1/images:annotate?key=AIzaSyTOPSECRET123&alt=json)";
        let safe = redact(leaked.to_string());
        assert!(!safe.contains("AIzaSyTOPSECRET123"), "key leaked: {safe}");
        assert!(safe.contains("key=REDACTED"));
        assert!(safe.contains("alt=json"));
    }

    #[test]
    fn sigv4_signature_is_stable_and_well_formed() {
        let cfg = AwsConfig {
            access_key_id: "AKIDEXAMPLE".to_string(),
            secret_access_key: "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY".to_string(),
            session_token: None,
            region: "us-east-1".to_string(),
        };
        let headers = sigv4_headers(
            &cfg,
            "textract.us-east-1.amazonaws.com",
            "Textract.DetectDocumentText",
            "application/x-amz-json-1.1",
            b"{}",
        );
        let auth = headers
            .iter()
            .find(|(k, _)| k == "Authorization")
            .map(|(_, v)| v.clone())
            .expect("authorization header present");
        assert!(auth.starts_with("AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/"));
        assert!(auth.contains("SignedHeaders=content-type;host;x-amz-content-sha256;x-amz-date;x-amz-target"));
        assert!(auth.contains("Signature="));
        // x-amz-date must be present and host must be excluded (reqwest sets it).
        assert!(headers.iter().any(|(k, _)| k == "x-amz-date"));
        assert!(headers.iter().all(|(k, _)| k != "host"));
    }
}
