//! Strict NATS request-intake adapter.
//!
//! `enqueue_build` returns an existing retained job for an identical
//! deterministic request and returns HTTP 409 only when the same request ID is
//! rebound to different execution inputs. NATS must terminate that conflicting
//! message rather than acknowledging it as a harmless duplicate.

use axum::http::StatusCode;

use crate::{
    jobs::{enqueue_build, submit_from_nats as submit_malformed_from_nats},
    state::AppState,
    types::{BuildRequest, NatsSubmitError},
};

pub(crate) async fn submit_from_nats(
    state: &AppState,
    payload: &[u8],
) -> Result<(), NatsSubmitError> {
    let request: BuildRequest = match serde_json::from_slice(payload) {
        Ok(request) => request,
        // Preserve the existing bounded parse error contract for malformed
        // documents. Valid documents take the strict mapping below.
        Err(_) => return submit_malformed_from_nats(state, payload).await,
    };

    map_enqueue_result(enqueue_build(state, request, "nats").await)
}

fn map_enqueue_result<T>(
    result: Result<T, (StatusCode, String)>,
) -> Result<(), NatsSubmitError> {
    match result {
        Ok(_) => Ok(()),
        Err((StatusCode::SERVICE_UNAVAILABLE, message)) => {
            Err(NatsSubmitError::Transient(message))
        }
        // Includes a deterministic request-ID/content conflict. JetStream must
        // terminate that payload instead of silently acknowledging it.
        Err((_, message)) => Err(NatsSubmitError::Invalid(message)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_or_new_acceptance_is_acknowledged() {
        assert!(map_enqueue_result::<()>(Ok(())).is_ok());
    }

    #[test]
    fn request_id_content_conflict_is_permanently_invalid() {
        let result = map_enqueue_result::<()>(Err((
            StatusCode::CONFLICT,
            "requestId is already bound to a different build request".to_string(),
        )));
        match result {
            Err(NatsSubmitError::Invalid(message)) => {
                assert!(message.contains("different build request"));
            }
            _ => panic!("request identity conflict must terminate the NATS message"),
        }
    }

    #[test]
    fn queue_backpressure_remains_retryable() {
        let result = map_enqueue_result::<()>(Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "build queue is full".to_string(),
        )));
        match result {
            Err(NatsSubmitError::Transient(message)) => {
                assert!(message.contains("queue is full"));
            }
            _ => panic!("queue backpressure must request redelivery"),
        }
    }

    #[test]
    fn validation_failures_are_permanently_invalid() {
        let result = map_enqueue_result::<()>(Err((
            StatusCode::BAD_REQUEST,
            "invalid build request".to_string(),
        )));
        assert!(matches!(result, Err(NatsSubmitError::Invalid(_))));
    }
}
