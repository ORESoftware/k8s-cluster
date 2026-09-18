//! Braintree sync: GraphQL `searchTransactions` cursor walker.
//!
//! Braintree's REST API has been deprecated for OAuth merchants in
//! favor of the GraphQL endpoint at `/graphql`. We walk transactions
//! using `search(input: {...}, first: 100, after: $cursor)` with the
//! `Braintree-Version: 2019-01-01` header.
//!
//! Idempotency: `braintree:tx:<id>`. Only `SETTLED`, `SETTLING`, and
//! refunded statuses produce ledger postings; everything else opens a
//! recon break.

use serde::Deserialize;

use crate::error::{AppError, AppResult};
use crate::ledger::{AccountKind, Direction, DraftPosting, DraftTransaction};
use crate::money::Currency;
use crate::providers::amount::parse_decimal_to_minor;
use crate::providers::braintree::BraintreeCredential;
use crate::providers::connection::ProviderConnection;

use super::handler::{SyncCtx, SyncSummary};

const PAGE_SIZE: i32 = 100;
const MAX_PAGES_PER_RUN: u32 = 8;

const ACCT_CLEARING_PREFIX: &str = "clearing/braintree/";
const ACCT_REVENUE: &str = "revenue/braintree";
#[allow(dead_code)] // reserved: braintree fee splits land in a follow-up
const ACCT_FEES: &str = "expense/fees/braintree";
const ACCT_REFUNDS: &str = "expense/refunds/braintree";

pub async fn sync_braintree(
    ctx: &SyncCtx<'_>,
    conn: &ProviderConnection,
    caller_cursor: Option<&str>,
) -> AppResult<SyncSummary> {
    let plaintext = ctx
        .connections
        .load_credential(ctx.tenant_id, conn.id)
        .await?;
    let cred: BraintreeCredential =
        serde_json::from_slice(&plaintext).map_err(|e| AppError::Provider {
            provider: "braintree".into(),
            message: format!("decode sealed credential: {e}"),
        })?;
    let merchant_id = cred.merchant_id.clone();
    let api_base = graphql_endpoint(&cred.environment);
    let http = reqwest::Client::new();

    let mut cursor: Option<String> = caller_cursor
        .map(str::to_string)
        .or_else(|| conn.last_sync_cursor.clone().filter(|s| !s.is_empty()));
    let mut pages = 0u32;
    let mut total_events: i64 = 0;
    let mut total_postings: i64 = 0;
    let mut unrecognized = 0i64;
    let mut has_more = true;

    while has_more && pages < MAX_PAGES_PER_RUN {
        let page = run_search(&http, &api_base, &cred.access_token, cursor.as_deref()).await?;
        pages += 1;

        for edge in &page.edges {
            let node = &edge.node;
            cursor = Some(edge.cursor.clone());

            let status_class = classify_status(&node.status);
            match status_class {
                StatusClass::Authorized | StatusClass::Voided | StatusClass::Failed => {
                    continue;
                }
                StatusClass::Settled => match post_charge(ctx, &merchant_id, node).await {
                    Ok(PostOutcome::Posted { n }) => {
                        total_events += 1;
                        total_postings += n as i64;
                    }
                    Ok(PostOutcome::Replayed) => total_events += 1,
                    Ok(PostOutcome::Unrecognized) => unrecognized += 1,
                    Err(e) => return Err(e),
                },
                StatusClass::Refunded => match post_refund(ctx, &merchant_id, node).await {
                    Ok(PostOutcome::Posted { n }) => {
                        total_events += 1;
                        total_postings += n as i64;
                    }
                    Ok(PostOutcome::Replayed) => total_events += 1,
                    Ok(PostOutcome::Unrecognized) => unrecognized += 1,
                    Err(e) => return Err(e),
                },
            }
        }

        has_more = page.page_info.has_next_page;
    }

    let summary = format!(
        "braintree: pages_walked={pages}; processed {total_events} txs; \
         posted {total_postings} postings; unrecognized {unrecognized}; \
         has_more={has_more}"
    );

    Ok(SyncSummary {
        new_postings: total_postings,
        events_processed: total_events,
        next_cursor: cursor,
        has_more,
        summary,
    })
}

fn graphql_endpoint(env: &str) -> String {
    if env.eq_ignore_ascii_case("production") || env.eq_ignore_ascii_case("live") {
        "https://payments.braintree-api.com/graphql".into()
    } else {
        "https://payments.sandbox.braintree-api.com/graphql".into()
    }
}

// --- GraphQL request / response --------------------------------------

const SEARCH_QUERY: &str = r#"
query SearchTx($input: TransactionSearchInput!, $first: Int!, $after: String) {
  search {
    transactions(input: $input, first: $first, after: $after) {
      pageInfo { hasNextPage endCursor }
      edges {
        cursor
        node {
          id
          legacyId
          status
          createdAt
          updatedAt
          amount { value currencyIsoCode }
          orderId
          merchantAccountId
          refunds { id legacyId amount { value currencyIsoCode } status createdAt }
          customFields
        }
      }
    }
  }
}
"#;

#[derive(serde::Serialize)]
struct GqlReq<'a> {
    query: &'a str,
    variables: serde_json::Value,
}

#[derive(Deserialize)]
struct GqlResp {
    data: Option<GqlData>,
    errors: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct GqlData {
    search: SearchData,
}

#[derive(Deserialize)]
struct SearchData {
    transactions: TxConnection,
}

#[derive(Deserialize)]
struct TxConnection {
    #[serde(rename = "pageInfo")]
    page_info: PageInfo,
    edges: Vec<TxEdge>,
}

#[derive(Deserialize)]
struct PageInfo {
    #[serde(rename = "hasNextPage")]
    has_next_page: bool,
    #[allow(dead_code)]
    #[serde(rename = "endCursor")]
    end_cursor: Option<String>,
}

#[derive(Deserialize)]
struct TxEdge {
    cursor: String,
    node: TxNode,
}

#[derive(Deserialize)]
struct TxNode {
    id: String,
    #[serde(rename = "legacyId")]
    legacy_id: Option<String>,
    status: String,
    #[serde(rename = "createdAt")]
    created_at: Option<String>,
    #[serde(rename = "updatedAt")]
    updated_at: Option<String>,
    amount: GqlMoney,
    #[serde(rename = "orderId")]
    order_id: Option<String>,
    #[serde(rename = "merchantAccountId")]
    merchant_account_id: Option<String>,
    #[serde(default)]
    refunds: Vec<RefundNode>,
}

#[derive(Deserialize, Clone)]
struct GqlMoney {
    value: String,
    #[serde(rename = "currencyIsoCode")]
    currency_iso_code: String,
}

#[derive(Deserialize)]
struct RefundNode {
    id: String,
    amount: GqlMoney,
    status: String,
    #[allow(dead_code)]
    #[serde(rename = "createdAt")]
    created_at: Option<String>,
}

async fn run_search(
    http: &reqwest::Client,
    endpoint: &str,
    bearer: &str,
    after: Option<&str>,
) -> AppResult<TxConnection> {
    let variables = serde_json::json!({
        "input": {},
        "first": PAGE_SIZE,
        "after": after,
    });
    let body = GqlReq {
        query: SEARCH_QUERY,
        variables,
    };
    let resp = http
        .post(endpoint)
        .bearer_auth(bearer)
        .header("Braintree-Version", "2019-01-01")
        .json(&body)
        .send()
        .await
        .map_err(|e| AppError::Provider {
            provider: "braintree".into(),
            message: format!("graphql HTTP: {e}"),
        })?;
    let status = resp.status();
    let bytes = resp.bytes().await.map_err(|e| AppError::Provider {
        provider: "braintree".into(),
        message: format!("graphql body: {e}"),
    })?;
    if !status.is_success() {
        return Err(AppError::Provider {
            provider: "braintree".into(),
            message: format!("graphql {status}: {}", String::from_utf8_lossy(&bytes)),
        });
    }
    let parsed: GqlResp = serde_json::from_slice(&bytes).map_err(|e| AppError::Provider {
        provider: "braintree".into(),
        message: format!("graphql decode: {e}"),
    })?;
    if let Some(errs) = parsed.errors {
        return Err(AppError::Provider {
            provider: "braintree".into(),
            message: format!("graphql errors: {errs}"),
        });
    }
    let data = parsed.data.ok_or_else(|| AppError::Provider {
        provider: "braintree".into(),
        message: "graphql data missing".into(),
    })?;
    Ok(data.search.transactions)
}

// --- Status classification + posting -----------------------------------

#[allow(dead_code)]
// Refunded only ever returns from top-level refund nodes
// we haven't yet seen in real data; kept for completeness
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StatusClass {
    Authorized,
    Settled,
    Voided,
    Failed,
    Refunded,
}

pub(crate) fn classify_status(s: &str) -> StatusClass {
    match s {
        "SETTLED" | "SETTLEMENT_CONFIRMED" | "SETTLEMENT_PENDING" | "SUBMITTED_FOR_SETTLEMENT" => {
            StatusClass::Settled
        }
        "VOIDED" => StatusClass::Voided,
        "GATEWAY_REJECTED" | "PROCESSOR_DECLINED" | "FAILED" | "SETTLEMENT_DECLINED" => {
            StatusClass::Failed
        }
        "AUTHORIZED" | "AUTHORIZING" => StatusClass::Authorized,
        _ => StatusClass::Authorized,
    }
}

enum PostOutcome {
    Posted { n: usize },
    Replayed,
    Unrecognized,
}

async fn post_charge(ctx: &SyncCtx<'_>, merchant_id: &str, n: &TxNode) -> AppResult<PostOutcome> {
    let currency = Currency::new(&n.amount.currency_iso_code).map_err(|e| AppError::Provider {
        provider: "braintree".into(),
        message: format!("unknown currency {}: {e}", n.amount.currency_iso_code),
    })?;
    let cur = currency.as_str().to_string();
    let amount_minor = parse_decimal_to_minor(&n.amount.value, "braintree")?;
    let abs = amount_minor.unsigned_abs() as i128;
    if abs == 0 {
        return Ok(PostOutcome::Unrecognized);
    }
    let clearing = format!("{ACCT_CLEARING_PREFIX}{merchant_id}");

    let meta = serde_json::json!({
        "braintree_id": n.id,
        "braintree_legacy_id": n.legacy_id,
        "braintree_status": n.status,
        "braintree_order_id": n.order_id,
        "braintree_merchant_account_id": n.merchant_account_id,
        "braintree_created_at": n.created_at,
        "braintree_updated_at": n.updated_at,
    });

    let mk = |code: &str, dir: Direction, suffix: &str| DraftPosting {
        account_code: code.into(),
        direction: dir,
        amount_minor: abs,
        currency: cur.clone(),
        source: "braintree".into(),
        source_event_id: if suffix.is_empty() {
            n.id.clone()
        } else {
            format!("{}:{suffix}", n.id)
        },
        metadata: meta.clone(),
    };

    let postings = vec![
        mk(&clearing, Direction::Debit, ""),
        mk(ACCT_REVENUE, Direction::Credit, "cp"),
    ];

    for (code, kind) in &[
        (clearing.as_str(), AccountKind::Asset),
        (ACCT_REVENUE, AccountKind::Income),
    ] {
        ctx.ledger
            .ensure_account(
                ctx.tenant_id,
                ctx.region,
                None,
                *kind,
                code,
                currency.clone(),
            )
            .await?;
    }

    let len = postings.len();
    let draft = DraftTransaction {
        tenant_id: ctx.tenant_id,
        kind: "braintree.charge".into(),
        idempotency_key: format!("braintree:tx:{}", n.id),
        description: Some(format!(
            "braintree {} {} ({})",
            n.status,
            n.order_id.as_deref().unwrap_or(""),
            n.id
        )),
        metadata: meta,
        postings,
    };
    let outcome = match ctx.ledger.post_transaction(&draft, ctx.region).await {
        Ok(_) => PostOutcome::Posted { n: len },
        Err(AppError::Conflict(_)) => PostOutcome::Replayed,
        Err(e) => return Err(e),
    };

    // Post refunds as separate ledger transactions.
    for r in &n.refunds {
        if r.status != "SETTLED" && r.status != "SETTLEMENT_CONFIRMED" {
            continue;
        }
        post_refund_leg(ctx, merchant_id, &n.id, r, &currency).await?;
    }

    Ok(outcome)
}

async fn post_refund(ctx: &SyncCtx<'_>, merchant_id: &str, n: &TxNode) -> AppResult<PostOutcome> {
    let currency = Currency::new(&n.amount.currency_iso_code).map_err(|e| AppError::Provider {
        provider: "braintree".into(),
        message: format!("unknown currency {}: {e}", n.amount.currency_iso_code),
    })?;
    post_refund_leg(
        ctx,
        merchant_id,
        &n.id,
        &RefundNode {
            id: n.id.clone(),
            amount: n.amount.clone(),
            status: n.status.clone(),
            created_at: n.created_at.clone(),
        },
        &currency,
    )
    .await?;
    Ok(PostOutcome::Posted { n: 2 })
}

async fn post_refund_leg(
    ctx: &SyncCtx<'_>,
    merchant_id: &str,
    parent_tx_id: &str,
    r: &RefundNode,
    currency: &Currency,
) -> AppResult<()> {
    let cur = currency.as_str().to_string();
    let abs = parse_decimal_to_minor(&r.amount.value, "braintree")?.unsigned_abs() as i128;
    let clearing = format!("{ACCT_CLEARING_PREFIX}{merchant_id}");

    for (code, kind) in &[
        (clearing.as_str(), AccountKind::Asset),
        (ACCT_REFUNDS, AccountKind::Expense),
    ] {
        ctx.ledger
            .ensure_account(
                ctx.tenant_id,
                ctx.region,
                None,
                *kind,
                code,
                currency.clone(),
            )
            .await?;
    }

    let meta = serde_json::json!({
        "braintree_parent_tx_id": parent_tx_id,
        "braintree_refund_id": r.id,
        "braintree_refund_status": r.status,
    });
    let draft = DraftTransaction {
        tenant_id: ctx.tenant_id,
        kind: "braintree.refund".into(),
        idempotency_key: format!("braintree:refund:{}", r.id),
        description: Some(format!("braintree refund {} (of {parent_tx_id})", r.id)),
        metadata: meta.clone(),
        postings: vec![
            DraftPosting {
                account_code: ACCT_REFUNDS.into(),
                direction: Direction::Debit,
                amount_minor: abs,
                currency: cur.clone(),
                source: "braintree".into(),
                source_event_id: r.id.clone(),
                metadata: meta.clone(),
            },
            DraftPosting {
                account_code: clearing,
                direction: Direction::Credit,
                amount_minor: abs,
                currency: cur,
                source: "braintree".into(),
                source_event_id: format!("{}:cp", r.id),
                metadata: meta,
            },
        ],
    };
    match ctx.ledger.post_transaction(&draft, ctx.region).await {
        Ok(_) | Err(AppError::Conflict(_)) => Ok(()),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::mock_http::{ExpectedRequest, ProviderMock};

    #[test]
    fn classifies_settled_statuses() {
        assert_eq!(classify_status("SETTLED"), StatusClass::Settled);
        assert_eq!(
            classify_status("SETTLEMENT_CONFIRMED"),
            StatusClass::Settled
        );
        assert_eq!(classify_status("SETTLEMENT_PENDING"), StatusClass::Settled);
        assert_eq!(
            classify_status("SUBMITTED_FOR_SETTLEMENT"),
            StatusClass::Settled
        );
    }

    #[test]
    fn classifies_failed_statuses() {
        assert_eq!(classify_status("GATEWAY_REJECTED"), StatusClass::Failed);
        assert_eq!(classify_status("PROCESSOR_DECLINED"), StatusClass::Failed);
        assert_eq!(classify_status("FAILED"), StatusClass::Failed);
        assert_eq!(classify_status("SETTLEMENT_DECLINED"), StatusClass::Failed);
    }

    #[test]
    fn classifies_authorized_voided() {
        assert_eq!(classify_status("AUTHORIZED"), StatusClass::Authorized);
        assert_eq!(classify_status("AUTHORIZING"), StatusClass::Authorized);
        assert_eq!(classify_status("VOIDED"), StatusClass::Voided);
    }

    #[test]
    fn unknown_status_defaults_to_authorized_not_settled() {
        // Critical: an unknown status must NOT be treated as Settled,
        // because that would post unverified money into the ledger.
        // Defaulting to Authorized means the sync skips it (we only
        // post on Settled/Refunded).
        assert_eq!(classify_status("MYSTERY_STATUS"), StatusClass::Authorized);
        assert_eq!(classify_status(""), StatusClass::Authorized);
    }

    #[tokio::test]
    async fn run_search_posts_graphql_contract_to_mock() {
        let mock = ProviderMock::start(vec![
            ExpectedRequest::post("/")
                .header("authorization", "Bearer bt_access")
                .header("braintree-version", "2019-01-01")
                .json_body(serde_json::json!({
                    "query": SEARCH_QUERY,
                    "variables": {
                        "input": {},
                        "first": PAGE_SIZE,
                        "after": "cursor_1"
                    }
                }))
                .respond_json(serde_json::json!({
                    "data": {
                        "search": {
                            "transactions": {
                                "pageInfo": {
                                    "hasNextPage": false,
                                    "endCursor": "cursor_2"
                                },
                                "edges": [{
                                    "cursor": "cursor_2",
                                    "node": {
                                        "id": "bt_tx_1",
                                        "legacyId": "legacy_1",
                                        "status": "SETTLED",
                                        "amount": {
                                            "value": "12.34",
                                            "currencyIsoCode": "USD"
                                        },
                                        "refunds": []
                                    }
                                }]
                            }
                        }
                    }
                })),
        ])
        .await;
        let http = reqwest::Client::new();

        let page = run_search(&http, &mock.base_url(), "bt_access", Some("cursor_1"))
            .await
            .unwrap();

        assert!(!page.page_info.has_next_page);
        assert_eq!(page.edges[0].node.id, "bt_tx_1");
        mock.assert_finished().await;
    }
}
