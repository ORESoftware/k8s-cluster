use std::sync::atomic::Ordering;

use axum::http::StatusCode;
use serde_json::Value;

use crate::{
    error::{ApiError, ApiResult},
    state::AppState,
};

pub async fn validate_envelope(state: &AppState, envelope: &Value) -> ApiResult<Value> {
    proxy_json(state, "/validate", envelope).await
}

pub async fn simulate_transaction(state: &AppState, payload: &Value) -> ApiResult<Value> {
    proxy_json(state, "/simulate", payload).await
}

pub fn digest_from_contract_response(value: &Value) -> Option<String> {
    value
        .get("digest")
        .and_then(Value::as_str)
        .or_else(|| {
            value
                .get("validation")
                .and_then(|validation| validation.get("digest"))
                .and_then(Value::as_str)
        })
        .map(ToString::to_string)
}

async fn proxy_json(state: &AppState, path: &str, payload: &Value) -> ApiResult<Value> {
    state
        .metrics
        .contract_requests_total
        .fetch_add(1, Ordering::Relaxed);

    let url = format!(
        "{}{}",
        state.config.contract_service_url.trim_end_matches('/'),
        path
    );
    let response = state
        .http
        .post(url)
        .json(payload)
        .send()
        .await
        .map_err(|error| {
            state
                .metrics
                .contract_errors_total
                .fetch_add(1, Ordering::Relaxed);
            ApiError::from(error)
        })?;

    let status = response.status();
    let body = response.json::<Value>().await.map_err(|error| {
        state
            .metrics
            .contract_errors_total
            .fetch_add(1, Ordering::Relaxed);
        ApiError::from(error)
    })?;

    if status.is_success() {
        Ok(body)
    } else {
        state
            .metrics
            .contract_errors_total
            .fetch_add(1, Ordering::Relaxed);
        Err(ApiError::new(
            StatusCode::BAD_GATEWAY,
            format!("contract service returned HTTP {status}: {body}"),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn digest_read_from_top_level() {
        let value = json!({ "digest": "0xabc123" });
        assert_eq!(
            digest_from_contract_response(&value),
            Some("0xabc123".to_string())
        );
    }

    #[test]
    fn digest_falls_back_to_nested_validation_digest() {
        let value = json!({ "validation": { "digest": "0xnested" } });
        assert_eq!(
            digest_from_contract_response(&value),
            Some("0xnested".to_string())
        );
    }

    #[test]
    fn top_level_digest_wins_over_nested() {
        let value = json!({
            "digest": "0xtop",
            "validation": { "digest": "0xnested" }
        });
        assert_eq!(
            digest_from_contract_response(&value),
            Some("0xtop".to_string())
        );
    }

    #[test]
    fn missing_or_non_string_digest_yields_none() {
        assert_eq!(digest_from_contract_response(&json!({})), None);
        assert_eq!(digest_from_contract_response(&json!({ "digest": 42 })), None);
        assert_eq!(
            digest_from_contract_response(&json!({ "validation": { "digest": null } })),
            None
        );
        assert_eq!(digest_from_contract_response(&json!("just a string")), None);
    }
}
