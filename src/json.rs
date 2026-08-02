//! Request-body JSON extraction that answers the *documented* contract.
//!
//! `axum::Json`'s own rejection never passes through [`ApiError`]: a body that
//! parses as JSON but does not match the wire type answers **422** carrying
//! serde's message — "Failed to deserialize the JSON body into the target type:
//! blob: missing field `kdf_params` at line 1 column 182". That contradicts two
//! things this service promises:
//!
//!   * `3fa-clients/PROTOCOL.md` §8, whose status table documents **400
//!     "Malformed body"** and has no 422 row at all; and
//!   * `error.rs`'s "Body intentionally minimal" guarantee — the default
//!     rejection names internal fields of the wire type and the byte offset at
//!     which parsing stopped, which is a free schema-enumeration oracle.
//!
//! [`JsonBody`] wraps the standard extractor and collapses both the syntax and
//! the shape rejection to [`ApiError::BadRequest`]. Content-type and body-size
//! rejections keep their own statuses (415/413) — they are transport-level
//! answers that clients already handle and that carry no schema detail — but
//! their bodies are replaced with coarse strings too.

use crate::error::ApiError;
use axum::extract::rejection::JsonRejection;
use axum::extract::{FromRequest, Request};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::de::DeserializeOwned;

/// Drop-in replacement for `axum::Json` **as a request extractor**. Responses
/// keep using `axum::Json`, which has no such rejection problem.
#[derive(Debug, Clone, Copy, Default)]
pub struct JsonBody<T>(pub T);

impl<T, S> FromRequest<S> for JsonBody<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request(request: Request, state: &S) -> Result<Self, Self::Rejection> {
        match axum::Json::<T>::from_request(request, state).await {
            Ok(axum::Json(value)) => Ok(Self(value)),
            Err(rejection) => Err(coarse_rejection(rejection)),
        }
    }
}

fn coarse_rejection(rejection: JsonRejection) -> Response {
    match rejection {
        // Both "not JSON at all" and "JSON of the wrong shape" are one thing to
        // a caller: the body was malformed. 400, per PROTOCOL.md §8.
        JsonRejection::JsonSyntaxError(_) | JsonRejection::JsonDataError(_) => {
            ApiError::BadRequest.into_response()
        }
        other => {
            let status = other.status();
            let body = match status {
                StatusCode::UNSUPPORTED_MEDIA_TYPE => "unsupported media type",
                StatusCode::PAYLOAD_TOO_LARGE => "payload too large",
                // JsonRejection is #[non_exhaustive]; anything new lands here
                // and is reported as the coarse client error it is.
                _ => "bad request",
            };
            (status, body).into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::header;
    use axum::routing::post;
    use axum::Router;
    use http_body_util::BodyExt;
    use serde::Deserialize;
    use tower::ServiceExt;

    #[derive(Deserialize)]
    struct Wire {
        // A distinctive field name: the assertions below prove it never reaches
        // a client, so schema enumeration through error bodies is impossible.
        kdf_params: u32,
    }

    async fn handler(JsonBody(wire): JsonBody<Wire>) -> String {
        wire.kdf_params.to_string()
    }

    async fn post_body(content_type: Option<&str>, body: &'static str) -> (StatusCode, String) {
        let app = Router::new().route("/", post(handler));
        let mut request = Request::builder().method("POST").uri("/");
        if let Some(content_type) = content_type {
            request = request.header(header::CONTENT_TYPE, content_type);
        }
        let response = app
            .oneshot(request.body(Body::from(body)).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let body = response.into_body().collect().await.unwrap().to_bytes();
        (status, String::from_utf8(body.to_vec()).unwrap())
    }

    #[tokio::test]
    async fn a_well_formed_body_is_extracted_unchanged() {
        let (status, body) = post_body(Some("application/json"), r#"{"kdf_params":7}"#).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "7");
    }

    #[tokio::test]
    async fn malformed_and_wrongly_shaped_bodies_are_a_coarse_400() {
        for body in [
            "not json at all",
            "{unclosed:",
            "",
            // Parses, but is not the target type — axum's own default for this
            // is a 422 naming `kdf_params`.
            r#"{"other":1}"#,
            r#"{"kdf_params":"a string"}"#,
            "[]",
        ] {
            let (status, response) = post_body(Some("application/json"), body).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "status for {body:?}");
            assert_eq!(response, "bad request", "body for {body:?}");
            assert!(
                !response.contains("kdf_params"),
                "the rejection must not name internal wire-type fields: {response}"
            );
            assert!(
                !response.contains("column") && !response.contains("line"),
                "nor serde's offset detail: {response}"
            );
        }
    }

    #[tokio::test]
    async fn a_missing_json_content_type_stays_a_415() {
        // Clients rely on the 415/400 split; only the body is made coarse.
        let (status, body) = post_body(None, r#"{"kdf_params":7}"#).await;
        assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
        assert_eq!(body, "unsupported media type");
    }
}
