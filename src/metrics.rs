//! Direct Prometheus exposition for the Akrion web server.
//!
//! OpenTelemetry export remains enabled independently. This registry deliberately
//! excludes raw routes, game/session identifiers, and simulation values; only a
//! bounded HTTP method set and numeric response status are labels.

use std::{
    collections::BTreeMap,
    fmt::Write as _,
    sync::{
        atomic::{AtomicI64, Ordering},
        Mutex, OnceLock,
    },
    time::{Duration, Instant},
};

use axum::{
    body::Body,
    http::{header, Method, Request},
    middleware::Next,
    response::{IntoResponse, Response},
};

const CONTENT_TYPE: &str = "text/plain; version=0.0.4; charset=utf-8";
const PREFIX: &str = "akrion_web_server";

#[derive(Default)]
struct HttpAggregate {
    requests: u64,
    duration_seconds: f64,
}

struct Registry {
    started_at: Instant,
    active_requests: AtomicI64,
    http: Mutex<BTreeMap<(&'static str, u16), HttpAggregate>>,
}

fn registry() -> &'static Registry {
    static REGISTRY: OnceLock<Registry> = OnceLock::new();
    REGISTRY.get_or_init(|| Registry {
        started_at: Instant::now(),
        active_requests: AtomicI64::new(0),
        http: Mutex::new(BTreeMap::new()),
    })
}

pub(crate) async fn endpoint() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, CONTENT_TYPE)], render())
}

pub(crate) async fn record_http(request: Request<Body>, next: Next) -> Response {
    if request.uri().path() == "/metrics" {
        return next.run(request).await;
    }

    let method = bounded_method(request.method());
    let started_at = Instant::now();
    let registry = registry();
    registry.active_requests.fetch_add(1, Ordering::Relaxed);
    let response = next.run(request).await;
    registry.active_requests.fetch_sub(1, Ordering::Relaxed);
    record(method, response.status().as_u16(), started_at.elapsed());
    response
}

fn record(method: &'static str, status: u16, duration: Duration) {
    let mut http = registry()
        .http
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let aggregate = http.entry((method, status)).or_default();
    aggregate.requests = aggregate.requests.saturating_add(1);
    aggregate.duration_seconds += duration.as_secs_f64();
}

fn render() -> String {
    let registry = registry();
    let http = registry
        .http
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut output = String::with_capacity(2_048 + http.len() * 256);

    writeln!(
        output,
        "# HELP {PREFIX}_build_info Build and version information for the Akrion web server."
    )
    .expect("writing to String cannot fail");
    writeln!(output, "# TYPE {PREFIX}_build_info gauge").expect("writing to String cannot fail");
    writeln!(
        output,
        "{PREFIX}_build_info{{version=\"{}\"}} 1",
        escape_label(env!("CARGO_PKG_VERSION"))
    )
    .expect("writing to String cannot fail");

    writeln!(
        output,
        "# HELP {PREFIX}_process_up Whether the Akrion web process is serving metrics."
    )
    .expect("writing to String cannot fail");
    writeln!(output, "# TYPE {PREFIX}_process_up gauge").expect("writing to String cannot fail");
    writeln!(output, "{PREFIX}_process_up 1").expect("writing to String cannot fail");

    writeln!(
        output,
        "# HELP {PREFIX}_process_uptime_seconds Monotonic process uptime in seconds."
    )
    .expect("writing to String cannot fail");
    writeln!(output, "# TYPE {PREFIX}_process_uptime_seconds gauge")
        .expect("writing to String cannot fail");
    writeln!(
        output,
        "{PREFIX}_process_uptime_seconds {}",
        registry.started_at.elapsed().as_secs_f64()
    )
    .expect("writing to String cannot fail");

    writeln!(
        output,
        "# HELP {PREFIX}_http_active_requests HTTP requests currently executing."
    )
    .expect("writing to String cannot fail");
    writeln!(output, "# TYPE {PREFIX}_http_active_requests gauge")
        .expect("writing to String cannot fail");
    writeln!(
        output,
        "{PREFIX}_http_active_requests {}",
        registry.active_requests.load(Ordering::Relaxed)
    )
    .expect("writing to String cannot fail");

    writeln!(
        output,
        "# HELP {PREFIX}_http_requests_total Completed HTTP requests by bounded method and status."
    )
    .expect("writing to String cannot fail");
    writeln!(output, "# TYPE {PREFIX}_http_requests_total counter")
        .expect("writing to String cannot fail");
    writeln!(
        output,
        "# HELP {PREFIX}_http_request_duration_seconds Request duration summary by bounded method and status."
    )
    .expect("writing to String cannot fail");
    writeln!(
        output,
        "# TYPE {PREFIX}_http_request_duration_seconds summary"
    )
    .expect("writing to String cannot fail");

    for ((method, status), aggregate) in http.iter() {
        let labels = format!("method=\"{method}\",status=\"{status}\"");
        writeln!(
            output,
            "{PREFIX}_http_requests_total{{{labels}}} {}",
            aggregate.requests
        )
        .expect("writing to String cannot fail");
        writeln!(
            output,
            "{PREFIX}_http_request_duration_seconds_sum{{{labels}}} {}",
            aggregate.duration_seconds
        )
        .expect("writing to String cannot fail");
        writeln!(
            output,
            "{PREFIX}_http_request_duration_seconds_count{{{labels}}} {}",
            aggregate.requests
        )
        .expect("writing to String cannot fail");
    }

    output
}

fn bounded_method(method: &Method) -> &'static str {
    match *method {
        Method::GET => "GET",
        Method::POST => "POST",
        Method::PUT => "PUT",
        Method::PATCH => "PATCH",
        Method::DELETE => "DELETE",
        Method::HEAD => "HEAD",
        Method::OPTIONS => "OPTIONS",
        Method::CONNECT => "CONNECT",
        Method::TRACE => "TRACE",
        _ => "OTHER",
    }
}

fn escape_label(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\n', "\\n")
        .replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn endpoint_has_prometheus_content_type() {
        let response = endpoint().await.into_response();
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        assert_eq!(response.headers()[header::CONTENT_TYPE], CONTENT_TYPE);
    }

    #[test]
    fn exposition_declares_and_updates_bounded_http_metrics() {
        record("GET", 200, Duration::from_millis(50));
        let body = render();
        assert!(body.contains("# TYPE akrion_web_server_process_up gauge"));
        assert!(
            body.contains("akrion_web_server_http_requests_total{method=\"GET\",status=\"200\"}")
        );
        assert!(body.contains("# TYPE akrion_web_server_http_request_duration_seconds summary"));
        assert!(!body.contains("game_id"));
        assert!(!body.contains("session_id"));
    }
}
