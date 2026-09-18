use axum::Json;
use axum::extract::{Path, State};
use uuid::Uuid;

use crate::error::AppResult;
use crate::solana::verify::VerifyResult;
use crate::state::AppState;

pub async fn verify_posting(
    State(state): State<AppState>,
    Path((tenant_id, posting_id)): Path<(Uuid, i64)>,
) -> AppResult<Json<VerifyResult>> {
    let r = state.verifier.verify_posting(tenant_id, posting_id).await?;
    Ok(Json(r))
}
