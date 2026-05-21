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
}
