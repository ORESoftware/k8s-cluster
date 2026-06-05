//! Fireblocks — institutional MPC custody / treasury (observer-mode).
//!
//! Fireblocks is the dominant institutional custody platform in crypto;
//! basically every well-funded crypto company custodies through them.
//! Treating it as a provider means our B2B tenants can connect their
//! Fireblocks workspace and we observe transactions across all their
//! vaults without ever holding or moving funds ourselves.
//!
//! Auth model:
//!   - Each request is a JWT signed with the tenant's RSA private key
//!     (`api_secret_pem`). The JWT body includes a sha256 hash of the
//!     request body so a stolen JWT can't be replayed against a
//!     different endpoint.
//!   - We carry the public-key fingerprint in `X-API-Key` along with
//!     the JWT.
//!
//! Webhook model:
//!   - Fireblocks signs each webhook with RSA-SHA512 over the raw body
//!     and includes the signature in the `fireblocks-signature` header
//!     (base64). The public key is published at a stable URL per env
//!     (sandbox / prod); we store the per-tenant PEM in
//!     `FireblocksCredential.webhook_public_key_pem`.

use chrono::Utc;
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use rsa::RsaPublicKey;
use rsa::pkcs1v15::{Signature, VerifyingKey};
use rsa::pkcs8::DecodePublicKey;
use rsa::sha2::Sha512;
use rsa::signature::Verifier;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::error::{AppError, AppResult};

const PROD_BASE: &str = "https://api.fireblocks.io";
const SANDBOX_BASE: &str = "https://sandbox-api.fireblocks.io";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FireblocksCredential {
    /// Fireblocks public API key identifier (uuid-ish).
    pub api_key: String,
    /// RSA private key in PKCS#1 or PKCS#8 PEM. Used for per-request
    /// JWT signing. Sealed at rest.
    pub api_secret_pem: String,
    /// Public key PEM for verifying Fireblocks webhooks (the public
    /// key Fireblocks publishes per env). Sealed at rest only because
    /// we keep all webhook secrets sealed for consistency — it isn't
    /// actually secret.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub webhook_public_key_pem: Option<String>,
    /// "production" | "sandbox" — chooses the API base URL.
    pub environment: String,
}

impl FireblocksCredential {
    pub fn api_base(&self) -> &'static str {
        if self.environment.eq_ignore_ascii_case("production")
            || self.environment.eq_ignore_ascii_case("live")
        {
            PROD_BASE
        } else {
            SANDBOX_BASE
        }
    }
}

// =========================================================================
// JWT-signed request authentication
// =========================================================================
//
// Fireblocks requires each request to carry a JWT with these claims:
//   uri:    the request URI (path + query)
//   nonce:  random per request (we use a uuid)
//   iat:    issued at (epoch seconds)
//   exp:    expiry (iat + 55 seconds, must be < 60)
//   sub:    the API key identifier
//   bodyHash: hex sha256 of the request body (empty string for GETs)

#[derive(Serialize)]
struct FireblocksJwtClaims<'a> {
    uri: &'a str,
    nonce: String,
    iat: i64,
    exp: i64,
    sub: &'a str,
    #[serde(rename = "bodyHash")]
    body_hash: String,
}

pub struct FireblocksApi {
    cred: FireblocksCredential,
    http: reqwest::Client,
    base_url: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct FireblocksTransaction {
    pub id: String,
    pub status: String,
    pub source: Option<FireblocksParty>,
    pub destination: Option<FireblocksParty>,
    pub amount: Option<f64>,
    #[serde(rename = "netAmount")]
    pub net_amount: Option<f64>,
    pub fee: Option<f64>,
    #[serde(rename = "assetId")]
    pub asset_id: Option<String>,
    #[serde(rename = "createdAt")]
    pub created_at: Option<i64>,
    #[serde(rename = "lastUpdated")]
    pub last_updated: Option<i64>,
    pub note: Option<String>,
    #[serde(rename = "txHash")]
    pub tx_hash: Option<String>,
    pub operation: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct FireblocksParty {
    #[serde(rename = "type")]
    pub type_: Option<String>,
    pub id: Option<String>,
    pub name: Option<String>,
}

impl FireblocksApi {
    pub fn new(cred: FireblocksCredential) -> Self {
        let base_url = cred.api_base().to_string();
        Self {
            cred,
            http: reqwest::Client::new(),
            base_url,
        }
    }

    #[cfg(test)]
    pub fn with_base_url_for_tests(cred: FireblocksCredential, base_url: String) -> Self {
        Self {
            cred,
            http: reqwest::Client::new(),
            base_url,
        }
    }

    fn base_url(&self) -> &str {
        &self.base_url
    }

    fn sign_jwt(&self, uri_with_query: &str, body: &[u8]) -> AppResult<String> {
        let body_hash = hex::encode(Sha256::digest(body));
        let now = Utc::now().timestamp();
        let claims = FireblocksJwtClaims {
            uri: uri_with_query,
            nonce: Uuid::new_v4().to_string(),
            iat: now,
            exp: now + 55,
            sub: &self.cred.api_key,
            body_hash,
        };

        // Fireblocks accepts both PKCS#1 and PKCS#8 RSA private keys —
        // jsonwebtoken's `from_rsa_pem` handles both transparently.
        let encoding_key = EncodingKey::from_rsa_pem(self.cred.api_secret_pem.as_bytes())
            .map_err(|e| AppError::Crypto(format!("fireblocks api_secret pem: {e}")))?;
        let header = Header::new(Algorithm::RS256);
        jsonwebtoken::encode(&header, &claims, &encoding_key)
            .map_err(|e| AppError::Crypto(format!("fireblocks jwt encode: {e}")))
    }

    /// `GET /v1/transactions` — paginated transactions for the workspace.
    /// Fireblocks paginates by `before` / `after` epoch-ms cursors; we
    /// walk forward using `after`.
    pub async fn list_transactions(
        &self,
        after_epoch_ms: Option<i64>,
        limit: u32,
    ) -> AppResult<Vec<FireblocksTransaction>> {
        let mut path = format!("/v1/transactions?limit={limit}&orderBy=createdAt&sort=ASC");
        if let Some(after) = after_epoch_ms {
            path.push_str(&format!("&after={after}"));
        }
        let jwt = self.sign_jwt(&path, b"")?;
        let url = format!("{}{}", self.base_url(), path);
        let resp = self
            .http
            .get(&url)
            .bearer_auth(&jwt)
            .header("X-API-Key", &self.cred.api_key)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| AppError::Provider {
                provider: "fireblocks".into(),
                message: format!("transactions HTTP: {e}"),
            })?;
        let status = resp.status();
        let bytes = resp.bytes().await.map_err(|e| AppError::Provider {
            provider: "fireblocks".into(),
            message: format!("transactions body: {e}"),
        })?;
        if !status.is_success() {
            return Err(AppError::Provider {
                provider: "fireblocks".into(),
                message: format!("transactions {status}: {}", String::from_utf8_lossy(&bytes)),
            });
        }
        let parsed: Vec<FireblocksTransaction> =
            serde_json::from_slice(&bytes).map_err(|e| AppError::Provider {
                provider: "fireblocks".into(),
                message: format!("transactions decode: {e}"),
            })?;
        Ok(parsed)
    }
}

// =========================================================================
// Webhook signature verification — RSA-SHA512 over the raw body
// =========================================================================

pub fn verify_webhook_signature(
    body: &[u8],
    signature_b64: &str,
    public_key_pem: &str,
) -> AppResult<()> {
    use base64::Engine as _;
    let pubkey = RsaPublicKey::from_public_key_pem(public_key_pem.trim())
        .map_err(|e| AppError::Crypto(format!("fireblocks pubkey pem: {e}")))?;
    let verifying_key = VerifyingKey::<Sha512>::new(pubkey);
    let sig_bytes = base64::engine::general_purpose::STANDARD
        .decode(signature_b64.trim().as_bytes())
        .map_err(|e| AppError::Crypto(format!("fireblocks sig b64: {e}")))?;
    let signature = Signature::try_from(sig_bytes.as_slice())
        .map_err(|e| AppError::Crypto(format!("fireblocks sig: {e}")))?;
    verifying_key
        .verify(body, &signature)
        .map_err(|_| AppError::Unauthorized)
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;
    use rsa::RsaPrivateKey;
    use rsa::pkcs1v15::SigningKey;
    use rsa::pkcs8::EncodePublicKey;
    use rsa::rand_core::OsRng;
    use rsa::signature::{RandomizedSigner, SignatureEncoding};

    fn keypair() -> (RsaPrivateKey, String) {
        let private = RsaPrivateKey::new(&mut OsRng, 1024).unwrap();
        let pub_pem = private
            .to_public_key()
            .to_public_key_pem(rsa::pkcs8::LineEnding::LF)
            .unwrap();
        (private, pub_pem)
    }

    fn sign(private: &RsaPrivateKey, body: &[u8]) -> String {
        let signing_key = SigningKey::<Sha512>::new(private.clone());
        let sig = signing_key.sign_with_rng(&mut OsRng, body);
        base64::engine::general_purpose::STANDARD.encode(sig.to_bytes())
    }

    #[test]
    fn fireblocks_credential_picks_sandbox_for_non_prod_env() {
        let mut cred = FireblocksCredential {
            api_key: "k".into(),
            api_secret_pem: "p".into(),
            webhook_public_key_pem: None,
            environment: "sandbox".into(),
        };
        assert_eq!(cred.api_base(), "https://sandbox-api.fireblocks.io");
        cred.environment = "production".into();
        assert_eq!(cred.api_base(), "https://api.fireblocks.io");
        cred.environment = "live".into(); // case-insensitive synonym
        assert_eq!(cred.api_base(), "https://api.fireblocks.io");
    }

    #[test]
    fn verifies_genuine_fireblocks_signature() {
        let (priv_, pub_pem) = keypair();
        let body = br#"{"type":"TRANSACTION_CREATED","data":{"id":"x"}}"#;
        let sig = sign(&priv_, body);
        verify_webhook_signature(body, &sig, &pub_pem).unwrap();
    }

    #[test]
    fn rejects_tampered_fireblocks_body() {
        let (priv_, pub_pem) = keypair();
        let body = br#"{"amount":100}"#;
        let sig = sign(&priv_, body);
        assert!(matches!(
            verify_webhook_signature(b"{\"amount\":99999}", &sig, &pub_pem).unwrap_err(),
            AppError::Unauthorized
        ));
    }

    #[test]
    fn rejects_sig_from_different_keypair() {
        let (_a_priv, pub_a) = keypair();
        let (b_priv, _b_pub) = keypair();
        let body = br#"{"x":1}"#;
        let sig = sign(&b_priv, body);
        assert!(matches!(
            verify_webhook_signature(body, &sig, &pub_a).unwrap_err(),
            AppError::Unauthorized
        ));
    }

    #[test]
    fn rejects_malformed_pem() {
        let err = verify_webhook_signature(b"x", "AA==", "not a pem").unwrap_err();
        assert!(matches!(err, AppError::Crypto(_)));
    }

    #[test]
    fn rejects_malformed_signature_b64() {
        let (_, pub_pem) = keypair();
        let err = verify_webhook_signature(b"x", "@@not-base64@@", &pub_pem).unwrap_err();
        assert!(matches!(err, AppError::Crypto(_)));
    }
}
