use axum::extract::{Path, Query, State};
use axum::Json;
use uuid::Uuid;

use crate::error::AppResult;
use crate::money::Currency;
use crate::state::AppState;
use crate::vendors::PayableState;

use super::customers::CurrencyQuery;

pub async fn payable_state(
    State(state): State<AppState>,
    Path((tenant_id, email)): Path<(Uuid, String)>,
    Query(q): Query<CurrencyQuery>,
) -> AppResult<Json<PayableState>> {
    let currency = Currency::new(&q.currency).map_err(|e| {
        crate::error::AppError::BadRequest(format!("invalid currency: {e}"))
    })?;
    let ps = state.vendors.payable_state(tenant_id, &email, currency).await?;
    Ok(Json(ps))
}
