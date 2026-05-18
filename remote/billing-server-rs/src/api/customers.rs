use axum::extract::{Path, Query, State};
use axum::Json;
use serde::Deserialize;
use uuid::Uuid;

use crate::customers::BillingState;
use crate::error::AppResult;
use crate::money::Currency;
use crate::state::AppState;

#[derive(Deserialize)]
pub struct CurrencyQuery {
    #[serde(default = "default_currency")]
    pub currency: String,
}

fn default_currency() -> String { "USD".into() }

pub async fn billing_state(
    State(state): State<AppState>,
    Path((tenant_id, email)): Path<(Uuid, String)>,
    Query(q): Query<CurrencyQuery>,
) -> AppResult<Json<BillingState>> {
    let currency = Currency::new(&q.currency).map_err(|e| {
        crate::error::AppError::BadRequest(format!("invalid currency: {e}"))
    })?;
    let bs = state.customers.billing_state(tenant_id, &email, currency).await?;
    Ok(Json(bs))
}
