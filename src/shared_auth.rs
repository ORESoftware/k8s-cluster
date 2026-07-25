//! Centralized identity verification through the shared-auth service.
//!
//! Contract source: <https://github.com/shared-auth/shared-auth-server.rs>.
//! This module owns only human identity verification. The sync server keeps
//! issuing separate per-device sync tokens whose hashes live in its database.

use crate::config::SharedAuthConfig;
use opentelemetry::global;
use opentelemetry_http::HeaderInjector;
use serde::Deserialize;
use std::time::Duration;
use tracing_opentelemetry::OpenTelemetrySpanExt;
use uuid::Uuid;

const MAX_AUTH_RESPONSE_BYTES: usize = 64 * 1024;
const MAX_ACCESS_TOKEN_BYTES: usize = 16 * 1024;

/// Bounds on the outbound identity call. Without them the only limit is the
/// router-wide 15s `TimeoutLayer`, so an upstream that accepts a connection and
/// then never answers pins a request slot (and a socket) for the full 15s per
/// attempt — a slow identity provider becomes resource exhaustion here instead
/// of a fast, honest degradation. Introspection is a single in-cluster hop, so
/// seconds are generous; a timeout maps to `SharedAuthError::Unavailable` and
/// therefore to 503 — enrollment is degraded, not broken, and the credential is
/// not evidence-of-invalid.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(4);

#[derive(Clone)]
pub struct SharedAuthClient {
    base_url: String,
    http: reqwest::Client,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedIdentity {
    pub shared_auth_user_id: Uuid,
    pub supabase_user_id: Option<Uuid>,
    pub email: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SharedAuthError {
    #[error("credential rejected")]
    Unauthenticated,
    #[error("shared-auth unavailable")]
    Unavailable,
}

#[derive(Deserialize)]
struct ExchangeResponse {
    access_token: String,
    shared_user_id: String,
    provider: String,
    provider_subject: String,
}

#[derive(Deserialize)]
struct IntrospectResponse {
    active: bool,
    #[serde(default)]
    sub: Option<String>,
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    provider_subject: Option<String>,
    #[serde(default)]
    email: Option<String>,
}

impl SharedAuthClient {
    /// Build the client **once**, at startup. `reqwest::Client` owns the
    /// connection pool, so it is cloned (cheaply, it is an `Arc` inside) rather
    /// than rebuilt per request; `SharedAuthClient` lives in `AppState`.
    pub fn new(config: SharedAuthConfig) -> Self {
        let http = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .build()
            // Only fails if the TLS backend cannot be initialized. Falling back
            // to an untimed client would silently reintroduce the hang this
            // guards against, so surface it instead.
            .expect("shared-auth HTTP client must build");
        Self {
            base_url: config.base_url,
            http,
        }
    }

    #[tracing::instrument(
        name = "shared_auth.exchange",
        skip_all,
        fields(auth.system = "shared-auth", auth.operation = "provider_exchange")
    )]
    pub async fn exchange_provider_token(
        &self,
        provider_token: &str,
    ) -> Result<VerifiedIdentity, SharedAuthError> {
        validate_token(provider_token)?;
        let response = self
            .http
            .post(format!("{}/auth/exchange", self.base_url))
            .headers(outbound_trace_headers())
            .bearer_auth(provider_token)
            .send()
            .await
            .map_err(|_| SharedAuthError::Unavailable)?;
        let body = successful_body(response).await?;
        let exchange: ExchangeResponse =
            serde_json::from_slice(&body).map_err(|_| SharedAuthError::Unavailable)?;
        if exchange.access_token.is_empty() || exchange.access_token.len() > MAX_ACCESS_TOKEN_BYTES
        {
            return Err(SharedAuthError::Unavailable);
        }
        let identity = verified_identity(
            &exchange.shared_user_id,
            &exchange.provider,
            &exchange.provider_subject,
            None,
        )?;
        tracing::info!(
            auth.authority = "shared-auth",
            auth.outcome = "authenticated",
            "provider token exchanged"
        );
        Ok(identity)
    }

    #[tracing::instrument(
        name = "shared_auth.introspect",
        skip_all,
        fields(auth.system = "shared-auth", auth.operation = "introspect")
    )]
    pub async fn introspect_access_token(
        &self,
        token: &str,
    ) -> Result<VerifiedIdentity, SharedAuthError> {
        validate_token(token)?;
        let response = self
            .http
            .post(format!("{}/auth/introspect", self.base_url))
            .headers(outbound_trace_headers())
            .json(&serde_json::json!({ "token": token }))
            .send()
            .await
            .map_err(|_| SharedAuthError::Unavailable)?;
        let body = successful_body(response).await?;
        let result: IntrospectResponse =
            serde_json::from_slice(&body).map_err(|_| SharedAuthError::Unavailable)?;
        if !result.active {
            tracing::info!(auth.outcome = "unauthenticated", "access token inactive");
            return Err(SharedAuthError::Unauthenticated);
        }
        let identity = verified_identity(
            result.sub.as_deref().ok_or(SharedAuthError::Unavailable)?,
            result
                .provider
                .as_deref()
                .ok_or(SharedAuthError::Unavailable)?,
            result
                .provider_subject
                .as_deref()
                .ok_or(SharedAuthError::Unavailable)?,
            result.email,
        )?;
        tracing::info!(
            auth.authority = "shared-auth",
            auth.outcome = "authenticated",
            "access token active"
        );
        Ok(identity)
    }
}

fn validate_token(token: &str) -> Result<(), SharedAuthError> {
    if token.is_empty() || token.len() > MAX_ACCESS_TOKEN_BYTES {
        Err(SharedAuthError::Unauthenticated)
    } else {
        Ok(())
    }
}

async fn successful_body(mut response: reqwest::Response) -> Result<Vec<u8>, SharedAuthError> {
    if matches!(
        response.status(),
        reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN
    ) {
        tracing::info!(auth.outcome = "unauthenticated", "credential rejected");
        return Err(SharedAuthError::Unauthenticated);
    }
    if !response.status().is_success() {
        tracing::warn!(
            auth.outcome = "degraded",
            http.response.status_code = response.status().as_u16(),
            "shared-auth request unavailable"
        );
        return Err(SharedAuthError::Unavailable);
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_AUTH_RESPONSE_BYTES as u64)
    {
        return Err(SharedAuthError::Unavailable);
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| SharedAuthError::Unavailable)?
    {
        if body.len().saturating_add(chunk.len()) > MAX_AUTH_RESPONSE_BYTES {
            return Err(SharedAuthError::Unavailable);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn verified_identity(
    shared_user_id: &str,
    provider: &str,
    provider_subject: &str,
    email: Option<String>,
) -> Result<VerifiedIdentity, SharedAuthError> {
    let shared_auth_user_id =
        Uuid::parse_str(shared_user_id).map_err(|_| SharedAuthError::Unavailable)?;
    if provider.is_empty() || provider_subject.is_empty() {
        return Err(SharedAuthError::Unavailable);
    }
    let supabase_user_id = if provider == "supabase" {
        Some(Uuid::parse_str(provider_subject).map_err(|_| SharedAuthError::Unavailable)?)
    } else {
        None
    };
    Ok(VerifiedIdentity {
        shared_auth_user_id,
        supabase_user_id,
        email,
    })
}

fn outbound_trace_headers() -> reqwest::header::HeaderMap {
    let mut headers = reqwest::header::HeaderMap::new();
    let context = tracing::Span::current().context();
    global::get_text_map_propagator(|propagator| {
        propagator.inject_context(&context, &mut HeaderInjector(&mut headers));
    });
    headers
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;
    use axum::routing::post;
    use axum::{Json, Router};

    const SHARED_USER_ID: &str = "00000000-0000-4000-8000-000000000001";
    const PROVIDER_SUBJECT: &str = "00000000-0000-4000-8000-000000000002";

    async fn mock_service(
        exchange_status: StatusCode,
        active: bool,
    ) -> (SharedAuthClient, tokio::task::JoinHandle<()>) {
        let app = Router::new()
            .route(
                "/auth/exchange",
                post(move || async move {
                    (
                        exchange_status,
                        Json(serde_json::json!({
                            "access_token": "shared-access-token",
                            "shared_user_id": SHARED_USER_ID,
                            "provider": "supabase",
                            "provider_subject": PROVIDER_SUBJECT
                        })),
                    )
                }),
            )
            .route(
                "/auth/introspect",
                post(move || async move {
                    Json(if active {
                        serde_json::json!({
                            "active": true,
                            "sub": SHARED_USER_ID,
                            "provider": "supabase",
                            "provider_subject": PROVIDER_SUBJECT,
                            "email": "not-logged@example.test"
                        })
                    } else {
                        serde_json::json!({ "active": false })
                    })
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let client = SharedAuthClient::new(SharedAuthConfig {
            base_url: format!("http://{address}"),
        });
        (client, task)
    }

    #[tokio::test]
    async fn exchange_maps_provider_identity_without_returning_tokens() {
        let (client, task) = mock_service(StatusCode::OK, true).await;
        let identity = client
            .exchange_provider_token("provider-token")
            .await
            .unwrap();
        assert_eq!(identity.shared_auth_user_id.to_string(), SHARED_USER_ID);
        assert_eq!(
            identity.supabase_user_id.unwrap().to_string(),
            PROVIDER_SUBJECT
        );
        assert_eq!(identity.email, None);
        task.abort();
    }

    #[tokio::test]
    async fn introspection_preserves_inactive_as_unauthenticated() {
        let (client, task) = mock_service(StatusCode::OK, false).await;
        assert_eq!(
            client.introspect_access_token("shared-token").await,
            Err(SharedAuthError::Unauthenticated)
        );
        task.abort();
    }

    #[tokio::test]
    async fn upstream_failures_are_degraded_not_invalid_credentials() {
        let (client, task) = mock_service(StatusCode::SERVICE_UNAVAILABLE, true).await;
        assert_eq!(
            client.exchange_provider_token("provider-token").await,
            Err(SharedAuthError::Unavailable)
        );
        task.abort();
    }

    #[tokio::test]
    async fn an_upstream_that_never_answers_degrades_within_the_client_timeout() {
        // The regression this pins: with a bare `reqwest::Client::new()` the
        // only bound was the router's 15s timeout, so every hung enrollment held
        // a request slot for 15s. It must now fail fast, and as *unavailable* —
        // an unanswering authority is not evidence a credential is bad.
        let app = Router::new().route(
            "/auth/introspect",
            post(|| async {
                std::future::pending::<()>().await;
                StatusCode::OK
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let client = SharedAuthClient::new(SharedAuthConfig {
            base_url: format!("http://{address}"),
        });

        let started = std::time::Instant::now();
        assert_eq!(
            client.introspect_access_token("shared-token").await,
            Err(SharedAuthError::Unavailable)
        );
        let elapsed = started.elapsed();
        assert!(
            elapsed < REQUEST_TIMEOUT * 2,
            "the client's own timeout must end the call, took {elapsed:?}"
        );
        task.abort();
    }

    #[tokio::test]
    async fn exchange_rejects_oversized_responses_before_deserializing() {
        let app = Router::new().route(
            "/auth/exchange",
            post(|| async { "x".repeat(MAX_AUTH_RESPONSE_BYTES + 1) }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let client = SharedAuthClient::new(SharedAuthConfig {
            base_url: format!("http://{address}"),
        });

        assert_eq!(
            client.exchange_provider_token("provider-token").await,
            Err(SharedAuthError::Unavailable)
        );
        task.abort();
    }
}
