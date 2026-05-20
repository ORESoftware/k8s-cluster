//! Stripe Connect (OAuth Standard) — observer-mode integration.
//!
//! Connection model:
//!   * Tenant clicks "Connect Stripe" in their dashboard.
//!   * We redirect to `https://connect.stripe.com/oauth/authorize?...`
//!     with `client_id`, `scope=read_write`, `redirect_uri`, `state`.
//!   * Stripe redirects back to `/v1/oauth/stripe/callback?code=…&state=…`.
//!   * We POST `code` to `https://connect.stripe.com/oauth/token`, receive
//!     `stripe_user_id`, `access_token`, `refresh_token`.
//!   * We seal `{access_token, refresh_token, stripe_user_id}` and store.
//!
//! At runtime, we authenticate API calls with the **platform** secret key
//! (`STRIPE_CLIENT_SECRET`) and scope them to the connected account using
//! `Stripe-Account: <stripe_user_id>` header. This is the recommended
//! Stripe Connect pattern and avoids the access-token refresh dance.

use chrono::{DateTime, TimeZone, Utc};
use hmac::{Hmac, KeyInit, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use crate::config::Config;
use crate::error::{AppError, AppResult};
use crate::ledger::{Direction, DraftPosting, DraftTransaction};
use crate::money::Currency;

use super::oauth_common::CodeExchangeResult;

// --- Credential ------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StripeCredential {
    pub stripe_user_id: String,
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub livemode: bool,
    pub scope: Option<String>,
}

// --- OAuth -----------------------------------------------------------------

pub struct StripeOAuth<'a> {
    cfg: &'a Config,
}

#[derive(Debug, Deserialize)]
struct StripeTokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    stripe_user_id: String,
    scope: Option<String>,
    #[serde(default)]
    livemode: bool,
}

#[derive(Debug, Deserialize)]
struct StripeOAuthError {
    error: String,
    error_description: Option<String>,
}

impl<'a> StripeOAuth<'a> {
    pub fn new(cfg: &'a Config) -> Self { Self { cfg } }

    pub fn authorize_url(&self, state: &str) -> AppResult<String> {
        let client_id = self.cfg.stripe_client_id.as_ref()
            .ok_or_else(|| AppError::BadRequest("STRIPE_CLIENT_ID not configured".into()))?;
        let redirect = format!("{}/v1/oauth/stripe/callback", self.cfg.oauth_redirect_base);
        Ok(format!(
            "https://connect.stripe.com/oauth/authorize\
             ?response_type=code&client_id={client_id}\
             &scope=read_write\
             &redirect_uri={redirect}\
             &state={state}"
        ))
    }

    /// Exchange the auth code for tokens via `POST /oauth/token`.
    ///
    /// Stripe expects form-urlencoded body. Returns 200 on success, 400 on
    /// rejected codes with a JSON `{error, error_description}` body — we
    /// surface those as `Provider` errors.
    pub async fn exchange_code(&self, code: &str) -> AppResult<CodeExchangeResult> {
        let secret = self.cfg.stripe_client_secret.as_ref().ok_or_else(|| {
            AppError::BadRequest("STRIPE_CLIENT_SECRET not configured".into())
        })?;

        let body = serde_urlencoded::to_string(&[
            ("client_secret", secret.as_str()),
            ("code", code),
            ("grant_type", "authorization_code"),
        ])
        .map_err(|e| AppError::Provider {
            provider: "stripe".into(),
            message: format!("encode form: {e}"),
        })?;
        let resp = reqwest::Client::new()
            .post("https://connect.stripe.com/oauth/token")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(body)
            .send()
            .await
            .map_err(|e| AppError::Provider {
                provider: "stripe".into(),
                message: format!("oauth/token HTTP: {e}"),
            })?;

        let status = resp.status();
        let bytes = resp.bytes().await.map_err(|e| AppError::Provider {
            provider: "stripe".into(),
            message: format!("oauth/token body: {e}"),
        })?;

        if !status.is_success() {
            let body: StripeOAuthError =
                serde_json::from_slice(&bytes).unwrap_or(StripeOAuthError {
                    error: format!("http {status}"),
                    error_description: Some(String::from_utf8_lossy(&bytes).into_owned()),
                });
            return Err(AppError::Provider {
                provider: "stripe".into(),
                message: format!(
                    "{}: {}",
                    body.error,
                    body.error_description.unwrap_or_default()
                ),
            });
        }

        let token: StripeTokenResponse =
            serde_json::from_slice(&bytes).map_err(|e| AppError::Provider {
                provider: "stripe".into(),
                message: format!("oauth/token decode: {e}"),
            })?;

        let cred = StripeCredential {
            stripe_user_id: token.stripe_user_id.clone(),
            access_token: token.access_token,
            refresh_token: token.refresh_token,
            livemode: token.livemode,
            scope: token.scope.clone(),
        };
        let plaintext = serde_json::to_vec(&cred).map_err(|e| AppError::Provider {
            provider: "stripe".into(),
            message: format!("seal-encode: {e}"),
        })?;
        let scopes: Vec<String> = token
            .scope
            .as_deref()
            .map(|s| s.split(',').map(str::trim).map(str::to_string).collect())
            .unwrap_or_default();
        Ok(CodeExchangeResult {
            external_account_id: token.stripe_user_id.clone(),
            sealed_plaintext: plaintext,
            scopes,
            expires_at: None, // Stripe Connect Standard access tokens don't expire by themselves.
            display_label_suggestion: Some(format!("Stripe {}", token.stripe_user_id)),
        })
    }
}

// --- BalanceTransaction API client ----------------------------------------

#[derive(Debug, Deserialize)]
pub struct BalanceTransaction {
    pub id: String,
    pub amount: i64,
    pub fee: i64,
    pub net: i64,
    pub currency: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub status: String,
    pub created: i64,
    pub available_on: Option<i64>,
    pub source: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct StripeList<T> {
    data: Vec<T>,
    has_more: bool,
}

pub struct StripeApi {
    secret_key: String,
    stripe_account: String,
    http: reqwest::Client,
}

impl StripeApi {
    pub fn new(secret_key: String, stripe_account: String) -> Self {
        Self {
            secret_key,
            stripe_account,
            http: reqwest::Client::new(),
        }
    }

    /// List balance transactions older than `ending_before` (i.e. newer than
    /// the cursor) — Stripe orders newest-first, so this gets us only items
    /// we haven't seen. Returns up to `limit` per call; pagination handled
    /// by the caller.
    pub async fn list_balance_transactions(
        &self,
        ending_before: Option<&str>,
        limit: u32,
    ) -> AppResult<(Vec<BalanceTransaction>, bool)> {
        let mut params: Vec<(&str, String)> = vec![("limit", limit.to_string())];
        if let Some(c) = ending_before {
            params.push(("ending_before", c.to_string()));
        }
        let qs = serde_urlencoded::to_string(&params)
            .map_err(|e| AppError::Provider {
                provider: "stripe".into(),
                message: format!("encode query: {e}"),
            })?;
        let url = format!("https://api.stripe.com/v1/balance_transactions?{qs}");

        let resp = self
            .http
            .get(&url)
            .basic_auth(&self.secret_key, Some(""))
            .header("Stripe-Account", &self.stripe_account)
            .send()
            .await
            .map_err(|e| AppError::Provider {
                provider: "stripe".into(),
                message: format!("balance_transactions HTTP: {e}"),
            })?;

        let status = resp.status();
        let bytes = resp.bytes().await.map_err(|e| AppError::Provider {
            provider: "stripe".into(),
            message: format!("balance_transactions body: {e}"),
        })?;
        if !status.is_success() {
            return Err(AppError::Provider {
                provider: "stripe".into(),
                message: format!(
                    "balance_transactions returned {status}: {}",
                    String::from_utf8_lossy(&bytes)
                ),
            });
        }

        let list: StripeList<BalanceTransaction> =
            serde_json::from_slice(&bytes).map_err(|e| AppError::Provider {
                provider: "stripe".into(),
                message: format!("balance_transactions decode: {e}"),
            })?;

        Ok((list.data, list.has_more))
    }
}

// --- Webhook signature verification ---------------------------------------

#[derive(Clone, Debug, Deserialize)]
pub struct StripeWebhookEvent {
    pub id: String,
    #[serde(rename = "type")]
    pub event_type: String,
    pub data: serde_json::Value,
    pub livemode: bool,
    pub account: Option<String>,
}

/// Verify Stripe's `Stripe-Signature` header (v1 scheme).
/// Header looks like `t=1234,v1=hex_sig[,v1=...]`.
pub fn verify_signature(
    payload: &[u8],
    header: &str,
    signing_secret: &str,
) -> AppResult<()> {
    let mut timestamp: Option<&str> = None;
    let mut v1_sigs: Vec<&str> = Vec::new();
    for part in header.split(',') {
        let mut it = part.splitn(2, '=');
        match (it.next(), it.next()) {
            (Some("t"), Some(v))  => timestamp = Some(v),
            (Some("v1"), Some(v)) => v1_sigs.push(v),
            _ => {}
        }
    }
    let ts = timestamp.ok_or_else(|| AppError::BadRequest("missing t= in Stripe-Signature".into()))?;
    if v1_sigs.is_empty() {
        return Err(AppError::BadRequest("missing v1= signature".into()));
    }

    let signed = format!("{ts}.");
    let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(signing_secret.as_bytes())
        .map_err(|e| AppError::Crypto(format!("hmac init: {e}")))?;
    Mac::update(&mut mac, signed.as_bytes());
    Mac::update(&mut mac, payload);
    let expected = Mac::finalize(mac).into_bytes();
    let expected_hex = hex::encode(expected);

    for sig in v1_sigs {
        if constant_time_eq_str(sig, &expected_hex) {
            return Ok(());
        }
    }
    Err(AppError::Unauthorized)
}

fn constant_time_eq_str(a: &str, b: &str) -> bool {
    let ab = a.as_bytes();
    let bb = b.as_bytes();
    if ab.len() != bb.len() { return false; }
    let mut diff: u8 = 0;
    for (x, y) in ab.iter().zip(bb.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

// --- Normalization: BalanceTransaction -> ledger DraftTransaction ----------

/// What we emit for posting. The caller (sync handler) uses
/// `LedgerService::ensure_account` for each account_code first, then
/// `post_transaction(draft, region)`.
pub struct NormalizedTransaction {
    pub draft: DraftTransaction,
    /// Account codes the caller must ensure exist before posting.
    pub accounts_to_ensure: Vec<EnsureAccount>,
    /// True when we recognized the type and posted real entries. False when
    /// we don't have a posting template for this `type` — caller should
    /// open a reconciliation_break rather than silently dropping.
    pub recognized: bool,
}

#[derive(Clone, Debug)]
pub struct EnsureAccount {
    pub code: String,
    pub kind: crate::ledger::AccountKind,
    pub currency: Currency,
}

const ACCT_CLEARING_PREFIX: &str = "clearing/stripe/";
const ACCT_REVENUE: &str = "revenue/stripe";
const ACCT_FEES: &str = "expense/fees/stripe";
const ACCT_REFUNDS: &str = "expense/refunds/stripe";
const ACCT_BANK_PENDING: &str = "asset/bank/pending";

/// Normalize a Stripe BalanceTransaction into a balanced double-entry draft.
///
/// Posting templates by `type`:
///
///   * `charge` (amount > 0, fee >= 0):
///       DR clearing/stripe/<acct>  net
///       DR expense/fees/stripe     fee
///       CR revenue/stripe          amount       (= net + fee)
///
///   * `refund` (amount < 0, fee = 0 normally):
///       DR expense/refunds/stripe  |amount|
///       CR clearing/stripe/<acct>  |amount|
///
///   * `payout` (amount < 0, money moving from Stripe -> bank):
///       DR asset/bank/pending      |amount|
///       CR clearing/stripe/<acct>  |amount|
///
///   * `payout_failure` (amount > 0, money returns to clearing):
///       DR clearing/stripe/<acct>  amount
///       CR asset/bank/pending      amount
///
///   * `adjustment`, `dispute`, etc.: caller raises a reconciliation_break;
///     we return `recognized = false` so they don't silently disappear.
pub fn normalize_balance_transaction(
    tx: &BalanceTransaction,
    tenant_id: uuid::Uuid,
    stripe_account_id: &str,
) -> AppResult<NormalizedTransaction> {
    let currency = Currency::new(&tx.currency.to_uppercase())
        .map_err(|e| AppError::Provider {
            provider: "stripe".into(),
            message: format!("unknown currency {}: {e}", tx.currency),
        })?;
    let cur = currency.as_str().to_string();
    let clearing_code = format!("{ACCT_CLEARING_PREFIX}{stripe_account_id}");
    let posted_at: DateTime<Utc> = Utc
        .timestamp_opt(tx.created, 0)
        .single()
        .unwrap_or_else(Utc::now);

    let idempotency_key = format!("stripe:bt:{}", tx.id);
    let description = Some(format!(
        "stripe {} {} {} ({})",
        tx.kind,
        signed_amount_human(tx.amount, &cur),
        tx.id,
        posted_at.format("%Y-%m-%dT%H:%M:%SZ")
    ));

    let meta_common = serde_json::json!({
        "stripe_id": tx.id,
        "stripe_type": tx.kind,
        "stripe_status": tx.status,
        "stripe_account": stripe_account_id,
        "available_on": tx.available_on,
        "source": tx.source,
    });

    let mut postings: Vec<DraftPosting> = Vec::new();
    let mut accounts: Vec<EnsureAccount> = Vec::new();
    let mut recognized = true;

    let mk_posting = |account_code: &str, direction: Direction, amount: i128| DraftPosting {
        account_code: account_code.to_string(),
        direction,
        amount_minor: amount,
        currency: cur.clone(),
        source: "stripe".into(),
        source_event_id: tx.id.clone(),
        metadata: meta_common.clone(),
    };

    match tx.kind.as_str() {
        "charge" | "payment" => {
            // amount = gross, fee = stripe fee, net = amount - fee
            let amount = tx.amount.unsigned_abs() as i128;
            let fee = tx.fee.unsigned_abs() as i128;
            let net = amount - fee;
            postings.push(mk_posting(&clearing_code, Direction::Debit, net));
            if fee > 0 {
                postings.push(mk_posting(ACCT_FEES, Direction::Debit, fee));
            }
            postings.push(mk_posting(ACCT_REVENUE, Direction::Credit, amount));
            accounts.push(EnsureAccount {
                code: clearing_code.clone(),
                kind: crate::ledger::AccountKind::Asset,
                currency: currency.clone(),
            });
            if fee > 0 {
                accounts.push(EnsureAccount {
                    code: ACCT_FEES.into(),
                    kind: crate::ledger::AccountKind::Expense,
                    currency: currency.clone(),
                });
            }
            accounts.push(EnsureAccount {
                code: ACCT_REVENUE.into(),
                kind: crate::ledger::AccountKind::Income,
                currency: currency.clone(),
            });
        }
        "refund" | "payment_refund" => {
            let abs = tx.amount.unsigned_abs() as i128;
            postings.push(mk_posting(ACCT_REFUNDS, Direction::Debit, abs));
            postings.push(mk_posting(&clearing_code, Direction::Credit, abs));
            accounts.push(EnsureAccount {
                code: ACCT_REFUNDS.into(),
                kind: crate::ledger::AccountKind::Expense,
                currency: currency.clone(),
            });
            accounts.push(EnsureAccount {
                code: clearing_code.clone(),
                kind: crate::ledger::AccountKind::Asset,
                currency: currency.clone(),
            });
        }
        "payout" => {
            let abs = tx.amount.unsigned_abs() as i128;
            postings.push(mk_posting(ACCT_BANK_PENDING, Direction::Debit, abs));
            postings.push(mk_posting(&clearing_code, Direction::Credit, abs));
            accounts.push(EnsureAccount {
                code: ACCT_BANK_PENDING.into(),
                kind: crate::ledger::AccountKind::Asset,
                currency: currency.clone(),
            });
            accounts.push(EnsureAccount {
                code: clearing_code.clone(),
                kind: crate::ledger::AccountKind::Asset,
                currency: currency.clone(),
            });
        }
        "payout_failure" => {
            let abs = tx.amount.unsigned_abs() as i128;
            postings.push(mk_posting(&clearing_code, Direction::Debit, abs));
            postings.push(mk_posting(ACCT_BANK_PENDING, Direction::Credit, abs));
            accounts.push(EnsureAccount {
                code: clearing_code.clone(),
                kind: crate::ledger::AccountKind::Asset,
                currency: currency.clone(),
            });
            accounts.push(EnsureAccount {
                code: ACCT_BANK_PENDING.into(),
                kind: crate::ledger::AccountKind::Asset,
                currency: currency.clone(),
            });
        }
        _other => {
            // Recognized as "we saw it" but no posting template — caller
            // raises a reconciliation_break with the raw event attached.
            recognized = false;
        }
    }

    let draft = DraftTransaction {
        tenant_id,
        kind: format!("stripe.{}", tx.kind),
        idempotency_key,
        description,
        metadata: meta_common,
        postings,
    };

    Ok(NormalizedTransaction { draft, accounts_to_ensure: accounts, recognized })
}

fn signed_amount_human(amount: i64, currency: &str) -> String {
    let sign = if amount < 0 { "-" } else { "" };
    let abs = amount.unsigned_abs();
    let whole = abs / 100;
    let frac = abs % 100;
    format!("{sign}{whole}.{frac:02} {currency}")
}
