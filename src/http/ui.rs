//! HTML UI handlers (Maud). Returns HTML blobs; the auth API stays JSON.

use axum::{
    extract::State,
    response::{Html, IntoResponse},
    Form,
};
use serde::Deserialize;

use crate::state::AppState;
use crate::views;

use super::exchange::perform_exchange;

/// GET / — status landing page.
pub async fn landing(State(state): State<AppState>) -> Html<String> {
    Html(
        views::landing(
            state.supabase.len(),
            &state.config.signing.issuer,
            state.db.is_some(),
        )
        .into_string(),
    )
}

/// GET /ui — the token-exchange helper form.
pub async fn sign_in() -> Html<String> {
    Html(views::sign_in().into_string())
}

#[derive(Debug, Deserialize)]
pub struct UiExchangeForm {
    access_token: String,
}

/// POST /ui/exchange — runs the exchange and returns an HTML result.
pub async fn ui_exchange(
    State(state): State<AppState>,
    Form(form): Form<UiExchangeForm>,
) -> impl IntoResponse {
    let token = form.access_token.trim();
    if token.is_empty() {
        return Html(views::error_fragment().into_string());
    }
    match perform_exchange(&state, token).await {
        Ok(exchanged) => {
            // Decode our own token for display (it just came from our minter).
            match state.minter.verify(&exchanged.access_token) {
                Ok(claims) => Html(
                    views::exchange_result(&exchanged.access_token, &exchanged.project, &claims)
                        .into_string(),
                ),
                Err(_) => Html(views::error_fragment().into_string()),
            }
        }
        Err(_) => Html(views::error_fragment().into_string()),
    }
}
