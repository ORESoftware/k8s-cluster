//! Coinbase Commerce / Coinbase Prime — observer-mode (API key auth).
//!
//! Coinbase Commerce uses an API key + webhook signing secret. There is no
//! OAuth flow for Commerce, so the tenant generates a key in their Coinbase
//! dashboard and pastes it into our connect screen. The key is sealed
//! immediately and never leaves the server in plaintext.
//!
//! Sync path: walk `GET /charges?limit=N&starting_after=<id>` paginated
//! and normalize each `charge:confirmed` to ledger postings. Webhooks
//! are still primary; this is the backstop.
//!
//! API docs: https://docs.cdp.coinbase.com/commerce/reference

use chrono::{DateTime, Utc};
use hmac::{Hmac, KeyInit, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use crate::error::{AppError, AppResult};
use crate::ledger::{AccountKind, Direction, DraftPosting, DraftTransaction};
use crate::money::Currency;

const COMMERCE_BASE: &str = "https://api.commerce.coinbase.com";
const COMMERCE_VERSION: &str = "2018-03-22";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CoinbaseCredential {
    pub api_key: String,
    pub webhook_secret: String,
    pub variant: CoinbaseVariant,
    /// Prime: HMAC-SHA256 signing secret (base64-encoded). Unused for
    /// Commerce.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_secret: Option<String>,
    /// Prime: API key passphrase chosen when the key was created.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub passphrase: Option<String>,
    /// Prime: portfolio (sub-account) UUID to scope queries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub portfolio_id: Option<String>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CoinbaseVariant {
    Commerce,
    Prime,
}

// --- API client (Commerce v2) ---------------------------------------------

pub struct CoinbaseCommerceApi {
    cred: CoinbaseCredential,
    http: reqwest::Client,
}

#[derive(Clone, Debug, Deserialize)]
pub struct CoinbaseCharge {
    pub id: String,
    pub code: Option<String>,
    pub name: Option<String>,
    pub description: Option<String>,
    pub created_at: Option<String>,
    pub confirmed_at: Option<String>,
    pub pricing: Option<CoinbasePricing>,
    pub timeline: Option<Vec<CoinbaseTimelineEntry>>,
    pub metadata: Option<serde_json::Value>,
    #[serde(flatten)]
    pub raw: serde_json::Value,
}

#[derive(Clone, Debug, Deserialize)]
pub struct CoinbasePricing {
    pub local: Option<CoinbaseMoney>,
    pub bitcoin: Option<CoinbaseMoney>,
    pub ethereum: Option<CoinbaseMoney>,
    pub usdc: Option<CoinbaseMoney>,
    #[serde(flatten)]
    pub other: serde_json::Value,
}

#[derive(Clone, Debug, Deserialize)]
pub struct CoinbaseMoney {
    pub amount: String,
    pub currency: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct CoinbaseTimelineEntry {
    pub time: String,
    pub status: String,
    #[serde(default)]
    pub context: Option<String>,
}

#[derive(Deserialize)]
struct ChargesPage {
    data: Vec<CoinbaseCharge>,
    pagination: Option<ChargesPagination>,
}

#[derive(Deserialize)]
struct ChargesPagination {
    #[serde(default)]
    cursor_range: Option<Vec<String>>,
    #[serde(default)]
    next_uri: Option<String>,
}

impl CoinbaseCommerceApi {
    pub fn new(cred: CoinbaseCredential) -> Self {
        Self {
            cred,
            http: reqwest::Client::new(),
        }
    }

    /// `GET /charges?limit=N&starting_after=<id>` — returns (charges, cursor_next).
    pub async fn list_charges(
        &self,
        limit: u32,
        starting_after: Option<&str>,
    ) -> AppResult<(Vec<CoinbaseCharge>, Option<String>)> {
        let mut params: Vec<(&str, String)> = vec![
            ("limit", limit.to_string()),
            ("order", "asc".to_string()),
        ];
        if let Some(c) = starting_after {
            params.push(("starting_after", c.to_string()));
        }
        let qs = serde_urlencoded::to_string(&params).map_err(|e| AppError::Provider {
            provider: "coinbase_commerce".into(),
            message: format!("encode query: {e}"),
        })?;
        let url = format!("{COMMERCE_BASE}/charges?{qs}");

        let resp = self
            .http
            .get(&url)
            .header("X-CC-Api-Key", &self.cred.api_key)
            .header("X-CC-Version", COMMERCE_VERSION)
            .send()
            .await
            .map_err(|e| AppError::Provider {
                provider: "coinbase_commerce".into(),
                message: format!("charges HTTP: {e}"),
            })?;
        let status = resp.status();
        let bytes = resp.bytes().await.map_err(|e| AppError::Provider {
            provider: "coinbase_commerce".into(),
            message: format!("charges body: {e}"),
        })?;
        if !status.is_success() {
            return Err(AppError::Provider {
                provider: "coinbase_commerce".into(),
                message: format!(
                    "charges {status}: {}",
                    String::from_utf8_lossy(&bytes)
                ),
            });
        }
        let page: ChargesPage =
            serde_json::from_slice(&bytes).map_err(|e| AppError::Provider {
                provider: "coinbase_commerce".into(),
                message: format!("charges decode: {e}"),
            })?;

        let next = page.pagination.as_ref().and_then(|p| {
            p.cursor_range
                .as_ref()
                .and_then(|r| r.get(1).cloned())
                .or_else(|| p.next_uri.clone())
        });

        Ok((page.data, next))
    }
}

// --- Normalization --------------------------------------------------------

const ACCT_CLEARING: &str = "clearing/coinbase_commerce";
const ACCT_REVENUE: &str = "revenue/coinbase_commerce";

#[derive(Clone, Debug)]
pub struct EnsureAccount {
    pub code: String,
    pub kind: AccountKind,
    pub currency: Currency,
}

pub struct NormalizedCharge {
    pub draft: DraftTransaction,
    pub accounts_to_ensure: Vec<EnsureAccount>,
    pub recognized: bool,
}

/// Normalize a Coinbase Commerce charge into a balanced draft transaction.
/// We only post for `COMPLETED` charges; everything else (`NEW`, `PENDING`,
/// `EXPIRED`, `UNRESOLVED`, `CANCELED`) is ignored for posting and surfaced
/// only via `recognized = false` to avoid duplicate / phantom entries.
pub fn normalize_charge(
    charge: &CoinbaseCharge,
    tenant_id: uuid::Uuid,
) -> AppResult<NormalizedCharge> {
    let is_completed = charge
        .timeline
        .as_ref()
        .map(|t| {
            t.iter().any(|e| {
                e.status.eq_ignore_ascii_case("COMPLETED")
                    || e.status.eq_ignore_ascii_case("CONFIRMED")
            })
        })
        .unwrap_or(false);

    let local = charge
        .pricing
        .as_ref()
        .and_then(|p| p.local.as_ref())
        .cloned();

    let posted_at: DateTime<Utc> = charge
        .confirmed_at
        .as_deref()
        .or(charge.created_at.as_deref())
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(Utc::now);

    let mut postings: Vec<DraftPosting> = Vec::new();
    let mut accounts: Vec<EnsureAccount> = Vec::new();
    let mut recognized = false;

    let meta = serde_json::json!({
        "coinbase_commerce_charge_id": charge.id,
        "coinbase_commerce_code": charge.code,
        "coinbase_commerce_name": charge.name,
        "coinbase_commerce_completed": is_completed,
        "coinbase_commerce_metadata": charge.metadata,
    });

    if is_completed {
        if let Some(local) = local {
            let amount_minor = parse_to_minor(&local.amount, &local.currency)?;
            let currency = Currency::new(&local.currency).map_err(|e| AppError::Provider {
                provider: "coinbase_commerce".into(),
                message: format!("unknown local currency {}: {e}", local.currency),
            })?;
            let cur = currency.as_str().to_string();
            postings.push(DraftPosting {
                account_code: ACCT_CLEARING.into(),
                direction: Direction::Debit,
                amount_minor,
                currency: cur.clone(),
                source: "coinbase_commerce".into(),
                source_event_id: charge.id.clone(),
                metadata: meta.clone(),
            });
            postings.push(DraftPosting {
                account_code: ACCT_REVENUE.into(),
                direction: Direction::Credit,
                amount_minor,
                currency: cur.clone(),
                source: "coinbase_commerce".into(),
                source_event_id: charge.id.clone(),
                metadata: meta.clone(),
            });
            accounts.push(EnsureAccount {
                code: ACCT_CLEARING.into(),
                kind: AccountKind::Asset,
                currency: currency.clone(),
            });
            accounts.push(EnsureAccount {
                code: ACCT_REVENUE.into(),
                kind: AccountKind::Income,
                currency,
            });
            recognized = true;
        }
    }

    let description = Some(format!(
        "coinbase_commerce charge {} ({}) at {}",
        charge.id,
        if is_completed { "completed" } else { "open" },
        posted_at.format("%Y-%m-%dT%H:%M:%SZ"),
    ));

    let draft = DraftTransaction {
        tenant_id,
        kind: "coinbase_commerce.charge".into(),
        idempotency_key: format!("coinbase_commerce:chg:{}", charge.id),
        description,
        metadata: meta,
        postings,
    };

    Ok(NormalizedCharge {
        draft,
        accounts_to_ensure: accounts,
        recognized,
    })
}

/// Convert a Coinbase Commerce "1234.56"/"USD" string pair to minor units
/// (cents). Crypto amounts come back with many fractional digits and
/// should not be passed through here — use them as observability only.
fn parse_to_minor(amount: &str, currency: &str) -> AppResult<i128> {
    if !is_fiat_like(currency) {
        return Err(AppError::Provider {
            provider: "coinbase_commerce".into(),
            message: format!(
                "{currency} is not fiat; charge skipped for ledger posting"
            ),
        });
    }
    let cleaned = amount.replace([',', '_'], "");
    let parts: Vec<&str> = cleaned.split('.').collect();
    let (whole, frac) = match parts.as_slice() {
        [w] => (*w, "00".to_string()),
        [w, f] => {
            let f = if f.len() >= 2 {
                f[..2].to_string()
            } else {
                format!("{f}{}", "0".repeat(2 - f.len()))
            };
            (*w, f)
        }
        _ => {
            return Err(AppError::Provider {
                provider: "coinbase_commerce".into(),
                message: format!("malformed amount {amount}"),
            })
        }
    };
    let whole_i: i128 = whole.parse().map_err(|e| AppError::Provider {
        provider: "coinbase_commerce".into(),
        message: format!("amount whole {whole}: {e}"),
    })?;
    let frac_i: i128 = frac.parse().map_err(|e| AppError::Provider {
        provider: "coinbase_commerce".into(),
        message: format!("amount frac {frac}: {e}"),
    })?;
    Ok(whole_i * 100 + frac_i)
}

fn is_fiat_like(c: &str) -> bool {
    matches!(
        c,
        "USD" | "EUR" | "GBP" | "CAD" | "AUD" | "JPY" | "CHF" | "SEK" | "NOK" | "DKK" | "PLN"
    )
}

// --- Webhook signature verification ---------------------------------------

pub fn verify_commerce_signature(
    payload: &[u8],
    header_sig: &str,
    shared_secret: &str,
) -> AppResult<()> {
    let provided = parse_provided(header_sig);
    let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(shared_secret.as_bytes())
        .map_err(|e| AppError::Crypto(format!("hmac init: {e}")))?;
    Mac::update(&mut mac, payload);
    let expected = hex::encode(Mac::finalize(mac).into_bytes());
    if constant_time_eq_str(&provided, &expected) {
        Ok(())
    } else {
        Err(AppError::Unauthorized)
    }
}

fn parse_provided(header: &str) -> String {
    let trimmed = header.trim();
    if let Some(rest) = trimmed.strip_prefix("sha256=") {
        return rest.to_string();
    }
    if let Some(rest) = trimmed.strip_prefix("v1=") {
        return rest.to_string();
    }
    trimmed.to_string()
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

// =========================================================================
// Coinbase Prime — institutional REST API (different shape from Commerce)
// =========================================================================
//
// Auth: `X-CB-ACCESS-{KEY,SIGNATURE,TIMESTAMP,PASSPHRASE}` headers,
// where SIGNATURE = base64(HMAC-SHA256(api_secret_b64_decoded,
// "{timestamp}{METHOD}{path_with_query}{body}")).
//
// Docs: https://docs.cdp.coinbase.com/prime/reference

const PRIME_BASE: &str = "https://api.prime.coinbase.com";

pub struct CoinbasePrimeApi {
    cred: CoinbaseCredential,
    http: reqwest::Client,
}

#[derive(Clone, Debug, Deserialize)]
pub struct PrimeTransactionsPage {
    #[serde(default)]
    pub transactions: Vec<PrimeTransaction>,
    pub pagination: Option<PrimePagination>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct PrimePagination {
    pub next_cursor: Option<String>,
    #[serde(default)]
    pub has_next: bool,
}

#[derive(Clone, Debug, Deserialize)]
pub struct PrimeTransaction {
    pub id: String,
    /// One of: DEPOSIT, WITHDRAWAL, REWARD, FEE, INTERNAL, CONVERSION, ...
    #[serde(rename = "type")]
    pub type_: String,
    /// One of: TRANSACTION_CREATED, TRANSACTION_REQUESTED,
    /// TRANSACTION_APPROVED, TRANSACTION_PROCESSING,
    /// TRANSACTION_COMPLETED, TRANSACTION_FAILED, TRANSACTION_CANCELED.
    pub status: String,
    pub created_at: Option<String>,
    pub completed_at: Option<String>,
    pub symbol: Option<String>,
    pub amount: Option<String>,
    pub destination: Option<String>,
    pub network_fees: Option<String>,
    pub wallet_id: Option<String>,
    #[serde(flatten)]
    pub raw: serde_json::Value,
}

impl CoinbasePrimeApi {
    pub fn new(cred: CoinbaseCredential) -> AppResult<Self> {
        if cred.variant != CoinbaseVariant::Prime {
            return Err(AppError::BadRequest(
                "CoinbasePrimeApi requires variant=Prime".into(),
            ));
        }
        Ok(Self {
            cred,
            http: reqwest::Client::new(),
        })
    }

    pub fn portfolio_id(&self) -> AppResult<&str> {
        self.cred
            .portfolio_id
            .as_deref()
            .ok_or_else(|| AppError::BadRequest("coinbase prime portfolio_id missing".into()))
    }

    /// Sign + execute a GET against Coinbase Prime.
    async fn signed_get(
        &self,
        path_with_query: &str,
    ) -> AppResult<Vec<u8>> {
        let api_secret_b64 = self.cred.api_secret.as_deref().ok_or_else(|| {
            AppError::BadRequest("coinbase prime api_secret missing".into())
        })?;
        let passphrase = self.cred.passphrase.as_deref().ok_or_else(|| {
            AppError::BadRequest("coinbase prime passphrase missing".into())
        })?;

        let timestamp = Utc::now().timestamp().to_string();
        let signature =
            prime_request_signature(api_secret_b64, &timestamp, "GET", path_with_query, b"")?;

        let url = format!("{PRIME_BASE}{path_with_query}");
        let resp = self
            .http
            .get(&url)
            .header("X-CB-ACCESS-KEY", &self.cred.api_key)
            .header("X-CB-ACCESS-PASSPHRASE", passphrase)
            .header("X-CB-ACCESS-SIGNATURE", signature)
            .header("X-CB-ACCESS-TIMESTAMP", timestamp)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| AppError::Provider {
                provider: "coinbase_prime".into(),
                message: format!("HTTP {path_with_query}: {e}"),
            })?;
        let status = resp.status();
        let bytes = resp.bytes().await.map_err(|e| AppError::Provider {
            provider: "coinbase_prime".into(),
            message: format!("body {path_with_query}: {e}"),
        })?;
        if !status.is_success() {
            return Err(AppError::Provider {
                provider: "coinbase_prime".into(),
                message: format!(
                    "{path_with_query} {status}: {}",
                    String::from_utf8_lossy(&bytes)
                ),
            });
        }
        Ok(bytes.to_vec())
    }

    /// `GET /v1/portfolios/{portfolio_id}/transactions?cursor=<>` —
    /// cursor-paginated transaction history for a Prime portfolio.
    pub async fn list_transactions(
        &self,
        cursor: Option<&str>,
        limit: u32,
    ) -> AppResult<PrimeTransactionsPage> {
        let portfolio_id = self.portfolio_id()?.to_string();
        let mut path = format!(
            "/v1/portfolios/{portfolio_id}/transactions?limit={limit}"
        );
        if let Some(c) = cursor {
            path.push_str(&format!("&cursor={c}"));
        }
        let bytes = self.signed_get(&path).await?;
        let parsed: PrimeTransactionsPage =
            serde_json::from_slice(&bytes).map_err(|e| AppError::Provider {
                provider: "coinbase_prime".into(),
                message: format!("transactions decode: {e}"),
            })?;
        Ok(parsed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verifies_coinbase_hmac() {
        let payload = br#"{"id":"evt","type":"charge:confirmed"}"#;
        let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(b"secret").unwrap();
        Mac::update(&mut mac, payload);
        let sig = hex::encode(Mac::finalize(mac).into_bytes());

        verify_commerce_signature(payload, &sig, "secret").unwrap();
    }

    #[test]
    fn rejects_bad_coinbase_hmac() {
        let err = verify_commerce_signature(b"{}", "deadbeef", "secret").unwrap_err();
        assert!(matches!(err, AppError::Unauthorized));
    }

    #[test]
    fn prime_signature_is_deterministic() {
        // Same inputs ⇒ same MAC. This is the core property of any HMAC
        // — if it were ever salted, signed requests would fail to verify
        // on the server side.
        let secret = "c2hoaGgK"; // "shhhh\n" b64
        let s1 = prime_request_signature(secret, "100", "GET", "/v1/x", b"").unwrap();
        let s2 = prime_request_signature(secret, "100", "GET", "/v1/x", b"").unwrap();
        assert_eq!(s1, s2);
        // Output is base64.
        use base64::Engine as _;
        assert!(base64::engine::general_purpose::STANDARD.decode(&s1).is_ok());
    }

    #[test]
    fn prime_signature_depends_on_every_field() {
        let secret = "c2hoaGgK";
        let base = prime_request_signature(secret, "100", "GET", "/v1/x", b"").unwrap();
        let diff_ts = prime_request_signature(secret, "101", "GET", "/v1/x", b"").unwrap();
        let diff_method = prime_request_signature(secret, "100", "POST", "/v1/x", b"").unwrap();
        let diff_path = prime_request_signature(secret, "100", "GET", "/v1/y", b"").unwrap();
        let diff_body =
            prime_request_signature(secret, "100", "GET", "/v1/x", b"{}").unwrap();
        assert_ne!(base, diff_ts);
        assert_ne!(base, diff_method);
        assert_ne!(base, diff_path);
        assert_ne!(base, diff_body);
    }

    #[test]
    fn prime_signature_rejects_invalid_base64_secret() {
        let err =
            prime_request_signature("@@not base64@@", "100", "GET", "/v1/x", b"").unwrap_err();
        assert!(matches!(err, AppError::Crypto(_)));
    }
}

/// Compute the Coinbase Prime request signature.
///
/// Prehash: `{timestamp}{METHOD}{path_with_query}{body}` (no separators).
/// HMAC-SHA256 keyed with the base64-decoded `api_secret`. Returns the
/// base64-encoded MAC (Coinbase's wire format on the
/// `X-CB-ACCESS-SIGN` header).
///
/// Exposed at module scope so the tests can verify exact-property
/// invariants without round-tripping through a live HTTP request.
pub(crate) fn prime_request_signature(
    api_secret_b64: &str,
    timestamp: &str,
    method: &str,
    path_with_query: &str,
    body: &[u8],
) -> AppResult<String> {
    use base64::Engine as _;
    let secret_bytes = base64::engine::general_purpose::STANDARD
        .decode(api_secret_b64.as_bytes())
        .map_err(|e| AppError::Crypto(format!("coinbase api_secret b64: {e}")))?;
    let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(&secret_bytes)
        .map_err(|e| AppError::Crypto(format!("hmac init: {e}")))?;
    let prehash = format!("{timestamp}{method}{path_with_query}");
    Mac::update(&mut mac, prehash.as_bytes());
    Mac::update(&mut mac, body);
    Ok(base64::engine::general_purpose::STANDARD.encode(Mac::finalize(mac).into_bytes()))
}
