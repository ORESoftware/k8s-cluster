use axum::extract::{Path, Query, State};
use axum::Json;
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
    let currency = Currency::new(&input.currency)
        .map_err(|e| AppError::BadRequest(e.to_string()))?;
    let acct = state
        .ledger
        .ensure_account(tenant_id, region, input.user_id, input.kind, &input.code, currency)
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
    Json(mut draft): Json<DraftTransaction>,
) -> AppResult<Json<PostTxResp>> {
    if draft.tenant_id != tenant_id {
        draft.tenant_id = tenant_id;
    }
    let tenant = state.tenants.by_id(tenant_id).await?;
    let region = tenant.region()?;
    let tx_id = state.ledger.post_transaction(&draft, region).await?;
    Ok(Json(PostTxResp { transaction_id: tx_id }))
}

#[derive(Deserialize)]
pub struct BalanceQuery {
    #[serde(default = "default_currency")]
    pub currency: String,
}

fn default_currency() -> String { "USD".into() }

pub async fn account_balance(
    State(state): State<AppState>,
    Path((tenant_id, code)): Path<(Uuid, String)>,
    Query(q): Query<BalanceQuery>,
) -> AppResult<Json<AccountBalance>> {
    let currency = Currency::new(&q.currency)
        .map_err(|e| AppError::BadRequest(e.to_string()))?;
    let bal = state.ledger.account_balance(tenant_id, &code, currency).await?;
    Ok(Json(bal))
}
