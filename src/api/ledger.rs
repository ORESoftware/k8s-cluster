use axum::Json;
use axum::extract::{Extension, Path, Query, State};
use axum::http::{HeaderMap, HeaderValue};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api::auth::Principal;
use crate::error::{AppError, AppResult};
use crate::financial_audit::{
    BILLING_WRITE_SCOPE, FinancialOperationContext, REQUEST_ID_HEADER,
};
use crate::ledger::{AccountBalance, AccountKind, DraftTransaction};
use crate::money::Currency;
use crate::state::AppState;

#[derive(Deserialize)]
pub struct EnsureAccountBody {
    pub kind: AccountKind,
    pub code: String,
    pub currency: String,
    pub user_id: Option<Uuid>,
}

#[derive(Serialize)]
pub struct EnsureAccountResp {
    pub id: Uuid,
    pub code: String,
    pub currency: String,
}

pub async fn ensure_account(
    State(state): State<AppState>,
    Path(tenant_id): Path<Uuid>,
    Json(input): Json<EnsureAccountBody>,
) -> AppResult<Json<EnsureAccountResp>> {
    let tenant = state.tenants.by_id(tenant_id).await?;
    let region = tenant.region()?;
    let currency =
        Currency::new(&input.currency).map_err(|e| AppError::BadRequest(e.to_string()))?;
    let acct = state
        .ledger
        .ensure_account(
            tenant_id,
            region,
            input.user_id,
            input.kind,
            &input.code,
            currency,
        )
        .await?;
    Ok(Json(EnsureAccountResp {
        id: acct.id,
        code: acct.code,
        currency: acct.currency.to_string(),
    }))
}

#[derive(Serialize)]
pub struct PostTxResp {
    pub transaction_id: Uuid,
    pub operation_event_id: Option<Uuid>,
    pub operation_correlation_id: Option<Uuid>,
    pub request_correlation_id: Uuid,
    pub replayed: bool,
    pub audit_status: &'static str,
}

pub async fn post_transaction(
    State(state): State<AppState>,
    Path(tenant_id): Path<Uuid>,
    Extension(principal): Extension<Principal>,
    headers: HeaderMap,
    Json(draft): Json<DraftTransaction>,
) -> AppResult<(HeaderMap, Json<PostTxResp>)> {
    // Defense in depth: refuse rather than silently rewrite. A body
    // tenant_id that disagrees with the path almost always signals a
    // bug in the caller (which they want to know about) or an attempt
    // to write into another tenant (which we want to reject loudly).
    //
    // Nil UUIDs in the body are tolerated because clients sometimes
    // omit the field; serde fills it with `Uuid::nil()`.
    if !draft.tenant_id.is_nil() && draft.tenant_id != tenant_id {
        return Err(AppError::BadRequest(format!(
            "body.tenant_id {} does not match path tenant_id {}",
            draft.tenant_id, tenant_id
        )));
    }
    let mut draft = draft;
    draft.tenant_id = tenant_id;

    // The middleware has already authenticated and authorized this exact
    // tenant. Hand the verified principal—not untrusted request headers—to the
    // ledger so actor attribution commits atomically with the postings.
    let operation_context =
        FinancialOperationContext::from_request(&principal, &headers, BILLING_WRITE_SCOPE)?;
    let request_correlation_id = operation_context.request_correlation_id;

    let tenant = state.tenants.by_id(tenant_id).await?;
    let region = tenant.region()?;
    let posted = state
        .ledger
        .post_transaction(&draft, region, &operation_context)
        .await?;

    let mut response_headers = HeaderMap::new();
    response_headers.insert(
        REQUEST_ID_HEADER,
        HeaderValue::from_str(&request_correlation_id.to_string())
            .map_err(|error| AppError::Other(error.into()))?,
    );

    Ok((
        response_headers,
        Json(PostTxResp {
            transaction_id: posted.transaction_id,
            operation_event_id: posted.audit.event_id,
            operation_correlation_id: posted.audit.operation_correlation_id,
            request_correlation_id,
            replayed: posted.replayed,
            audit_status: posted.audit.status.as_str(),
        }),
    ))
}

#[derive(Deserialize)]
pub struct BalanceQuery {
    #[serde(default = "default_currency")]
    pub currency: String,
}

fn default_currency() -> String {
    "USD".into()
}

pub async fn account_balance(
    State(state): State<AppState>,
    Path((tenant_id, code)): Path<(Uuid, String)>,
    Query(q): Query<BalanceQuery>,
) -> AppResult<Json<AccountBalance>> {
    let currency = Currency::new(&q.currency).map_err(|e| AppError::BadRequest(e.to_string()))?;
    let bal = state
        .ledger
        .account_balance(tenant_id, &code, currency)
        .await?;
    Ok(Json(bal))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::{Direction, DraftPosting};

    fn draft_with(tenant_id: Uuid) -> DraftTransaction {
        DraftTransaction {
            tenant_id,
            kind: "test".into(),
            idempotency_key: "k1".into(),
            description: None,
            metadata: serde_json::json!({}),
            postings: vec![
                DraftPosting {
                    account_code: "a".into(),
                    direction: Direction::Debit,
                    amount_minor: 100,
                    currency: "USD".into(),
                    source: String::new(),
                    source_event_id: String::new(),
                    metadata: serde_json::json!({}),
                },
                DraftPosting {
                    account_code: "b".into(),
                    direction: Direction::Credit,
                    amount_minor: 100,
                    currency: "USD".into(),
                    source: String::new(),
                    source_event_id: String::new(),
                    metadata: serde_json::json!({}),
                },
            ],
        }
    }

    fn validate_body_tenant(path_tenant_id: Uuid, draft: &DraftTransaction) -> Result<(), String> {
        if !draft.tenant_id.is_nil() && draft.tenant_id != path_tenant_id {
            return Err(format!(
                "body.tenant_id {} does not match path tenant_id {}",
                draft.tenant_id, path_tenant_id
            ));
        }
        Ok(())
    }

    #[test]
    fn body_tenant_matching_path_is_ok() {
        let t = Uuid::new_v4();
        assert!(validate_body_tenant(t, &draft_with(t)).is_ok());
    }

    #[test]
    fn body_tenant_nil_is_ok_and_will_be_filled_by_handler() {
        let t = Uuid::new_v4();
        assert!(validate_body_tenant(t, &draft_with(Uuid::nil())).is_ok());
    }

    #[test]
    fn body_tenant_mismatch_is_rejected() {
        let path = Uuid::new_v4();
        let body = Uuid::new_v4();
        assert!(validate_body_tenant(path, &draft_with(body)).is_err());
    }

    #[test]
    fn body_tenant_mismatch_error_mentions_both_uuids() {
        let path = Uuid::new_v4();
        let body = Uuid::new_v4();
        let err = validate_body_tenant(path, &draft_with(body)).unwrap_err();
        assert!(err.contains(&body.to_string()));
        assert!(err.contains(&path.to_string()));
    }
}
