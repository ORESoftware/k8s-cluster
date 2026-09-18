use axum::Json;
use axum::extract::{Path, Query, State};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{AppError, AppResult};
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
}

pub async fn post_transaction(
    State(state): State<AppState>,
    Path(tenant_id): Path<Uuid>,
    Json(draft): Json<DraftTransaction>,
) -> AppResult<Json<PostTxResp>> {
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
    let tenant = state.tenants.by_id(tenant_id).await?;
    let region = tenant.region()?;
    let tx_id = state.ledger.post_transaction(&draft, region).await?;
    Ok(Json(PostTxResp {
        transaction_id: tx_id,
    }))
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

    /// Replicate the body/path tenant-mismatch check that the handler
    /// runs before touching the DB. We can't easily mock `AppState`
    /// here, but the check itself is the load-bearing fix; keep its
    /// behavior pinned with a unit test that mirrors the handler's
    /// branch order.
    fn validate_body_tenant(
        path_tenant_id: Uuid,
        draft: &DraftTransaction,
    ) -> Result<(), String> {
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
