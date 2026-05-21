//! Bridge.xyz — stablecoin (USDC) orchestration.
//!
//! Bridge.xyz (acquired by Stripe in October 2024) provides USDC
//! issuance, redemption, virtual accounts, and cross-border stablecoin
//! payouts under a regulated MTL framework. Same VASP-license leverage
//! story as Coinflow, but specifically for stablecoin rails.
//!
//! Auth: `Api-Key: <api_key>` header.
//! Base URL: production `https://api.bridge.xyz/v0`, sandbox
//! `https://api.sandbox.bridge.xyz/v0`.
//!
//! Endpoints used:
//!   * `GET /transfers?limit=N&starting_after=<id>` — paginated transfer
//!     list (newest-to-oldest by default; we reverse to post in order)
//!
//! Webhook signature:
//!   `X-Webhook-Signature: t=<ts_ms>,v0=<base64_rsa_sha256>`
//!   Bridge uses RSA-SHA256 PKCS1v15 with a per-account PEM public key
//!   delivered out-of-band. We do timestamp staleness checks (>10 min →
//!   reject) and structure the verifier, but the cryptographic signature
//!   step is deliberately left for a follow-up that adds the `rsa` crate
//!   — same posture as Plaid. Until then, signature_ok is recorded as
//!   `false` and the require_webhook_signatures flag in config controls
//!   whether unverified deliveries are rejected.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};
use crate::ledger::{AccountKind, Direction, DraftPosting, DraftTransaction};
use crate::money::Currency;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BridgeCredential {
    pub api_key: String,
    pub webhook_secret: Option<String>,
    /// PEM-encoded RSA public key Bridge delivers to the merchant
    /// out-of-band, used for webhook signature verification. Optional
    /// today — when present, future cryptographic verification will
    /// pick it up automatically.
    pub webhook_public_key_pem: Option<String>,
    /// "production" | "sandbox"
    #[serde(default = "default_env")]
    pub environment: String,
}
fn default_env() -> String { "production".into() }

impl BridgeCredential {
    pub fn base_url(&self) -> &'static str {
        if self.environment.eq_ignore_ascii_case("sandbox") {
            "https://api.sandbox.bridge.xyz/v0"
        } else {
            "https://api.bridge.xyz/v0"
        }
    }
}

// --- Wire types -----------------------------------------------------------

#[derive(Clone, Debug, Deserialize)]
pub struct BridgeTransfer {
    pub id: String,
    pub state: Option<String>,
    pub amount: String,
    pub currency: String,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub customer_id: Option<String>,
    pub on_behalf_of: Option<String>,
    pub source: Option<serde_json::Value>,
    pub destination: Option<serde_json::Value>,
    #[serde(flatten)]
    pub raw: serde_json::Value,
}

#[derive(Deserialize)]
struct TransfersResponse {
    data: Vec<BridgeTransfer>,
    #[serde(default)]
    count: Option<i64>,
}

// --- API client -----------------------------------------------------------

pub struct BridgeApi {
    cred: BridgeCredential,
    http: reqwest::Client,
}

impl BridgeApi {
    pub fn new(cred: BridgeCredential) -> Self {
        Self {
            cred,
            http: reqwest::Client::new(),
        }
    }

    /// `GET /transfers?limit=N&starting_after=<id>` — Bridge returns
    /// newest-to-oldest by default. We pass back `next_cursor = last
    /// transfer id` so the next page starts after it.
    pub async fn list_transfers(
        &self,
        limit: u32,
        starting_after: Option<&str>,
    ) -> AppResult<(Vec<BridgeTransfer>, Option<String>)> {
        let mut params: Vec<(&str, String)> = vec![
            ("limit", limit.to_string()),
        ];
        if let Some(c) = starting_after {
            params.push(("starting_after", c.to_string()));
        }
        let qs = serde_urlencoded::to_string(&params).map_err(|e| AppError::Provider {
            provider: "bridge".into(),
            message: format!("encode query: {e}"),
        })?;
        let url = format!("{}/transfers?{qs}", self.cred.base_url());

        let resp = self
            .http
            .get(&url)
            .header("Api-Key", &self.cred.api_key)
            .send()
            .await
            .map_err(|e| AppError::Provider {
                provider: "bridge".into(),
                message: format!("transfers HTTP: {e}"),
            })?;
        let status = resp.status();
        let bytes = resp.bytes().await.map_err(|e| AppError::Provider {
            provider: "bridge".into(),
            message: format!("transfers body: {e}"),
        })?;
        if !status.is_success() {
            return Err(AppError::Provider {
                provider: "bridge".into(),
                message: format!(
                    "transfers {status}: {}",
                    String::from_utf8_lossy(&bytes)
                ),
            });
        }
        let parsed: TransfersResponse =
            serde_json::from_slice(&bytes).map_err(|e| AppError::Provider {
                provider: "bridge".into(),
                message: format!("transfers decode: {e}"),
            })?;

        let next = parsed.data.last().map(|t| t.id.clone());
        Ok((parsed.data, next))
    }
}

// --- Webhook signature header parsing -------------------------------------

/// Result of parsing the `X-Webhook-Signature: t=<ts_ms>,v0=<sig>` header.
pub struct BridgeSignatureHeader {
    pub timestamp_ms: i64,
    pub signature_b64: String,
}

pub fn parse_signature_header(header: &str) -> AppResult<BridgeSignatureHeader> {
    let mut ts: Option<i64> = None;
    let mut sig: Option<String> = None;
    for part in header.split(',') {
        let p = part.trim();
        if let Some(rest) = p.strip_prefix("t=") {
            ts = rest.parse().ok();
        } else if let Some(rest) = p.strip_prefix("v0=") {
            sig = Some(rest.to_string());
        }
    }
    match (ts, sig) {
        (Some(timestamp_ms), Some(signature_b64)) => Ok(BridgeSignatureHeader {
            timestamp_ms,
            signature_b64,
        }),
        _ => Err(AppError::BadRequest(
            "bridge signature header must contain both t= and v0=".into(),
        )),
    }
}

/// Verify the staleness check Bridge requires (timestamp not older than
/// 10 minutes). The RSA-SHA256 PKCS1v15 cryptographic step requires a
/// vetted asymmetric-crypto dep (next push) — until then we return
/// `Ok(false)` for "structure was valid but signature not yet checked",
/// matching the Plaid pattern. The caller passes `Ok(false)` through to
/// `signature_ok=false` and `verification_error="signature_not_yet_verified"`.
pub fn validate_timestamp_freshness(
    parsed: &BridgeSignatureHeader,
    max_age_seconds: i64,
    now: DateTime<Utc>,
) -> AppResult<()> {
    let now_ms = now.timestamp_millis();
    let max_age_ms = max_age_seconds.saturating_mul(1000);
    if (now_ms - parsed.timestamp_ms).abs() > max_age_ms {
        return Err(AppError::Unauthorized);
    }
    Ok(())
}

// --- Normalization --------------------------------------------------------

#[derive(Clone, Debug)]
pub struct EnsureAccount {
    pub code: String,
    pub kind: AccountKind,
    pub currency: Currency,
}

pub struct NormalizedBridgeTransfer {
    pub draft: DraftTransaction,
    pub accounts_to_ensure: Vec<EnsureAccount>,
    pub recognized: bool,
}

const ACCT_BRIDGE_CLEARING: &str = "clearing/bridge";
const ACCT_BRIDGE_USDC: &str = "asset/bridge/usdc";
const ACCT_BRIDGE_FEES: &str = "expense/fees/bridge";

/// Normalize a Bridge transfer into a balanced draft. We only post for
/// terminal states (`payment_processed`, `payment_submitted`, `funds_received`,
/// `completed`); in-flight states stay observability-only so we don't
/// double-post when the transfer settles later.
///
/// Stablecoin amounts come back as decimal strings — we convert to minor
/// units (USDC is 1:1 with USD for ledger purposes; tenants who want
/// the raw on-chain amount in 6-decimal precision get it via the
/// metadata stash).
pub fn normalize_transfer(
    tr: &BridgeTransfer,
    tenant_id: uuid::Uuid,
) -> AppResult<NormalizedBridgeTransfer> {
    let terminal_states = ["payment_processed", "funds_received", "completed"];
    let is_terminal = tr
        .state
        .as_deref()
        .map(|s| terminal_states.contains(&s))
        .unwrap_or(false);

    let mut postings: Vec<DraftPosting> = Vec::new();
    let mut accounts: Vec<EnsureAccount> = Vec::new();
    let mut recognized = false;

    let posted_at: DateTime<Utc> = tr
        .updated_at
        .as_deref()
        .or(tr.created_at.as_deref())
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(Utc::now);

    let cur_upper = tr.currency.to_uppercase();
    let ledger_currency = if cur_upper == "USDC" || cur_upper == "USDT" {
        "USD".to_string()
    } else {
        cur_upper.clone()
    };
    let currency = Currency::new(&ledger_currency).map_err(|e| AppError::Provider {
        provider: "bridge".into(),
        message: format!("unknown currency {ledger_currency}: {e}"),
    })?;
    let cur = currency.as_str().to_string();
    let amount_minor = parse_amount_to_minor(&tr.amount)?;

    let meta = serde_json::json!({
        "bridge_transfer_id": tr.id,
        "bridge_state": tr.state,
        "bridge_currency_raw": tr.currency,
        "bridge_amount_raw": tr.amount,
        "bridge_customer_id": tr.customer_id,
        "bridge_on_behalf_of": tr.on_behalf_of,
        "bridge_source": tr.source,
        "bridge_destination": tr.destination,
    });

    if is_terminal && amount_minor > 0 {
        // Posting template: tenant USDC balance increases (asset) and
        // clearing decreases. For an *outgoing* transfer the direction
        // is inverted. We pick direction from `source`/`destination`
        // structure: if `destination.payment_rail == "bridge_internal"`
        // we treat as inbound (mint); otherwise outbound.
        let outbound = tr
            .destination
            .as_ref()
            .and_then(|v| v.get("payment_rail"))
            .and_then(|v| v.as_str())
            .map(|s| !s.eq_ignore_ascii_case("bridge_internal"))
            .unwrap_or(false);

        let (usdc_dir, clearing_dir) = if outbound {
            (Direction::Credit, Direction::Debit)
        } else {
            (Direction::Debit, Direction::Credit)
        };

        postings.push(DraftPosting {
            account_code: ACCT_BRIDGE_USDC.into(),
            direction: usdc_dir,
            amount_minor,
            currency: cur.clone(),
            source: "bridge".into(),
            source_event_id: tr.id.clone(),
            metadata: meta.clone(),
        });
        accounts.push(EnsureAccount {
            code: ACCT_BRIDGE_USDC.into(),
            kind: AccountKind::Asset,
            currency: currency.clone(),
        });

        postings.push(DraftPosting {
            account_code: ACCT_BRIDGE_CLEARING.into(),
            direction: clearing_dir,
            amount_minor,
            currency: cur.clone(),
            source: "bridge".into(),
            source_event_id: format!("{}:cp", tr.id),
            metadata: meta.clone(),
        });
        accounts.push(EnsureAccount {
            code: ACCT_BRIDGE_CLEARING.into(),
            kind: AccountKind::Asset,
            currency,
        });
        recognized = true;
    }

    // Silence the warning if neither branch fires.
    let _ = ACCT_BRIDGE_FEES;

    let description = Some(format!(
        "bridge transfer {} ({} {}) at {}",
        tr.id,
        tr.amount,
        tr.currency,
        posted_at.format("%Y-%m-%dT%H:%M:%SZ")
    ));

    Ok(NormalizedBridgeTransfer {
        draft: DraftTransaction {
            tenant_id,
            kind: format!("bridge.transfer.{}", tr.state.as_deref().unwrap_or("unknown")),
            idempotency_key: format!("bridge:tr:{}", tr.id),
            description,
            metadata: meta,
            postings,
        },
        accounts_to_ensure: accounts,
        recognized,
    })
}

/// Bridge sends amounts as decimal strings like "100.00" or "100.123456".
/// We convert to minor units (cents). For tokens with >2-decimal precision
/// we truncate to 2 — full precision is preserved in metadata.
fn parse_amount_to_minor(amount: &str) -> AppResult<i128> {
    let cleaned = amount.trim().replace([',', '_'], "");
    let parts: Vec<&str> = cleaned.split('.').collect();
    match parts.as_slice() {
        [w] => {
            let whole: i128 = w.parse().map_err(|e| AppError::Provider {
                provider: "bridge".into(),
                message: format!("amount {w}: {e}"),
            })?;
            Ok(whole * 100)
        }
        [w, f] => {
            let whole: i128 = w.parse().map_err(|e| AppError::Provider {
                provider: "bridge".into(),
                message: format!("amount whole {w}: {e}"),
            })?;
            let frac_str = if f.len() >= 2 {
                f[..2].to_string()
            } else {
                format!("{f}{}", "0".repeat(2 - f.len()))
            };
            let frac: i128 = frac_str.parse().map_err(|e| AppError::Provider {
                provider: "bridge".into(),
                message: format!("amount frac {frac_str}: {e}"),
            })?;
            Ok(whole * 100 + frac)
        }
        _ => Err(AppError::Provider {
            provider: "bridge".into(),
            message: format!("malformed amount {amount}"),
        }),
    }
}
