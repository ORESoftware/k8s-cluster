//! Integration tests for the runtime admin surface (`/admin/log-level`,
//! `/admin/tcp`, `/admin/otel`) and the HTMX-driven status-page UI.
//!
//! Each test boots a fresh broker on an ephemeral loopback port and
//! drives the HTTP layer with a tiny in-test HTTP/1.1 client (no
//! reqwest dependency — we want to control the raw bytes we send so
//! we can assert on the `HX-Request: true` dual-output, mixed JSON /
//! `application/x-www-form-urlencoded` content types, and the
//! `Content-Type` of the response).
//!
//! The default admin shared secret in `src/server.rs::admin_token`
//! is the literal `"all-dogs-go-to-heaven"`; we deliberately don't
//! override `LMX_ADMIN_TOKEN` here because env mutation is
//! process-global and would race with the other tests in this
//! binary.

use std::sync::Once;
use std::time::Duration;

use dd_rust_network_mutex::{server, BrokerConfig, ServerConfig};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::Mutex;

const ADMIN_TOKEN: &str = "all-dogs-go-to-heaven";

static INIT: Once = Once::new();

/// Serialises the log-level tests in this binary. The reloadable
/// `EnvFilter` handle and the `current_log_level` snapshot are
/// process-global, so two `#[tokio::test]` functions racing on
/// `set_log_level` would clobber each other's "previous" /
/// "current" assertions. Each broker's `Arc<TcpFlags>` is
/// per-broker so the TCP tests don't need this.
fn log_level_lock() -> &'static Mutex<()> {
    static LOCK: std::sync::OnceLock<Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// Install the reloadable tracing subscriber once per test binary so
/// `/admin/log-level` POSTs actually find a handle to modify. The
/// underlying `init_tracing` is itself idempotent (it uses
/// `try_init`), but this `Once` keeps the test output tidy by
/// eliminating any "global subscriber already set" debug noise.
fn init_tracing_once() {
    INIT.call_once(|| {
        dd_rust_network_mutex::init_tracing();
    });
}

async fn pick_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

/// Boot a broker bound to ephemeral TCP/HTTP ports on loopback. The
/// HTTP port is what we drive `/admin/*` against; the TCP port is
/// returned for tests that want to probe the wire-protocol path
/// after a runtime toggle.
async fn start_broker() -> (u16, u16) {
    init_tracing_once();
    let tcp_port = pick_port().await;
    let http_port = pick_port().await;
    let cfg = ServerConfig {
        tcp_bind: Some(format!("127.0.0.1:{tcp_port}").parse().unwrap()),
        uds_path: None,
        http_bind: Some(format!("127.0.0.1:{http_port}").parse().unwrap()),
        auth_token: None,
        broker: BrokerConfig::default(),
        tcp_nodelay: true,
        tcp_quickack: true,
        status_bind: None,
        #[cfg(feature = "tls")]
        tls: None,
    };
    tokio::spawn(async move {
        let _ = server::run(cfg).await;
    });
    // Tiny grace period for the listeners to bind.
    tokio::time::sleep(Duration::from_millis(75)).await;
    (tcp_port, http_port)
}

/// Minimal HTTP/1.1 client. Returns `(status_code, headers_block,
/// body)`. We deliberately don't use a fancy client crate — the goal
/// is precise control over headers (esp. `HX-Request`) and parsing
/// `Content-Type` plus body.
async fn http_request(
    method: &str,
    port: u16,
    path: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> (u16, String, String) {
    let mut req = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\nContent-Length: {len}\r\n",
        len = body.len(),
    );
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str("\r\n");
    let mut sock = tokio::net::TcpStream::connect(format!("127.0.0.1:{port}"))
        .await
        .unwrap();
    sock.write_all(req.as_bytes()).await.unwrap();
    if !body.is_empty() {
        sock.write_all(body).await.unwrap();
    }
    sock.flush().await.unwrap();
    let mut buf = Vec::new();
    sock.read_to_end(&mut buf).await.unwrap();
    let raw = String::from_utf8_lossy(&buf).to_string();
    let (head, body) = raw
        .split_once("\r\n\r\n")
        .map(|(h, b)| (h.to_string(), b.to_string()))
        .unwrap_or_else(|| (raw.clone(), String::new()));
    let status_line = head.lines().next().unwrap_or("");
    let status_code = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(0);
    (status_code, head, body)
}

fn header_value<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    let lower = name.to_ascii_lowercase();
    for line in head.lines() {
        if let Some((k, v)) = line.split_once(':') {
            if k.trim().eq_ignore_ascii_case(&lower) {
                return Some(v.trim());
            }
        }
    }
    None
}

fn auth_header() -> [(&'static str, &'static str); 1] {
    [("X-Admin-Token", ADMIN_TOKEN)]
}

#[tokio::test]
async fn admin_log_level_round_trip() {
    let _g = log_level_lock().lock().await;
    let (_tcp, http) = start_broker().await;

    // GET without a token: 401.
    let (status, _head, _body) = http_request("GET", http, "/admin/log-level", &[], b"").await;
    assert_eq!(status, 401, "unauth GET should be rejected");

    // GET with token: 200, returns current directive.
    let (status, _head, body) =
        http_request("GET", http, "/admin/log-level", &auth_header(), b"").await;
    assert_eq!(status, 200);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    let initial = v["directive"].as_str().unwrap().to_string();
    assert!(!initial.is_empty(), "initial directive should not be empty");

    // POST a new directive (`debug`) — JSON.
    let (status, _head, body) = http_request(
        "POST",
        http,
        "/admin/log-level",
        &[
            ("X-Admin-Token", ADMIN_TOKEN),
            ("Content-Type", "application/json"),
        ],
        br#"{"directive":"debug"}"#,
    )
    .await;
    assert_eq!(status, 200, "POST debug should succeed: {body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["directive"], "debug");
    assert_eq!(v["previous"], serde_json::Value::String(initial));

    // GET again: directive sticks.
    let (status, _head, body) =
        http_request("GET", http, "/admin/log-level", &auth_header(), b"").await;
    assert_eq!(status, 200);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["directive"], "debug");

    // POST garbage: 400 + JSON error.
    let (status, _head, body) = http_request(
        "POST",
        http,
        "/admin/log-level",
        &[
            ("X-Admin-Token", ADMIN_TOKEN),
            ("Content-Type", "application/json"),
        ],
        br#"{"directive":"=== nonsense ==="}"#,
    )
    .await;
    assert_eq!(status, 400, "garbage directive should 400: {body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert!(v["error"].is_string());

    // Missing field: 400 (still JSON).
    let (status, _head, _body) = http_request(
        "POST",
        http,
        "/admin/log-level",
        &[
            ("X-Admin-Token", ADMIN_TOKEN),
            ("Content-Type", "application/json"),
        ],
        br#"{}"#,
    )
    .await;
    assert_eq!(status, 400);
}

#[tokio::test]
async fn admin_log_level_returns_html_for_htmx_request() {
    let _g = log_level_lock().lock().await;
    let (_tcp, http) = start_broker().await;
    let (status, head, body) = http_request(
        "POST",
        http,
        "/admin/log-level",
        &[
            ("X-Admin-Token", ADMIN_TOKEN),
            ("Content-Type", "application/x-www-form-urlencoded"),
            ("HX-Request", "true"),
        ],
        b"directive=info",
    )
    .await;
    assert_eq!(status, 200, "expected 200, got {status}: {body}");
    let ct = header_value(&head, "Content-Type").unwrap_or("");
    assert!(
        ct.starts_with("text/html"),
        "HX-Request response must be HTML, got `{ct}`",
    );
    assert!(body.contains("log-level:"), "snippet body: {body}");
    assert!(body.contains("info"), "snippet body: {body}");
}

#[tokio::test]
async fn admin_log_level_still_returns_json_without_htmx() {
    let _g = log_level_lock().lock().await;
    let (_tcp, http) = start_broker().await;
    let (status, head, body) = http_request(
        "POST",
        http,
        "/admin/log-level",
        &[
            ("X-Admin-Token", ADMIN_TOKEN),
            ("Content-Type", "application/json"),
        ],
        br#"{"directive":"info"}"#,
    )
    .await;
    assert_eq!(status, 200);
    let ct = header_value(&head, "Content-Type").unwrap_or("");
    assert!(
        ct.starts_with("application/json"),
        "expected JSON, got `{ct}`"
    );
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["directive"], "info");
}

#[tokio::test]
async fn admin_tcp_round_trip() {
    let (_tcp, http) = start_broker().await;

    // GET with token returns the live state.
    let (status, _head, body) = http_request("GET", http, "/admin/tcp", &auth_header(), b"").await;
    assert_eq!(status, 200);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert!(v["nodelay"].is_boolean());
    assert!(v["quickack"].is_boolean());
    assert_eq!(v["quickack_supported"], cfg!(target_os = "linux"));

    // POST `nodelay: false` (JSON).
    let (status, _head, body) = http_request(
        "POST",
        http,
        "/admin/tcp",
        &[
            ("X-Admin-Token", ADMIN_TOKEN),
            ("Content-Type", "application/json"),
        ],
        br#"{"nodelay":false}"#,
    )
    .await;
    assert_eq!(status, 200, "POST tcp should succeed: {body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["nodelay"], false);

    // GET reflects the flip.
    let (_status, _head, body) = http_request("GET", http, "/admin/tcp", &auth_header(), b"").await;
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["nodelay"], false);

    // Empty body: 400.
    let (status, _head, _body) = http_request(
        "POST",
        http,
        "/admin/tcp",
        &[
            ("X-Admin-Token", ADMIN_TOKEN),
            ("Content-Type", "application/json"),
        ],
        b"{}",
    )
    .await;
    assert_eq!(status, 400);

    // 401 without token.
    let (status, _head, _body) = http_request("GET", http, "/admin/tcp", &[], b"").await;
    assert_eq!(status, 401);
}

#[tokio::test]
async fn admin_tcp_accepts_form_urlencoded() {
    // HTMX (without the optional json-enc extension) submits its
    // `hx-vals` payload as form-urlencoded. We must accept that as
    // well as the canonical JSON shape.
    let (_tcp, http) = start_broker().await;
    let (status, _head, body) = http_request(
        "POST",
        http,
        "/admin/tcp",
        &[
            ("X-Admin-Token", ADMIN_TOKEN),
            ("Content-Type", "application/x-www-form-urlencoded"),
        ],
        b"quickack=true",
    )
    .await;
    assert_eq!(status, 200, "form post should succeed: {body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["quickack"], true);
}

#[tokio::test]
async fn admin_tcp_returns_html_for_htmx_request() {
    let (_tcp, http) = start_broker().await;
    let (status, head, body) = http_request(
        "POST",
        http,
        "/admin/tcp",
        &[
            ("X-Admin-Token", ADMIN_TOKEN),
            ("Content-Type", "application/x-www-form-urlencoded"),
            ("HX-Request", "true"),
        ],
        b"nodelay=true",
    )
    .await;
    assert_eq!(status, 200);
    let ct = header_value(&head, "Content-Type").unwrap_or("");
    assert!(ct.starts_with("text/html"), "expected HTML, got `{ct}`");
    assert!(body.contains("tcp:"), "snippet body: {body}");
    assert!(body.contains("NODELAY"), "snippet body: {body}");
}

#[tokio::test]
async fn admin_tcp_still_returns_json_without_htmx() {
    let (_tcp, http) = start_broker().await;
    let (status, head, body) = http_request(
        "POST",
        http,
        "/admin/tcp",
        &[
            ("X-Admin-Token", ADMIN_TOKEN),
            ("Content-Type", "application/json"),
        ],
        br#"{"nodelay":true}"#,
    )
    .await;
    assert_eq!(status, 200);
    let ct = header_value(&head, "Content-Type").unwrap_or("");
    assert!(
        ct.starts_with("application/json"),
        "expected JSON, got `{ct}`"
    );
    assert!(serde_json::from_str::<serde_json::Value>(&body).is_ok());
}

#[tokio::test]
async fn admin_otel_returns_html_for_htmx_request() {
    let (_tcp, http) = start_broker().await;
    let (status, head, body) = http_request(
        "POST",
        http,
        "/admin/otel",
        &[
            ("X-Admin-Token", ADMIN_TOKEN),
            ("Content-Type", "application/x-www-form-urlencoded"),
            ("HX-Request", "true"),
        ],
        b"enabled=true",
    )
    .await;
    assert_eq!(status, 200, "expected 200, got {status}: {body}");
    let ct = header_value(&head, "Content-Type").unwrap_or("");
    assert!(ct.starts_with("text/html"), "expected HTML, got `{ct}`");
    assert!(body.contains("otel:"), "snippet body: {body}");
    assert!(
        body.contains("on") || body.contains("off"),
        "snippet body: {body}",
    );
}

#[tokio::test]
async fn admin_otel_still_returns_json_without_htmx() {
    let (_tcp, http) = start_broker().await;
    let (status, head, body) = http_request(
        "POST",
        http,
        "/admin/otel",
        &[
            ("X-Admin-Token", ADMIN_TOKEN),
            ("Content-Type", "application/json"),
        ],
        br#"{"enabled":false}"#,
    )
    .await;
    assert_eq!(status, 200);
    let ct = header_value(&head, "Content-Type").unwrap_or("");
    assert!(
        ct.starts_with("application/json"),
        "expected JSON, got `{ct}`"
    );
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert!(v["enabled"].is_boolean());
}

#[tokio::test]
async fn status_page_loads_htmx_from_cdn() {
    let (_tcp, http) = start_broker().await;
    let (status, _head, body) = http_request("GET", http, "/", &[], b"").await;
    assert_eq!(status, 200);
    assert!(
        body.contains("https://unpkg.com/htmx.org@2.0.4/dist/htmx.min.js"),
        "status page must load HTMX from the public CDN",
    );
    assert!(
        body.contains(
            "integrity=\"sha384-HGfztofotfshcF7+8n44JQL2oJmowVChPTg48S+jvZoztPfvwD79OC/LTtG6dMp+\""
        ),
        "status page must include the SRI hash",
    );
}

#[tokio::test]
async fn status_page_uses_relative_admin_paths() {
    let (_tcp, http) = start_broker().await;
    let (status, _head, body) = http_request("GET", http, "/status", &[], b"").await;
    assert_eq!(status, 200);
    // All `hx-post` URLs must be relative — a leading slash would
    // break the page when served behind a gateway prefix.
    assert!(body.contains("hx-post=\"admin/otel\""));
    assert!(body.contains("hx-post=\"admin/log-level\""));
    assert!(body.contains("hx-post=\"admin/tcp\""));
    assert!(
        !body.contains("hx-post=\"/admin/"),
        "no leading-slash absolute admin paths allowed",
    );
    assert!(
        !body.contains("hx-get=\"/admin/"),
        "no leading-slash absolute admin paths allowed",
    );
    assert!(body.contains("id=\"lmx-admin\""));
    assert!(body.contains("'lmx-admin-token'"));
}
