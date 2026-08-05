//! Hosted checkout creation for trusted product services.
//!
//! Quaestor is the only service that sees the Stripe secret key. Product
//! services submit an already-computed amount and an idempotent external
//! reference; Stripe webhooks continue to enter through Quaestor's existing
//! signed, encrypted-at-rest webhook pipeline.

use std::{collections::BTreeMap, env, sync::Arc};

use axum::{
    Extension, Json, Router,
    extract::State,
    middleware,
    routing::post,
};
use serde::{Deserialize, Serialize};
use url::Url;
use uuid::Uuid;

use crate::{
    api::auth::{ApiAuth, Principal, require_api_auth},
    error::{AppError, AppResult},
    state::AppState,
};

const STRIPE_CHECKOUT_SESSIONS_URL: &str = "https://api.stripe.com/v1/checkout/sessions";
const MIN_AMOUNT_MINOR: u64 = 50;
const MAX_AMOUNT_MINOR: u64 = 100_000_000;
const MAX_METADATA_ENTRIES: usize = 20;

#[derive(Clone)]
struct CheckoutState {
    app: AppState,
    http: reqwest::Client,
    allowed_return_prefixes: Arc<Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub struct CreateCheckoutSession {
    pub tenant_id: Uuid,
    pub idempotency_key: String,
    pub external_reference: String,
    pub customer_email: Option<String>,
    pub amount_minor: u64,
    pub currency: String,
    pub description: String,
    pub success_url: String,
    pub cancel_url: String,
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
}

#[derive(Debug, Serialize)]
pub struct CheckoutSessionView {
    pub id: String,
    pub url: String,
    pub status: Option<String>,
    pub payment_status: Option<String>,
    pub expires_at: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct StripeCheckoutSession {
    id: String,
    url: Option<String>,
    status: Option<String>,
    payment_status: Option<String>,
    expires_at: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct StripeErrorEnvelope {
    error: Option<StripeErrorBody>,
}

#[derive(Debug, Deserialize)]
struct StripeErrorBody {
    message: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
    code: Option<String>,
}

pub fn router(state: AppState) -> anyhow::Result<Router> {
    let auth = ApiAuth::from_state(&state)?;
    let checkout_state = CheckoutState {
        app: state,
        http: reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(20))
            .build()?,
        allowed_return_prefixes: Arc::new(parse_csv_env(
            "BILLING_CHECKOUT_RETURN_TO_ALLOWED_PREFIXES",
        )),
    };

    Ok(Router::new()
        .route(
            "/v1/commerce/checkout-sessions",
            post(create_checkout_session),
        )
        .layer(middleware::from_fn_with_state(auth, require_api_auth))
        .with_state(checkout_state))
}

async fn create_checkout_session(
    State(state): State<CheckoutState>,
    principal: Option<Extension<Principal>>,
    Json(request): Json<CreateCheckoutSession>,
) -> AppResult<Json<CheckoutSessionView>> {
    // This endpoint accepts a tenant id in the body and creates a charge. Keep
    // it service-to-service: a user JWT must never be able to choose an
    // arbitrary tenant and amount. In explicit insecure development, where no
    // API bearer is configured, the missing extension is tolerated.
    let service_principal = matches!(
        principal.as_ref().map(|wrapped| &wrapped.0),
        Some(Principal::Service)
    );
    if state.app.cfg.api_auth_bearer.is_some() && !service_principal {
        return Err(AppError::Forbidden);
    }

    request.validate(&state.allowed_return_prefixes)?;
    let stripe_key = state
        .app
        .cfg
        .stripe_api_key()
        .ok_or_else(|| AppError::BadRequest("Stripe checkout is not configured".into()))?;

    let mut form = vec![
        ("mode".to_owned(), "payment".to_owned()),
        (
            "payment_method_types[0]".to_owned(),
            "card".to_owned(),
        ),
        ("success_url".to_owned(), request.success_url.clone()),
        ("cancel_url".to_owned(), request.cancel_url.clone()),
        (
            "client_reference_id".to_owned(),
            request.external_reference.clone(),
        ),
        ("line_items[0][quantity]".to_owned(), "1".to_owned()),
        (
            "line_items[0][price_data][currency]".to_owned(),
            request.currency.to_ascii_lowercase(),
        ),
        (
            "line_items[0][price_data][unit_amount]".to_owned(),
            request.amount_minor.to_string(),
        ),
        (
            "line_items[0][price_data][product_data][name]".to_owned(),
            request.description.clone(),
        ),
        (
            "metadata[tenant_id]".to_owned(),
            request.tenant_id.to_string(),
        ),
        (
            "metadata[external_reference]".to_owned(),
            request.external_reference.clone(),
        ),
        (
            "payment_intent_data[metadata][tenant_id]".to_owned(),
            request.tenant_id.to_string(),
        ),
        (
            "payment_intent_data[metadata][external_reference]".to_owned(),
            request.external_reference.clone(),
        ),
    ];
    if let Some(email) = request.customer_email.as_deref() {
        form.push(("customer_email".to_owned(), email.to_owned()));
    }
    for (key, value) in &request.metadata {
        form.push((format!("metadata[{key}]"), value.clone()));
        form.push((
            format!("payment_intent_data[metadata][{key}]"),
            value.clone(),
        ));
    }

    let response = state
        .http
        .post(STRIPE_CHECKOUT_SESSIONS_URL)
        .basic_auth(stripe_key, Some(""))
        .header("Stripe-Version", &state.app.cfg.stripe_api_version)
        .header("Idempotency-Key", &request.idempotency_key)
        .form(&form)
        .send()
        .await
        .map_err(|error| AppError::Provider {
            provider: "stripe".into(),
            message: format!("checkout session HTTP request failed: {error}"),
        })?;

    let status = response.status();
    let retry_after_seconds = response
        .headers()
        .get("retry-after")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(30);
    let bytes = response.bytes().await.map_err(|error| AppError::Provider {
        provider: "stripe".into(),
        message: format!("checkout session response body failed: {error}"),
    })?;

    if !status.is_success() {
        let stripe_error = serde_json::from_slice::<StripeErrorEnvelope>(&bytes)
            .ok()
            .and_then(|envelope| envelope.error)
            .map(|error| {
                format!(
                    "{}{}{}",
                    error.kind.unwrap_or_else(|| "stripe_error".into()),
                    error
                        .code
                        .map(|code| format!("/{code}"))
                        .unwrap_or_default(),
                    error
                        .message
                        .map(|message| format!(": {message}"))
                        .unwrap_or_default()
                )
            })
            .unwrap_or_else(|| format!("Stripe returned HTTP {status}"));
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(AppError::ProviderRateLimited {
                provider: "stripe".into(),
                retry_after_seconds,
                message: stripe_error,
            });
        }
        return Err(AppError::Provider {
            provider: "stripe".into(),
            message: stripe_error,
        });
    }

    let session: StripeCheckoutSession =
        serde_json::from_slice(&bytes).map_err(|error| AppError::Provider {
            provider: "stripe".into(),
            message: format!("checkout session response decode failed: {error}"),
        })?;
    let url = session.url.ok_or_else(|| AppError::Provider {
        provider: "stripe".into(),
        message: "checkout session response did not contain a hosted URL".into(),
    })?;

    Ok(Json(CheckoutSessionView {
        id: session.id,
        url,
        status: session.status,
        payment_status: session.payment_status,
        expires_at: session.expires_at,
    }))
}

impl CreateCheckoutSession {
    fn validate(&self, allowed_return_prefixes: &[String]) -> AppResult<()> {
        validate_text("idempotency_key", &self.idempotency_key, 8, 255)?;
        validate_text("external_reference", &self.external_reference, 1, 200)?;
        validate_text("description", &self.description, 1, 200)?;
        if !(MIN_AMOUNT_MINOR..=MAX_AMOUNT_MINOR).contains(&self.amount_minor) {
            return Err(AppError::BadRequest(format!(
                "amount_minor must be between {MIN_AMOUNT_MINOR} and {MAX_AMOUNT_MINOR}"
            )));
        }
        if self.currency.len() != 3 || !self.currency.bytes().all(|byte| byte.is_ascii_alphabetic())
        {
            return Err(AppError::BadRequest(
                "currency must be a three-letter ISO code".into(),
            ));
        }
        if let Some(email) = self.customer_email.as_deref() {
            validate_email(email)?;
        }
        validate_return_url(
            "success_url",
            &self.success_url,
            allowed_return_prefixes,
        )?;
        validate_return_url("cancel_url", &self.cancel_url, allowed_return_prefixes)?;
        if self.metadata.len() > MAX_METADATA_ENTRIES {
            return Err(AppError::BadRequest(format!(
                "metadata may contain at most {MAX_METADATA_ENTRIES} entries"
            )));
        }
        for (key, value) in &self.metadata {
            if key.is_empty()
                || key.len() > 40
                || !key
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
            {
                return Err(AppError::BadRequest(
                    "metadata keys must be 1-40 ASCII letters, digits, '_' or '-'".into(),
                ));
            }
            validate_text("metadata value", value, 0, 500)?;
        }
        Ok(())
    }
}

fn validate_text(name: &str, value: &str, min: usize, max: usize) -> AppResult<()> {
    let length = value.trim().len();
    if length < min || length > max || value.chars().any(char::is_control) {
        return Err(AppError::BadRequest(format!(
            "{name} must contain {min}-{max} control-free bytes"
        )));
    }
    Ok(())
}

fn validate_email(email: &str) -> AppResult<()> {
    validate_text("customer_email", email, 3, 254)?;
    let mut parts = email.split('@');
    let local = parts.next().unwrap_or_default();
    let domain = parts.next().unwrap_or_default();
    if local.is_empty() || domain.is_empty() || parts.next().is_some() || !domain.contains('.') {
        return Err(AppError::BadRequest(
            "customer_email must be a valid email address".into(),
        ));
    }
    Ok(())
}

fn validate_return_url(name: &str, raw: &str, allowed_prefixes: &[String]) -> AppResult<()> {
    if allowed_prefixes.is_empty() {
        return Err(AppError::BadRequest(
            "BILLING_CHECKOUT_RETURN_TO_ALLOWED_PREFIXES is empty".into(),
        ));
    }
    let parsed = Url::parse(raw)
        .map_err(|_| AppError::BadRequest(format!("{name} must be an absolute URL")))?;
    if parsed.username() != ""
        || parsed.password().is_some()
        || parsed.fragment().is_some()
        || !matches!(parsed.scheme(), "https" | "http")
    {
        return Err(AppError::BadRequest(format!(
            "{name} has a forbidden URL shape"
        )));
    }
    let host = parsed.host_str().unwrap_or_default();
    let local = matches!(host, "localhost" | "127.0.0.1" | "::1");
    if parsed.scheme() != "https" && !local {
        return Err(AppError::BadRequest(format!(
            "{name} must use HTTPS outside localhost"
        )));
    }

    let candidate = raw.trim_end_matches('/');
    let allowed = allowed_prefixes.iter().any(|prefix| {
        let prefix = prefix.trim_end_matches('/');
        candidate == prefix
            || candidate.starts_with(&format!("{prefix}/"))
            || candidate.starts_with(&format!("{prefix}?"))
    });
    if !allowed {
        return Err(AppError::BadRequest(format!(
            "{name} is outside BILLING_CHECKOUT_RETURN_TO_ALLOWED_PREFIXES"
        )));
    }
    Ok(())
}

fn parse_csv_env(name: &str) -> Vec<String> {
    env::var(name)
        .ok()
        .map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|entry| !entry.is_empty())
                .map(|entry| entry.trim_end_matches('/').to_owned())
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> CreateCheckoutSession {
        CreateCheckoutSession {
            tenant_id: Uuid::nil(),
            idempotency_key: "dfab-request-00000001".into(),
            external_reference: "dfab-job-00000001".into(),
            customer_email: Some("customer@example.com".into()),
            amount_minor: 15_000,
            currency: "USD".into(),
            description: "Engineering review deposit".into(),
            success_url: "https://fab.example.com/payment/success".into(),
            cancel_url: "https://fab.example.com/payment/cancel".into(),
            metadata: BTreeMap::from([("product".into(), "daedalus-fab".into())]),
        }
    }

    #[test]
    fn accepts_a_bounded_service_checkout_request() {
        request()
            .validate(&["https://fab.example.com".into()])
            .unwrap();
    }

    #[test]
    fn return_urls_are_exactly_allowlisted_and_https() {
        let mut value = request();
        value.success_url = "https://fab.example.com.evil.test/payment/success".into();
        assert!(value
            .validate(&["https://fab.example.com".into()])
            .is_err());

        let mut value = request();
        value.cancel_url = "http://fab.example.com/payment/cancel".into();
        assert!(value
            .validate(&["https://fab.example.com".into()])
            .is_err());
    }

    #[test]
    fn rejects_unbounded_amounts_and_metadata() {
        let mut value = request();
        value.amount_minor = 0;
        assert!(value
            .validate(&["https://fab.example.com".into()])
            .is_err());

        let mut value = request();
        value.metadata = (0..=MAX_METADATA_ENTRIES)
            .map(|index| (format!("key_{index}"), "value".into()))
            .collect();
        assert!(value
            .validate(&["https://fab.example.com".into()])
            .is_err());
    }
}