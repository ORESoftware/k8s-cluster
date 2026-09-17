//! Client boundary for the centralized shared-auth service.
//!
//! Contract source: <https://github.com/shared-auth/shared-auth-server.rs>.
//! Provider credentials are exchanged once at login; protected routes then
//! validate only the short-lived shared-auth access token.

use crate::config::SharedAuthConfig;
use opentelemetry::global;
use opentelemetry_http::HeaderInjector;
use serde::Deserialize;
use tracing_opentelemetry::OpenTelemetrySpanExt;

const MAX_AUTH_RESPONSE_BYTES: usize = 64 * 1024;
const MAX_ACCESS_TOKEN_BYTES: usize = 16 * 1024;

#[derive(Clone)]
pub(crate) struct SharedAuthClient {
    base_url: String,
    http: reqwest::Client,
    #[cfg(test)]
    forced_verdict: Option<AuthVerdict>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AuthVerdict {
    Authenticated,
    Unauthenticated,
    Degraded,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub(crate) enum SharedAuthError {
    #[error("credential rejected")]
    Unauthenticated,
    #[error("shared-auth unavailable")]
    Unavailable,
}

#[derive(Deserialize)]
struct ExchangeResponse {
    access_token: String,
}

impl SharedAuthClient {
    pub(crate) fn new(config: SharedAuthConfig, http: reqwest::Client) -> Self {
        Self {
            base_url: config.base_url,
            http,
            #[cfg(test)]
            forced_verdict: None,
        }
    }

    #[tracing::instrument(
        name = "shared_auth.exchange",
        skip_all,
        fields(auth.system = "shared-auth", auth.operation = "provider_exchange")
    )]
    pub(crate) async fn exchange_provider_token(
        &self,
        provider_token: &str,
    ) -> Result<String, SharedAuthError> {
        if provider_token.is_empty() || provider_token.len() > MAX_ACCESS_TOKEN_BYTES {
            return Err(SharedAuthError::Unauthenticated);
        }
        let response = self
            .http
            .post(format!("{}/auth/exchange", self.base_url))
            .headers(outbound_trace_headers())
            .bearer_auth(provider_token)
            .send()
            .await
            .map_err(|_| SharedAuthError::Unavailable)?;

        if matches!(
            response.status(),
            reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN
        ) {
            tracing::info!(auth.outcome = "unauthenticated", "provider token rejected");
            return Err(SharedAuthError::Unauthenticated);
        }
        if !response.status().is_success() {
            tracing::warn!(
                auth.outcome = "degraded",
                http.response.status_code = response.status().as_u16(),
                "shared-auth exchange unavailable"
            );
            return Err(SharedAuthError::Unavailable);
        }

        let body = bounded_body(response).await?;
        let exchange: ExchangeResponse =
            serde_json::from_slice(&body).map_err(|_| SharedAuthError::Unavailable)?;
        if exchange.access_token.is_empty() || exchange.access_token.len() > MAX_ACCESS_TOKEN_BYTES
        {
            return Err(SharedAuthError::Unavailable);
        }
        tracing::info!(
            auth.authority = "shared-auth",
            auth.outcome = "authenticated",
            "provider token exchanged"
        );
        Ok(exchange.access_token)
    }

    #[tracing::instrument(
        name = "shared_auth.verify",
        skip_all,
        fields(auth.system = "shared-auth", auth.operation = "verify")
    )]
    pub(crate) async fn verify_access_token(&self, token: &str) -> AuthVerdict {
        #[cfg(test)]
        if let Some(verdict) = self.forced_verdict {
            return verdict;
        }

        if token.is_empty() || token.len() > MAX_ACCESS_TOKEN_BYTES {
            return AuthVerdict::Unauthenticated;
        }
        let response = self
            .http
            .get(format!("{}/auth/verify", self.base_url))
            .headers(outbound_trace_headers())
            .bearer_auth(token)
            .send()
            .await;
        let verdict = match response {
            Ok(response) if response.status().is_success() => AuthVerdict::Authenticated,
            Ok(response)
                if matches!(
                    response.status(),
                    reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN
                ) =>
            {
                AuthVerdict::Unauthenticated
            }
            Ok(_) | Err(_) => AuthVerdict::Degraded,
        };
        tracing::info!(
            auth.authority = "shared-auth",
            auth.outcome = match verdict {
                AuthVerdict::Authenticated => "authenticated",
                AuthVerdict::Unauthenticated => "unauthenticated",
                AuthVerdict::Degraded => "degraded",
            },
            "session verification completed"
        );
        verdict
    }

    #[cfg(test)]
    pub(crate) fn accepting_for_tests() -> Self {
        Self {
            base_url: "http://127.0.0.1:1".to_owned(),
            http: reqwest::Client::new(),
            forced_verdict: Some(AuthVerdict::Authenticated),
        }
    }
}

async fn bounded_body(mut response: reqwest::Response) -> Result<Vec<u8>, SharedAuthError> {
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

pub(crate) fn outbound_trace_headers() -> reqwest::header::HeaderMap {
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
    use axum::routing::{get, post};
    use axum::{Json, Router};

    async fn mock_service(
        exchange_status: StatusCode,
        verify_status: StatusCode,
    ) -> (SharedAuthClient, tokio::task::JoinHandle<()>) {
        let app = Router::new()
            .route(
                "/auth/exchange",
                post(move || async move {
                    (
                        exchange_status,
                        Json(serde_json::json!({ "access_token": "shared-access-token" })),
                    )
                }),
            )
            .route("/auth/verify", get(move || async move { verify_status }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let client = SharedAuthClient::new(
            SharedAuthConfig {
                base_url: format!("http://{address}"),
            },
            reqwest::Client::new(),
        );
        (client, task)
    }

    #[tokio::test]
    async fn exchanges_provider_tokens_for_shared_auth_tokens() {
        let (client, task) = mock_service(StatusCode::OK, StatusCode::OK).await;
        assert_eq!(
            client.exchange_provider_token("provider-token").await,
            Ok("shared-access-token".to_owned())
        );
        task.abort();
    }

    #[tokio::test]
    async fn verification_preserves_unauthenticated_and_degraded() {
        let (client, task) = mock_service(StatusCode::OK, StatusCode::UNAUTHORIZED).await;
        assert_eq!(
            client.verify_access_token("shared-token").await,
            AuthVerdict::Unauthenticated
        );
        task.abort();

        let (client, task) = mock_service(StatusCode::OK, StatusCode::SERVICE_UNAVAILABLE).await;
        assert_eq!(
            client.verify_access_token("shared-token").await,
            AuthVerdict::Degraded
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
        let client = SharedAuthClient::new(
            SharedAuthConfig {
                base_url: format!("http://{address}"),
            },
            reqwest::Client::new(),
        );

        assert_eq!(
            client.exchange_provider_token("provider-token").await,
            Err(SharedAuthError::Unavailable)
        );
        task.abort();
    }
}
