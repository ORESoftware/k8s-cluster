//! Plaid Bank — observer-mode (Plaid Link, not standard OAuth).
//!
//! Connection model:
//!   1. Tenant clicks "Connect bank" in their dashboard.
//!   2. Frontend asks us for a Plaid Link token; we POST /link/token/create
//!      with our `client_id` + `secret` and the tenant_id as `client_user_id`.
//!   3. Frontend opens Plaid Link with that token. User picks bank + signs in
//!      with bank creds inside Plaid's iframe; we NEVER see them.
//!   4. Plaid Link returns a `public_token` to our callback.
//!   5. We POST /item/public_token/exchange to get a long-lived `access_token`
//!      and `item_id`. We seal these per tenant per institution.
//!   6. We poll /transactions/sync with the access_token + cursor.
//!
//! One tenant can have up to N (we target 10) institutions; each gets its
//! own `provider_connections` row.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::config::Config;
use crate::error::{AppError, AppResult};

use super::oauth_common::CodeExchangeResult;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlaidCredential {
    pub access_token: String,
    pub item_id: String,
    pub institution_id: Option<String>,
    pub institution_name: Option<String>,
}

pub struct PlaidLink<'a> {
    cfg: &'a Config,
}

#[derive(Debug, Serialize)]
struct LinkTokenCreateReq<'a> {
    client_id: &'a str,
    secret: &'a str,
    client_name: &'a str,
    language: &'a str,
    country_codes: Vec<&'a str>,
    products: Vec<&'a str>,
    user: LinkUser<'a>,
    webhook: Option<String>,
}
#[derive(Debug, Serialize)]
struct LinkUser<'a> {
    client_user_id: &'a str,
}
#[derive(Debug, Deserialize)]
struct LinkTokenCreateResp {
    link_token: String,
    #[allow(dead_code)]
    expiration: Option<String>,
}

#[derive(Debug, Serialize)]
struct ExchangeReq<'a> {
    client_id: &'a str,
    secret: &'a str,
    public_token: &'a str,
}
#[derive(Debug, Deserialize)]
struct ExchangeResp {
    access_token: String,
    item_id: String,
}

#[derive(Debug, Deserialize)]
struct PlaidErr {
    error_code: Option<String>,
    error_message: Option<String>,
    error_type: Option<String>,
}

impl<'a> PlaidLink<'a> {
    pub fn new(cfg: &'a Config) -> Self {
        Self { cfg }
    }

    fn base(&self) -> &'static str {
        self.cfg.plaid_api_base()
    }

    pub async fn create_link_token(&self, tenant_id: uuid::Uuid) -> AppResult<String> {
        let client_id = self
            .cfg
            .plaid_client_id
            .as_ref()
            .ok_or_else(|| AppError::BadRequest("PLAID_CLIENT_ID not configured".into()))?;
        let secret = self
            .cfg
            .plaid_secret
            .as_ref()
            .ok_or_else(|| AppError::BadRequest("PLAID_SECRET not configured".into()))?;

        let tenant_id_s = tenant_id.to_string();
        let webhook = Some(format!(
            "{}/v1/webhooks/plaid",
            self.cfg.oauth_redirect_base
        ));
        let body = LinkTokenCreateReq {
            client_id,
            secret,
            client_name: "billing-server",
            language: "en",
            country_codes: vec!["US"],
            products: vec!["transactions"],
            user: LinkUser {
                client_user_id: &tenant_id_s,
            },
            webhook,
        };

        let resp = reqwest::Client::new()
            .post(format!("{}/link/token/create", self.base()))
            .json(&body)
            .send()
            .await
            .map_err(|e| AppError::Provider {
                provider: "plaid".into(),
                message: format!("link/token/create HTTP: {e}"),
            })?;

        let status = resp.status();
        let bytes = resp.bytes().await.map_err(|e| AppError::Provider {
            provider: "plaid".into(),
            message: format!("link/token/create body: {e}"),
        })?;
        if !status.is_success() {
            return Err(plaid_err("link/token/create", status, &bytes));
        }
        let parsed: LinkTokenCreateResp =
            serde_json::from_slice(&bytes).map_err(|e| AppError::Provider {
                provider: "plaid".into(),
                message: format!("link/token/create decode: {e}"),
            })?;
        Ok(parsed.link_token)
    }

    /// Exchange the Plaid Link `public_token` for a permanent `access_token`.
    pub async fn exchange_public_token(
        &self,
        public_token: &str,
        institution_id: Option<&str>,
        institution_name: Option<&str>,
    ) -> AppResult<CodeExchangeResult> {
        let client_id = self
            .cfg
            .plaid_client_id
            .as_ref()
            .ok_or_else(|| AppError::BadRequest("PLAID_CLIENT_ID not configured".into()))?;
        let secret = self
            .cfg
            .plaid_secret
            .as_ref()
            .ok_or_else(|| AppError::BadRequest("PLAID_SECRET not configured".into()))?;

        let resp = reqwest::Client::new()
            .post(format!("{}/item/public_token/exchange", self.base()))
            .json(&ExchangeReq {
                client_id,
                secret,
                public_token,
            })
            .send()
            .await
            .map_err(|e| AppError::Provider {
                provider: "plaid".into(),
                message: format!("public_token/exchange HTTP: {e}"),
            })?;

        let status = resp.status();
        let bytes = resp.bytes().await.map_err(|e| AppError::Provider {
            provider: "plaid".into(),
            message: format!("public_token/exchange body: {e}"),
        })?;
        if !status.is_success() {
            return Err(plaid_err("public_token/exchange", status, &bytes));
        }
        let parsed: ExchangeResp =
            serde_json::from_slice(&bytes).map_err(|e| AppError::Provider {
                provider: "plaid".into(),
                message: format!("public_token/exchange decode: {e}"),
            })?;

        let cred = PlaidCredential {
            access_token: parsed.access_token,
            item_id: parsed.item_id.clone(),
            institution_id: institution_id.map(String::from),
            institution_name: institution_name.map(String::from),
        };
        let plaintext = serde_json::to_vec(&cred).map_err(|e| AppError::Provider {
            provider: "plaid".into(),
            message: format!("seal-encode: {e}"),
        })?;

        Ok(CodeExchangeResult {
            external_account_id: parsed.item_id.clone(),
            sealed_plaintext: plaintext,
            scopes: vec!["transactions".into()],
            // Plaid access_tokens don't expire.
            expires_at: None,
            display_label_suggestion: Some(
                institution_name
                    .map(|n| format!("Plaid {n}"))
                    .unwrap_or_else(|| format!("Plaid {}", parsed.item_id)),
            ),
        })
    }
}

// =========================================================================
// /transactions/sync — delta API for posting txs to the ledger
// =========================================================================

#[derive(Debug, Serialize)]
struct TxSyncReq<'a> {
    client_id: &'a str,
    secret: &'a str,
    access_token: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    cursor: Option<&'a str>,
    count: i32,
}

#[derive(Clone, Debug, Deserialize)]
pub struct PlaidTransaction {
    pub transaction_id: String,
    pub account_id: String,
    /// Plaid sign convention: positive = outflow from account, negative
    /// = inflow. We invert this in the normalizer so the ledger has the
    /// "natural" sign.
    pub amount: f64,
    pub iso_currency_code: Option<String>,
    pub unofficial_currency_code: Option<String>,
    pub date: Option<String>,
    pub authorized_date: Option<String>,
    pub name: Option<String>,
    pub merchant_name: Option<String>,
    pub pending: Option<bool>,
    pub payment_channel: Option<String>,
    pub category: Option<Vec<String>>,
    pub category_id: Option<String>,
    #[serde(flatten)]
    pub raw: serde_json::Value,
}

#[derive(Clone, Debug, Deserialize)]
pub struct PlaidRemovedTx {
    pub transaction_id: String,
    pub account_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct PlaidSyncPage {
    pub added: Vec<PlaidTransaction>,
    pub modified: Vec<PlaidTransaction>,
    pub removed: Vec<PlaidRemovedTx>,
    pub next_cursor: String,
    pub has_more: bool,
}

impl<'a> PlaidLink<'a> {
    /// `POST /transactions/sync` — Plaid's delta API for transaction
    /// updates. Returns a page of added/modified/removed events plus the
    /// `next_cursor` to feed into the following call. `has_more` is true
    /// while there are more pages in the current incremental update.
    pub async fn sync_transactions(
        &self,
        access_token: &str,
        cursor: Option<&str>,
        count: i32,
    ) -> AppResult<PlaidSyncPage> {
        let client_id = self
            .cfg
            .plaid_client_id
            .as_ref()
            .ok_or_else(|| AppError::BadRequest("PLAID_CLIENT_ID not configured".into()))?;
        let secret = self
            .cfg
            .plaid_secret
            .as_ref()
            .ok_or_else(|| AppError::BadRequest("PLAID_SECRET not configured".into()))?;

        let body = TxSyncReq {
            client_id,
            secret,
            access_token,
            cursor,
            count,
        };
        let resp = reqwest::Client::new()
            .post(format!("{}/transactions/sync", self.base()))
            .json(&body)
            .send()
            .await
            .map_err(|e| AppError::Provider {
                provider: "plaid".into(),
                message: format!("transactions/sync HTTP: {e}"),
            })?;
        let status = resp.status();
        let bytes = resp.bytes().await.map_err(|e| AppError::Provider {
            provider: "plaid".into(),
            message: format!("transactions/sync body: {e}"),
        })?;
        if !status.is_success() {
            return Err(plaid_err("transactions/sync", status, &bytes));
        }
        let parsed: PlaidSyncPage =
            serde_json::from_slice(&bytes).map_err(|e| AppError::Provider {
                provider: "plaid".into(),
                message: format!("transactions/sync decode: {e}"),
            })?;
        Ok(parsed)
    }
}

// =========================================================================
// Webhook verification (Plaid-Verification JWT, ES256, JWKS-cached)
// =========================================================================
//
// Plaid sends each webhook with a `Plaid-Verification` header containing
// a signed JWT (ES256). The JWT's `kid` identifies the key; we fetch
// the public JWK from Plaid's `/webhook_verification_key/get` endpoint
// (authenticated with our client_id + secret) and cache it by `kid`.
//
// JWT claim `request_body_sha256` must equal SHA256(raw_body).
// JWT claim `iat` (issued-at) must be within 5 minutes of now.

#[derive(Debug, Deserialize)]
struct PlaidJwk {
    alg: Option<String>,
    kty: String,
    #[serde(rename = "use")]
    use_: Option<String>,
    crv: Option<String>,
    x: Option<String>,
    y: Option<String>,
    kid: String,
    #[serde(default)]
    expired_at: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct WebhookKeyGetResp {
    key: PlaidJwk,
}

#[derive(Debug, Serialize)]
struct WebhookKeyGetReq<'a> {
    client_id: &'a str,
    secret: &'a str,
    key_id: &'a str,
}

#[derive(Clone)]
struct CachedKey {
    jwk: Arc<PlaidJwk>,
    fetched_at: DateTime<Utc>,
}

#[derive(Clone, Default)]
pub struct PlaidWebhookVerifier {
    keys: Arc<RwLock<HashMap<String, CachedKey>>>,
}

impl PlaidWebhookVerifier {
    pub fn new() -> Self {
        Self::default()
    }

    /// Verify a `Plaid-Verification` JWT against the raw body.
    ///
    /// Implements the steps from the Plaid docs:
    ///   1. Parse header to get `kid` + `alg=ES256`
    ///   2. Fetch the JWK for that `kid` (cached, ttl 1h)
    ///   3. Verify the JWT signature with ES256 over the public key
    ///   4. Verify `iat` is within `iat_skew_seconds`
    ///   5. Verify `request_body_sha256` matches sha256(raw_body)
    pub async fn verify(
        &self,
        cfg: &Config,
        jwt_header_value: &str,
        raw_body: &[u8],
        iat_skew_seconds: i64,
    ) -> AppResult<()> {
        use base64::Engine as _;
        use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};

        let header = decode_header(jwt_header_value).map_err(|e| {
            AppError::Provider {
                provider: "plaid".into(),
                message: format!("plaid jwt header decode: {e}"),
            }
        })?;
        if !matches!(header.alg, Algorithm::ES256) {
            return Err(AppError::Provider {
                provider: "plaid".into(),
                message: format!("plaid jwt alg must be ES256, got {:?}", header.alg),
            });
        }
        let kid = header.kid.ok_or_else(|| AppError::Provider {
            provider: "plaid".into(),
            message: "plaid jwt header missing kid".into(),
        })?;

        let jwk = self.get_jwk(cfg, &kid).await?;
        if jwk.kty != "EC" || jwk.crv.as_deref() != Some("P-256") {
            return Err(AppError::Provider {
                provider: "plaid".into(),
                message: format!(
                    "plaid jwk wrong type kty={} crv={:?}",
                    jwk.kty, jwk.crv
                ),
            });
        }
        let x = jwk.x.as_deref().ok_or_else(|| AppError::Provider {
            provider: "plaid".into(),
            message: "plaid jwk missing x".into(),
        })?;
        let y = jwk.y.as_deref().ok_or_else(|| AppError::Provider {
            provider: "plaid".into(),
            message: "plaid jwk missing y".into(),
        })?;
        let key = DecodingKey::from_ec_components(x, y).map_err(|e| AppError::Provider {
            provider: "plaid".into(),
            message: format!("plaid decoding key from x/y: {e}"),
        })?;

        let mut validation = Validation::new(Algorithm::ES256);
        validation.required_spec_claims.clear();
        validation.validate_exp = false;
        let token_data = decode::<serde_json::Value>(jwt_header_value, &key, &validation)
            .map_err(|e| AppError::Provider {
                provider: "plaid".into(),
                message: format!("plaid jwt verify: {e}"),
            })?;

        let claims = &token_data.claims;
        let claim_iat = claims.get("iat").and_then(|v| v.as_i64()).ok_or_else(|| {
            AppError::Provider {
                provider: "plaid".into(),
                message: "plaid jwt missing iat".into(),
            }
        })?;
        let now = Utc::now().timestamp();
        if (now - claim_iat).abs() > iat_skew_seconds {
            return Err(AppError::Unauthorized);
        }

        let claim_sha = claims
            .get("request_body_sha256")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AppError::Provider {
                provider: "plaid".into(),
                message: "plaid jwt missing request_body_sha256".into(),
            })?;

        let actual = hex::encode(Sha256::digest(raw_body));
        if !constant_time_eq_str(claim_sha, &actual) {
            // Try base64 url-safe — Plaid uses hex but be lenient.
            let actual_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD
                .encode(Sha256::digest(raw_body));
            if !constant_time_eq_str(claim_sha, &actual_b64) {
                return Err(AppError::Unauthorized);
            }
        }
        Ok(())
    }

    /// Seed a JWK into the verifier cache. Test-only: lets us bypass
    /// the `fetch_jwk` HTTPS call so unit tests run offline.
    #[cfg(test)]
    pub(crate) fn insert_jwk_for_tests(&self, kid: &str, x_b64u: &str, y_b64u: &str) {
        let jwk = PlaidJwk {
            alg: Some("ES256".into()),
            kty: "EC".into(),
            use_: Some("sig".into()),
            crv: Some("P-256".into()),
            x: Some(x_b64u.to_string()),
            y: Some(y_b64u.to_string()),
            kid: kid.to_string(),
            expired_at: None,
        };
        let mut map = self.keys.write().unwrap();
        map.insert(
            kid.to_string(),
            CachedKey {
                jwk: Arc::new(jwk),
                fetched_at: Utc::now(),
            },
        );
    }

    async fn get_jwk(&self, cfg: &Config, kid: &str) -> AppResult<Arc<PlaidJwk>> {
        const TTL_SECONDS: i64 = 3600;
        let now = Utc::now();
        if let Ok(map) = self.keys.read() {
            if let Some(c) = map.get(kid) {
                if (now - c.fetched_at).num_seconds() < TTL_SECONDS {
                    return Ok(c.jwk.clone());
                }
            }
        }
        let jwk = fetch_jwk(cfg, kid).await?;
        let arc = Arc::new(jwk);
        if let Ok(mut map) = self.keys.write() {
            map.insert(
                kid.to_string(),
                CachedKey {
                    jwk: arc.clone(),
                    fetched_at: now,
                },
            );
        }
        Ok(arc)
    }
}

async fn fetch_jwk(cfg: &Config, kid: &str) -> AppResult<PlaidJwk> {
    let client_id = cfg
        .plaid_client_id
        .as_ref()
        .ok_or_else(|| AppError::BadRequest("PLAID_CLIENT_ID not configured".into()))?;
    let secret = cfg
        .plaid_secret
        .as_ref()
        .ok_or_else(|| AppError::BadRequest("PLAID_SECRET not configured".into()))?;
    let resp = reqwest::Client::new()
        .post(format!("{}/webhook_verification_key/get", cfg.plaid_api_base()))
        .json(&WebhookKeyGetReq {
            client_id,
            secret,
            key_id: kid,
        })
        .send()
        .await
        .map_err(|e| AppError::Provider {
            provider: "plaid".into(),
            message: format!("webhook_verification_key/get HTTP: {e}"),
        })?;
    let status = resp.status();
    let bytes = resp.bytes().await.map_err(|e| AppError::Provider {
        provider: "plaid".into(),
        message: format!("webhook_verification_key/get body: {e}"),
    })?;
    if !status.is_success() {
        return Err(plaid_err("webhook_verification_key/get", status, &bytes));
    }
    let parsed: WebhookKeyGetResp =
        serde_json::from_slice(&bytes).map_err(|e| AppError::Provider {
            provider: "plaid".into(),
            message: format!("webhook_verification_key/get decode: {e}"),
        })?;
    Ok(parsed.key)
}

fn constant_time_eq_str(a: &str, b: &str) -> bool {
    let ab = a.as_bytes();
    let bb = b.as_bytes();
    if ab.len() != bb.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (x, y) in ab.iter().zip(bb.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

fn plaid_err(op: &str, status: reqwest::StatusCode, bytes: &[u8]) -> AppError {
    let err: PlaidErr = serde_json::from_slice(bytes).unwrap_or(PlaidErr {
        error_code: Some(format!("http {status}")),
        error_message: Some(String::from_utf8_lossy(bytes).into_owned()),
        error_type: None,
    });
    AppError::Provider {
        provider: "plaid".into(),
        message: format!(
            "{op} failed [{}/{}]: {}",
            err.error_type.unwrap_or_else(|| "?".into()),
            err.error_code.unwrap_or_else(|| "?".into()),
            err.error_message.unwrap_or_default()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;
    use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};

    // Static ES256 keypair generated offline. Test-only — never used
    // to protect anything real. We pre-bake it in PKCS8 PEM form because
    // `jsonwebtoken::EncodingKey::from_ec_pem` expects PKCS8 and not
    // the legacy SEC1 `EC PRIVATE KEY` format.
    const TEST_PRIVATE_KEY_PEM: &str = "\
-----BEGIN PRIVATE KEY-----
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQg7ZIs8ilH42biKVRr
AMdEs1IlsV5nc5Whrj7piz1ffN2hRANCAAQc26oKhK9zzwmkqvtzejHj6IxW53T1
Xzfy3kQ0pAIzDUtAuvoIhMUqvm2T2hU+RfJkGA157hxPYy14rH0nmOeN
-----END PRIVATE KEY-----
";

    // Base64url-encoded X and Y coordinates extracted from the public
    // key above (DER bytes 27..91).
    const TEST_X: &str = "HNuqCoSvc88JpKr7c3ox4-iMVud09V838t5ENKQCMw0";
    const TEST_Y: &str = "S0C6-giExSq-bZPaFT5F8mQYDXnuHE9jLXisfSeY540";
    const TEST_KID: &str = "test-kid";

    fn make_jwt(claims: &serde_json::Value) -> String {
        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some(TEST_KID.into());
        let key = EncodingKey::from_ec_pem(TEST_PRIVATE_KEY_PEM.as_bytes()).unwrap();
        encode(&header, claims, &key).unwrap()
    }

    fn fixture_cfg() -> Config {
        // PlaidWebhookVerifier::verify takes a Config but only reads
        // it inside fetch_jwk, which we bypass via the cache. So any
        // shape that builds is fine.
        Config::for_tests()
    }

    #[tokio::test]
    async fn verifies_genuine_plaid_jwt() {
        let body = br#"{"webhook_type":"TRANSACTIONS","webhook_code":"DEFAULT_UPDATE"}"#;
        let sha = hex::encode(Sha256::digest(body));
        let jwt = make_jwt(&serde_json::json!({
            "iat": Utc::now().timestamp(),
            "request_body_sha256": sha,
        }));
        let v = PlaidWebhookVerifier::new();
        v.insert_jwk_for_tests(TEST_KID, TEST_X, TEST_Y);
        v.verify(&fixture_cfg(), &jwt, body, 300).await.unwrap();
    }

    #[tokio::test]
    async fn rejects_tampered_body() {
        // SHA in the token is for body A, but we verify against body B.
        let signed_body = br#"{"a":1}"#;
        let sha = hex::encode(Sha256::digest(signed_body));
        let jwt = make_jwt(&serde_json::json!({
            "iat": Utc::now().timestamp(),
            "request_body_sha256": sha,
        }));
        let v = PlaidWebhookVerifier::new();
        v.insert_jwk_for_tests(TEST_KID, TEST_X, TEST_Y);
        let err = v
            .verify(&fixture_cfg(), &jwt, b"{\"a\":2}", 300)
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::Unauthorized));
    }

    #[tokio::test]
    async fn rejects_stale_iat() {
        let body = b"{}";
        let sha = hex::encode(Sha256::digest(body));
        let stale = Utc::now().timestamp() - 10_000;
        let jwt = make_jwt(&serde_json::json!({
            "iat": stale,
            "request_body_sha256": sha,
        }));
        let v = PlaidWebhookVerifier::new();
        v.insert_jwk_for_tests(TEST_KID, TEST_X, TEST_Y);
        let err = v.verify(&fixture_cfg(), &jwt, body, 300).await.unwrap_err();
        assert!(matches!(err, AppError::Unauthorized));
    }

    #[tokio::test]
    async fn rejects_future_iat() {
        let body = b"{}";
        let sha = hex::encode(Sha256::digest(body));
        let future = Utc::now().timestamp() + 10_000;
        let jwt = make_jwt(&serde_json::json!({
            "iat": future,
            "request_body_sha256": sha,
        }));
        let v = PlaidWebhookVerifier::new();
        v.insert_jwk_for_tests(TEST_KID, TEST_X, TEST_Y);
        assert!(matches!(
            v.verify(&fixture_cfg(), &jwt, body, 300).await.unwrap_err(),
            AppError::Unauthorized
        ));
    }

    #[tokio::test]
    async fn accepts_b64url_sha_format_as_fallback() {
        // Plaid docs say hex; we also accept base64url-no-pad for
        // resilience. Verify the b64 fallback path works.
        let body = b"hello";
        let sha_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(Sha256::digest(body));
        let jwt = make_jwt(&serde_json::json!({
            "iat": Utc::now().timestamp(),
            "request_body_sha256": sha_b64,
        }));
        let v = PlaidWebhookVerifier::new();
        v.insert_jwk_for_tests(TEST_KID, TEST_X, TEST_Y);
        v.verify(&fixture_cfg(), &jwt, body, 300).await.unwrap();
    }

    #[tokio::test]
    async fn rejects_missing_request_body_sha256() {
        let jwt = make_jwt(&serde_json::json!({
            "iat": Utc::now().timestamp(),
        }));
        let v = PlaidWebhookVerifier::new();
        v.insert_jwk_for_tests(TEST_KID, TEST_X, TEST_Y);
        let err = v.verify(&fixture_cfg(), &jwt, b"{}", 300).await.unwrap_err();
        assert!(matches!(err, AppError::Provider { .. }));
    }

    #[tokio::test]
    async fn rejects_missing_iat() {
        let jwt = make_jwt(&serde_json::json!({
            "request_body_sha256": hex::encode(Sha256::digest(b"{}")),
        }));
        let v = PlaidWebhookVerifier::new();
        v.insert_jwk_for_tests(TEST_KID, TEST_X, TEST_Y);
        let err = v.verify(&fixture_cfg(), &jwt, b"{}", 300).await.unwrap_err();
        assert!(matches!(err, AppError::Provider { .. }));
    }

    #[tokio::test]
    async fn rejects_wrong_kid() {
        let body = b"{}";
        let sha = hex::encode(Sha256::digest(body));
        let jwt = make_jwt(&serde_json::json!({
            "iat": Utc::now().timestamp(),
            "request_body_sha256": sha,
        }));
        let v = PlaidWebhookVerifier::new();
        // Cache holds a different kid, so verify will try to fetch
        // — and fetch_jwk will fail because no plaid_client_id is set
        // in the test config.
        v.insert_jwk_for_tests("other-kid", TEST_X, TEST_Y);
        let err = v.verify(&fixture_cfg(), &jwt, body, 300).await.unwrap_err();
        // Either BadRequest (missing client_id) or Provider — both
        // are acceptable as "couldn't verify". Crucially: NOT Ok.
        assert!(matches!(
            err,
            AppError::BadRequest(_) | AppError::Provider { .. }
        ));
    }
}
