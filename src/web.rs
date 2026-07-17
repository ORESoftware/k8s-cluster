//! Web dashboard + docs server (client mode).
//!
//! Runs alongside the SOCKS5 listener and exposes, on `TOR_UI_LISTEN`:
//!   GET /                 dashboard UI
//!   GET /api/status       JSON: config + live circuit counters
//!   GET /api/fetch?url=   build a fresh onion circuit and HTTP-GET the URL
//!   GET /docs             index of the markdown docs
//!   GET /docs/{name}      one rendered markdown doc
//!   GET /proxy.pac        browser proxy auto-config pointing at the SOCKS port
//!   GET /healthz          liveness probe
//!
//! The `/api/fetch` endpoint proves end-to-end reachability from a browser and
//! reveals the exit's public IP. It handles plaintext HTTP only; for HTTPS,
//! point the browser/curl at the SOCKS proxy (which tunnels TLS end to end).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use subtle::ConstantTimeEq;

use anyhow::{bail, Result};
use axum::extract::{Path as AxPath, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Json, Response};
use axum::routing::get;
use axum::Router;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::time::{timeout, Duration};
use tracing::info;

use crate::config::Directory;
use crate::connector::Connector;
use crate::stats::Stats;

/// Cap on the response body we buffer for the in-browser fetch preview.
const FETCH_MAX_BODY: usize = 512 * 1024;
const FETCH_TIMEOUT: Duration = Duration::from_secs(25);

pub struct WebConfig {
    pub ui_listen: String,
    pub socks_listen: String,
    pub connector: Arc<Connector>,
    /// Present only for the overlay backend; used to display the relay list.
    pub directory: Option<Directory>,
    pub hops: usize,
    pub docs_dir: PathBuf,
    pub stats: Arc<Stats>,
    /// If set, the `/api/fetch` proxy primitive requires this token (via
    /// `?token=` or `Authorization: Bearer`). Guards the dashboard when it is
    /// bound to a non-loopback address.
    pub ui_token: Option<String>,
}

type AppState = Arc<WebConfig>;

pub async fn run(cfg: Arc<WebConfig>) -> Result<()> {
    let app = Router::new()
        .route("/", get(index))
        .route("/api/status", get(status))
        .route("/api/fetch", get(fetch))
        .route("/docs", get(docs_index))
        .route("/docs/{name}", get(docs_page))
        .route("/proxy.pac", get(proxy_pac))
        .route("/healthz", get(|| async { "ok" }))
        .with_state(cfg.clone());

    let listener = TcpListener::bind(&cfg.ui_listen).await?;
    info!(listen = %cfg.ui_listen, "web dashboard listening");
    axum::serve(listener, app.into_make_service()).await?;
    return Ok(());
}

async fn index() -> Html<&'static str> {
    return Html(INDEX_HTML);
}

async fn status(State(cfg): State<AppState>) -> Json<serde_json::Value> {
    let s = cfg.stats.snapshot();
    let relays: Vec<serde_json::Value> = cfg
        .directory
        .as_ref()
        .map(|d| {
            d.relays
                .iter()
                .map(|r| serde_json::json!({ "name": r.name, "addr": r.addr }))
                .collect()
        })
        .unwrap_or_default();
    return Json(serde_json::json!({
        "backend": cfg.connector.backend(),
        "socks_listen": cfg.socks_listen,
        "hops": cfg.hops,
        "relay_count": relays.len(),
        "relays": relays,
        "circuits_built": s.circuits_built,
        "circuits_failed": s.circuits_failed,
        "circuits_active": s.circuits_active,
    }));
}

async fn fetch(
    State(cfg): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> Json<serde_json::Value> {
    if !authorized(&cfg, &headers, &q) {
        return Json(serde_json::json!({
            "ok": false,
            "error": "unauthorized: /api/fetch requires the TOR_UI_TOKEN (pass ?token= or Authorization: Bearer)"
        }));
    }
    let url = match q.get("url") {
        Some(u) if !u.is_empty() => u.clone(),
        _ => {
            return Json(serde_json::json!({ "ok": false, "error": "missing ?url= parameter" }));
        }
    };
    let started = Instant::now();
    match onion_get(&cfg, &url).await {
        Ok(mut v) => {
            v["elapsed_ms"] = serde_json::json!(started.elapsed().as_millis() as u64);
            Json(v)
        }
        Err(e) => {
            cfg.stats.failed();
            Json(serde_json::json!({
                "ok": false,
                "url": url,
                "error": format!("{e:#}"),
                "elapsed_ms": started.elapsed().as_millis() as u64,
            }))
        }
    }
}

/// Whether a request may use the `/api/fetch` proxy primitive. Open when no
/// `TOR_UI_TOKEN` is configured; otherwise the token must match (checked in
/// constant time).
fn authorized(cfg: &WebConfig, headers: &HeaderMap, q: &HashMap<String, String>) -> bool {
    let expected = match &cfg.ui_token {
        None => return true,
        Some(t) => t,
    };
    let provided = q.get("token").map(|s| s.as_str()).or_else(|| {
        headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.strip_prefix("Bearer "))
    });
    return match provided {
        Some(p) => ct_str_eq(p, expected),
        None => false,
    };
}

/// Constant-time string comparison (length-independent short-circuit avoided by
/// folding, but differing lengths return false immediately — the token length
/// is not itself secret).
fn ct_str_eq(a: &str, b: &str) -> bool {
    return bool::from(a.as_bytes().ct_eq(b.as_bytes()));
}

/// Open a stream through the active backend and perform one plaintext HTTP GET.
async fn onion_get(cfg: &WebConfig, url: &str) -> Result<serde_json::Value> {
    let (host, port, path) = parse_http_url(url)?;

    let mut stream = cfg.connector.connect(&host, port).await?;
    cfg.stats.built();

    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: {host}\r\nUser-Agent: tor-server-ui/0.1\r\nAccept: */*\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(request.as_bytes()).await?;

    let raw = timeout(FETCH_TIMEOUT, read_all_capped(&mut stream, FETCH_MAX_BODY))
        .await
        .map_err(|_| anyhow::anyhow!("fetch timed out after {}s", FETCH_TIMEOUT.as_secs()))??;

    if raw.is_empty() {
        bail!("no response from exit — destination unreachable, refused, or blocked by the exit policy");
    }

    let (status_line, status_code, headers, body) = split_http_response(&raw);
    let content_type = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
        .map(|(_, v)| v.clone())
        .unwrap_or_default();
    let is_text = content_type.is_empty()
        || content_type.starts_with("text/")
        || content_type.contains("json")
        || content_type.contains("xml")
        || content_type.contains("javascript");
    let body_preview = if is_text {
        String::from_utf8_lossy(&body)
            .chars()
            .take(200_000)
            .collect::<String>()
    } else {
        format!(
            "[{} bytes of {}]",
            body.len(),
            if content_type.is_empty() {
                "binary data"
            } else {
                &content_type
            }
        )
    };

    return Ok(serde_json::json!({
        "ok": true,
        "url": url,
        "target": format!("{host}:{port}"),
        "backend": cfg.connector.backend(),
        "status_line": status_line,
        "status_code": status_code,
        "headers": headers.iter().map(|(k, v)| format!("{k}: {v}")).collect::<Vec<_>>(),
        "content_type": content_type,
        "is_text": is_text,
        "body_len": body.len(),
        "body_preview": body_preview,
    }));
}

async fn read_all_capped<R>(r: &mut R, cap: usize) -> Result<Vec<u8>>
where
    R: AsyncReadExt + Unpin,
{
    let mut out = Vec::new();
    let mut buf = [0u8; 16 * 1024];
    loop {
        let n = r.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        out.extend_from_slice(&buf[..n]);
        if out.len() >= cap {
            break;
        }
    }
    return Ok(out);
}

/// Parse `http://host[:port]/path`. HTTPS is intentionally rejected here.
fn parse_http_url(url: &str) -> Result<(String, u16, String)> {
    let rest = if let Some(r) = url.strip_prefix("http://") {
        r
    } else if url.starts_with("https://") {
        bail!("the built-in fetch supports http:// only; for https point your browser/curl at the SOCKS proxy");
    } else {
        // Assume bare host means http.
        url
    };
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    if authority.is_empty() {
        bail!("could not parse host from URL");
    }
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) => (
            h.to_string(),
            p.parse().map_err(|_| anyhow::anyhow!("bad port"))?,
        ),
        None => (authority.to_string(), 80u16),
    };
    let path = if path.is_empty() {
        "/".to_string()
    } else {
        path.to_string()
    };

    // The host and path are interpolated into a raw HTTP request; any control
    // character (notably CR/LF) would allow header injection / request
    // smuggling to the target through the proxy. A space in the path would
    // break the request line. Reject all of them.
    if host.is_empty() || host.bytes().any(|b| b.is_ascii_control() || b == b' ') {
        bail!("invalid host in URL");
    }
    if path.bytes().any(|b| b.is_ascii_control() || b == b' ') {
        bail!("invalid characters in URL path (control or space)");
    }
    return Ok((host, port, path));
}

fn split_http_response(raw: &[u8]) -> (String, u16, Vec<(String, String)>, Vec<u8>) {
    let sep = b"\r\n\r\n";
    let split_at = raw
        .windows(4)
        .position(|w| w == sep)
        .map(|i| i + 4)
        .unwrap_or(raw.len());
    let head = String::from_utf8_lossy(&raw[..split_at.min(raw.len())]);
    let body = if split_at <= raw.len() {
        raw[split_at..].to_vec()
    } else {
        Vec::new()
    };

    let mut lines = head.lines();
    let status_line = lines.next().unwrap_or("").trim().to_string();
    let status_code = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| {
            l.split_once(':')
                .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        })
        .collect();
    return (status_line, status_code, headers, body);
}

async fn docs_index(State(cfg): State<AppState>) -> Response {
    let mut names = list_docs(&cfg.docs_dir);
    names.sort();
    let items: String = names
        .iter()
        .map(|n| format!("<li><a href=\"/docs/{n}\">{n}</a></li>"))
        .collect();
    let body = format!(
        "{}<h1>Documentation</h1><ul class=\"doclist\">{}</ul><p><a href=\"/\">&larr; Dashboard</a></p>{}",
        DOC_HEADER,
        if items.is_empty() { "<li><em>no docs found</em></li>".into() } else { items },
        DOC_FOOTER
    );
    return Html(body).into_response();
}

async fn docs_page(State(cfg): State<AppState>, AxPath(name): AxPath<String>) -> Response {
    // Sanitize: only a bare doc slug, no path traversal.
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return (StatusCode::BAD_REQUEST, "invalid doc name").into_response();
    }
    let file = cfg.docs_dir.join(format!("{name}.md"));
    let md = match std::fs::read_to_string(&file) {
        Ok(s) => s,
        Err(_) => return (StatusCode::NOT_FOUND, "doc not found").into_response(),
    };
    let mut html = String::new();
    let parser = pulldown_cmark::Parser::new(&md);
    pulldown_cmark::html::push_html(&mut html, parser);
    let body = format!(
        "{}<p class=\"nav\"><a href=\"/docs\">&larr; All docs</a> · <a href=\"/\">Dashboard</a></p><article>{}</article>{}",
        DOC_HEADER, html, DOC_FOOTER
    );
    return Html(body).into_response();
}

fn list_docs(dir: &std::path::Path) -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            let p = e.path();
            if p.extension().and_then(|s| s.to_str()) == Some("md") {
                if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                    out.push(stem.to_string());
                }
            }
        }
    }
    return out;
}

async fn proxy_pac(State(cfg): State<AppState>) -> Response {
    // The client's SOCKS listener may bind 0.0.0.0; browsers need a real host.
    let hostport = cfg.socks_listen.replace("0.0.0.0", "127.0.0.1");
    let pac = format!(
        "function FindProxyForURL(url, host) {{\n  return \"SOCKS5 {hostport}; SOCKS {hostport}\";\n}}\n"
    );
    return (
        [(header::CONTENT_TYPE, "application/x-ns-proxy-autoconfig")],
        pac,
    )
        .into_response();
}

const DOC_HEADER: &str = r#"<!doctype html><html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>tor-server docs</title><style>
body{background:#0d1117;color:#c9d1d9;font:16px/1.6 -apple-system,Segoe UI,Roboto,sans-serif;max-width:820px;margin:0 auto;padding:2rem}
a{color:#58a6ff}h1,h2,h3{color:#e6edf3}code{background:#161b22;padding:.15em .4em;border-radius:4px}
pre{background:#161b22;padding:1rem;border-radius:8px;overflow:auto}pre code{background:none;padding:0}
table{border-collapse:collapse}th,td{border:1px solid #30363d;padding:.4rem .7rem}.doclist li{margin:.3rem 0}.nav{color:#8b949e}
</style></head><body>"#;
const DOC_FOOTER: &str = "</body></html>";

const INDEX_HTML: &str = include_str!("web/index.html");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_http_url_accepts_valid() {
        let (h, p, path) = parse_http_url("http://example.com/foo?bar=1").unwrap();
        assert_eq!(h, "example.com");
        assert_eq!(p, 80);
        assert_eq!(path, "/foo?bar=1");
        let (h, p, _) = parse_http_url("http://example.com:8080/").unwrap();
        assert_eq!((h.as_str(), p), ("example.com", 8080));
    }

    #[test]
    fn parse_http_url_rejects_crlf_injection() {
        // Percent-decoding happens in the query extractor, so by the time we
        // parse, a CRLF is a real control character and must be rejected.
        assert!(parse_http_url("http://example.com/x\r\nX-Injected: 1").is_err());
        assert!(parse_http_url("http://exa\r\nmple.com/").is_err());
        assert!(parse_http_url("http://example.com/a b").is_err()); // space breaks request line
        assert!(parse_http_url("https://example.com/").is_err()); // https not supported here
    }

    #[test]
    fn ct_str_eq_matches_and_differs() {
        assert!(ct_str_eq("s3cret", "s3cret"));
        assert!(!ct_str_eq("s3cret", "s3creT"));
        assert!(!ct_str_eq("s3cret", "s3cret-longer"));
        assert!(!ct_str_eq("", "x"));
    }
}
