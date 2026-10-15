//! Supabase-backed login routes.

use crate::cookies::{set_cookie, EMAIL_COOKIE, SESSION_COOKIE};
use crate::shared_auth::{outbound_trace_headers, SharedAuthError};
use crate::state::AppState;
use crate::views::{login_form, page};
use axum::extract::{Form, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use maud::html;
use serde::Deserialize;

pub(crate) async fn page_handler(State(state): State<AppState>) -> impl IntoResponse {
    page(
        "Sign in",
        login_form(
            None,
            state.supabase.is_some() && state.shared_auth.is_some(),
        ),
    )
}

#[derive(Deserialize)]
pub(crate) struct LoginForm {
    email: String,
    password: String,
}

#[tracing::instrument(
    name = "auth.login",
    skip_all,
    fields(auth.provider = "supabase", auth.authority = "shared-auth")
)]
pub(crate) async fn submit(State(state): State<AppState>, Form(form): Form<LoginForm>) -> Response {
    let (Some(supabase), Some(shared_auth)) = (&state.supabase, &state.shared_auth) else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Html(
                html! {
                    div class="card" id="login-box" {
                        h1 { "Sign in to 3FA" }
                        p class="error" {
                            "Authentication is not configured — set the Supabase provider and shared-auth service settings."
                        }
                    }
                }
                .into_string(),
            ),
        )
            .into_response();
    };

    let result = state
        .http
        .post(format!(
            "{}/auth/v1/token?grant_type=password",
            supabase.url
        ))
        .headers(outbound_trace_headers())
        .header("apikey", &supabase.anon_key)
        .bearer_auth(&supabase.anon_key)
        .json(&serde_json::json!({ "email": form.email, "password": form.password }))
        .send()
        .await;
    let response = match result {
        Ok(response) => response,
        Err(error) => {
            tracing::warn!(error = %error, auth.provider = "supabase", "auth provider unavailable");
            return (
                StatusCode::BAD_GATEWAY,
                page(
                    "Sign in",
                    login_form(Some("Could not reach the auth service — try again."), true),
                ),
            )
                .into_response();
        }
    };

    if !response.status().is_success() {
        tracing::info!(
            auth.provider = "supabase",
            auth.outcome = "rejected",
            "login rejected"
        );
        return (
            StatusCode::UNAUTHORIZED,
            Html(login_form(Some("Invalid email or password."), true).into_string()),
        )
            .into_response();
    }

    let body: serde_json::Value = response.json().await.unwrap_or_default();
    let Some(provider_access_token) = body["access_token"]
        .as_str()
        .filter(|token| !token.is_empty())
    else {
        tracing::warn!(
            auth.provider = "supabase",
            "auth response omitted access token"
        );
        return (
            StatusCode::BAD_GATEWAY,
            Html(login_form(Some("Unexpected auth response — try again."), true).into_string()),
        )
            .into_response();
    };
    let email = body["user"]["email"].as_str().unwrap_or_default();
    let shared_access_token = match shared_auth
        .exchange_provider_token(provider_access_token)
        .await
    {
        Ok(token) => token,
        Err(SharedAuthError::Unauthenticated) => {
            tracing::warn!(
                auth.outcome = "provider_exchange_rejected",
                "shared-auth rejected a provider token minted moments earlier"
            );
            return (
                StatusCode::BAD_GATEWAY,
                Html(
                    login_form(Some("The auth services disagreed — try again."), true)
                        .into_string(),
                ),
            )
                .into_response();
        }
        Err(SharedAuthError::Unavailable) => {
            tracing::warn!(
                auth.outcome = "degraded",
                "shared-auth unavailable during login"
            );
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Html(
                    login_form(
                        Some("Sign-in is temporarily unavailable — try again."),
                        true,
                    )
                    .into_string(),
                ),
            )
                .into_response();
        }
    };

    let mut response = (
        StatusCode::OK,
        Html(
            html! {
                div class="card" id="login-box" {
                    p class="success" { "Signed in — loading enrollment…" }
                }
            }
            .into_string(),
        ),
    )
        .into_response();
    let headers = response.headers_mut();
    headers.append(
        header::SET_COOKIE,
        set_cookie(SESSION_COOKIE, &shared_access_token),
    );
    if !email.is_empty() {
        headers.append(header::SET_COOKIE, set_cookie(EMAIL_COOKIE, email));
    }
    headers.insert("HX-Redirect", HeaderValue::from_static("/enroll"));
    tracing::info!(
        auth.provider = "supabase",
        auth.authority = "shared-auth",
        auth.outcome = "accepted",
        "login accepted"
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{SharedAuthConfig, SupabaseConfig};
    use axum::routing::post;
    use axum::{Json, Router};

    async fn mock_provider(status: StatusCode) -> (String, tokio::task::JoinHandle<()>) {
        let app = Router::new().route(
            "/auth/v1/token",
            post(move || async move {
                if status.is_success() {
                    (
                        status,
                        Json(serde_json::json!({
                            "access_token": "provider-access-token",
                            "user": { "email": "user@example.test" }
                        })),
                    )
                } else {
                    (status, Json(serde_json::json!({ "error": "rejected" })))
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{address}"), task)
    }

    async fn mock_shared_auth(status: StatusCode) -> (String, tokio::task::JoinHandle<()>) {
        let app = Router::new().route(
            "/auth/exchange",
            post(move || async move {
                (
                    status,
                    Json(serde_json::json!({ "access_token": "shared-access-token" })),
                )
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{address}"), task)
    }

    fn state(provider_url: String, shared_auth_url: String) -> AppState {
        AppState::new(
            Some(SupabaseConfig {
                url: provider_url,
                anon_key: "public-anon-key".to_owned(),
            }),
            Some(SharedAuthConfig {
                base_url: shared_auth_url,
            }),
            b"test-server-secret".to_vec(),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn successful_provider_response_becomes_secure_session_cookies() {
        let (provider_url, provider) = mock_provider(StatusCode::OK).await;
        let (shared_auth_url, shared_auth) = mock_shared_auth(StatusCode::OK).await;
        let response = submit(
            State(state(provider_url, shared_auth_url)),
            Form(LoginForm {
                email: "user@example.test".to_owned(),
                password: "not-logged".to_owned(),
            }),
        )
        .await;
        provider.abort();
        shared_auth.abort();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["HX-Redirect"], "/enroll");
        let cookies = response
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .map(|value| value.to_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(cookies.len(), 2);
        assert!(cookies.iter().any(|cookie| {
            cookie.starts_with("threefa_session=shared-access-token;")
                && cookie.contains("HttpOnly")
                && cookie.contains("Secure")
        }));
        assert!(cookies
            .iter()
            .any(|cookie| cookie.starts_with("threefa_email=user@example.test;")));
    }

    #[tokio::test]
    async fn provider_rejection_sets_no_session_cookie() {
        let (provider_url, provider) = mock_provider(StatusCode::UNAUTHORIZED).await;
        let (shared_auth_url, shared_auth) = mock_shared_auth(StatusCode::OK).await;
        let response = submit(
            State(state(provider_url, shared_auth_url)),
            Form(LoginForm {
                email: "user@example.test".to_owned(),
                password: "wrong".to_owned(),
            }),
        )
        .await;
        provider.abort();
        shared_auth.abort();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert!(response.headers().get(header::SET_COOKIE).is_none());
    }

    #[tokio::test]
    async fn shared_auth_outage_is_degraded_and_sets_no_session_cookie() {
        let (provider_url, provider) = mock_provider(StatusCode::OK).await;
        let (shared_auth_url, shared_auth) =
            mock_shared_auth(StatusCode::SERVICE_UNAVAILABLE).await;
        let response = submit(
            State(state(provider_url, shared_auth_url)),
            Form(LoginForm {
                email: "user@example.test".to_owned(),
                password: "not-logged".to_owned(),
            }),
        )
        .await;
        provider.abort();
        shared_auth.abort();

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert!(response.headers().get(header::SET_COOKIE).is_none());
    }
}
