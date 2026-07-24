use axum::{
    extract::Path,
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};

use crate::metrics::record_request;

fn canonical_grafana_deployment_name(deployment: &str) -> Option<String> {
    let value = deployment.trim();
    if value.is_empty() || value.len() > 128 {
        return None;
    }
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return None;
    }

    Some(
        match value {
            "billing-server" => "dd-billing-server",
            "dart-server" => "dd-dart-server",
            "des-rs" => "dd-des-rs",
            other => other,
        }
        .to_string(),
    )
}

pub(crate) fn grafana_deployment_path(deployment: &str) -> String {
    format!("/grafana/depl/{deployment}")
}

fn grafana_deployment_dashboard_path(deployment: &str) -> String {
    format!("/telemetry/d/dd-deployment-drilldown/deployment-drilldown?orgId=1&var-deployment={deployment}")
}

pub(crate) async fn grafana_observability_redirect() -> Response {
    record_request("GET", "/grafana/observability", StatusCode::FOUND);
    let mut response = Response::new(axum::body::Body::empty());
    *response.status_mut() = StatusCode::FOUND;
    response.headers_mut().insert(
        header::LOCATION,
        HeaderValue::from_static(
            "/telemetry/d/dd-observability-control-plane/observability-control-plane?orgId=1",
        ),
    );
    response
}

pub(crate) async fn grafana_fabrication_redirect() -> Response {
    record_request("GET", "/grafana/fabrication", StatusCode::FOUND);
    let mut response = Response::new(axum::body::Body::empty());
    *response.status_mut() = StatusCode::FOUND;
    response.headers_mut().insert(
        header::LOCATION,
        HeaderValue::from_static("/telemetry/d/dd-fabrication-planner/fabrication-planner?orgId=1"),
    );
    response
}

pub(crate) async fn grafana_deployment_redirect(Path(deployment): Path<String>) -> Response {
    match canonical_grafana_deployment_name(&deployment) {
        Some(deployment) => {
            record_request("GET", "/grafana/depl/{deployment}", StatusCode::FOUND);
            let location = grafana_deployment_dashboard_path(&deployment);
            let mut response = Response::new(axum::body::Body::empty());
            *response.status_mut() = StatusCode::FOUND;
            if let Ok(value) = HeaderValue::from_str(&location) {
                response.headers_mut().insert(header::LOCATION, value);
                response
            } else {
                record_request(
                    "GET",
                    "/grafana/depl/{deployment}",
                    StatusCode::INTERNAL_SERVER_ERROR,
                );
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to build Grafana deployment URL",
                )
                    .into_response()
            }
        }
        None => {
            record_request("GET", "/grafana/depl/{deployment}", StatusCode::BAD_REQUEST);
            (
                StatusCode::BAD_REQUEST,
                "deployment must be a Kubernetes-safe name",
            )
                .into_response()
        }
    }
}
