use std::{
    fs,
    path::{Path as StdPath, PathBuf},
};

use axum::{
    body::Body,
    extract::{Path, State},
    http::{header, HeaderMap, HeaderName, HeaderValue, StatusCode},
    response::{Html, IntoResponse, Redirect, Response},
};
use tokio_util::io::ReaderStream;

use soccer_engine::soccer::try_write_soccer_playback_artifacts;

use crate::sims::artifact_ext;
use crate::state::AppState;

// =============================================================================
// Rendered-output serving (HTML / JSON / SVG / PNG / JSONL / CSV …).
//
// The artifacts live in `state.out_dir` (a writable working dir the engine
// renders into). Requests are confined to that directory via canonicalized
// path checks so `..` / symlinks cannot escape it.
// =============================================================================

fn content_type(path: &StdPath) -> &'static str {
    match path.extension().and_then(|ext| ext.to_str()) {
        Some("html" | "htm") => "text/html; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json" | "map") => "application/json; charset=utf-8",
        Some("jsonl") => "application/x-ndjson; charset=utf-8",
        Some("csv") => "text/csv; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("ico") => "image/x-icon",
        Some("woff2") => "font/woff2",
        Some("wasm") => "application/wasm",
        Some("md") => "text/markdown; charset=utf-8",
        Some("txt") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

/// Canonicalize `requested` and return it only if it stays inside the
/// canonicalized `base`. `None` for traversal, escaping symlinks, or missing
/// paths.
fn resolve_within(base: &StdPath, requested: &StdPath) -> Option<PathBuf> {
    let canon_base = base.canonicalize().ok()?;
    let canon_req = requested.canonicalize().ok()?;
    canon_req.starts_with(&canon_base).then_some(canon_req)
}

fn path_to_output_rel(base: &StdPath, target: &StdPath) -> String {
    target
        .strip_prefix(base)
        .unwrap_or(target)
        .to_string_lossy()
        .replace('\\', "/")
}

fn output_index_href(from_rel: &str) -> String {
    let depth = from_rel.split('/').filter(|part| !part.is_empty()).count();
    let parent_depth = depth.saturating_sub(1);
    if parent_depth == 0 {
        "./".to_string()
    } else {
        "../".repeat(parent_depth)
    }
}

fn relative_output_href(from_rel: &str, to_rel: &str) -> String {
    let mut from_parts: Vec<&str> = from_rel
        .split('/')
        .filter(|part| !part.is_empty())
        .collect();
    if !from_parts.is_empty() {
        from_parts.pop();
    }
    let to_parts: Vec<&str> = to_rel.split('/').filter(|part| !part.is_empty()).collect();
    let mut common = 0;
    while common < from_parts.len()
        && common < to_parts.len()
        && from_parts[common] == to_parts[common]
    {
        common += 1;
    }
    let mut parts: Vec<String> = Vec::new();
    for _ in common..from_parts.len() {
        parts.push("..".to_string());
    }
    for part in &to_parts[common..] {
        parts.push((*part).to_string());
    }
    if parts.is_empty() {
        "./".to_string()
    } else {
        parts.join("/")
    }
}

fn related_data_artifacts(base: &StdPath, current_rel: &str) -> Vec<String> {
    let current = StdPath::new(current_rel);
    if artifact_ext(current_rel) != Some("html") || current_rel == "index.html" {
        return Vec::new();
    }
    if current_rel == "soccer-sim.html" {
        return vec![
            SOCCER_SIM_META_JSON.to_string(),
            SOCCER_SIM_FRAMES_JSONL.to_string(),
        ];
    }
    let Some(stem) = current.file_stem().and_then(|s| s.to_str()) else {
        return Vec::new();
    };
    let parent = current
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(|p| p.to_path_buf());
    let dir = parent
        .as_ref()
        .map(|p| base.join(p))
        .unwrap_or_else(|| base.to_path_buf());
    let Ok(entries) = fs::read_dir(&dir) else {
        return Vec::new();
    };

    let mut exact = Vec::new();
    let mut fallback = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let ext = path.extension().and_then(|e| e.to_str());
        if !matches!(ext, Some("json" | "jsonl")) {
            continue;
        }
        let Some(file_name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let rel = parent
            .as_ref()
            .map(|p| p.join(file_name))
            .unwrap_or_else(|| PathBuf::from(file_name))
            .to_string_lossy()
            .replace('\\', "/");
        if file_name.starts_with(stem) {
            exact.push(rel);
        } else if parent.is_some() {
            fallback.push(rel);
        }
    }
    exact.sort();
    fallback.sort();
    if exact.is_empty() {
        fallback
    } else {
        exact
    }
}

fn output_toolbar_html(base: &StdPath, current_rel: &str) -> String {
    if artifact_ext(current_rel) != Some("html") || current_rel == "index.html" {
        return String::new();
    }
    let mut links = vec![format!(
        "<a href=\"{}\">Output index</a>",
        html_escape(&output_index_href(current_rel))
    )];
    for rel in related_data_artifacts(base, current_rel) {
        let label = match artifact_ext(&rel) {
            Some("jsonl") => "JSONL",
            Some("json") => "JSON",
            _ => "Artifact",
        };
        links.push(format!(
            "<a href=\"{}\" target=\"_blank\" rel=\"noopener\">{}</a>",
            html_escape(&relative_output_href(current_rel, &rel)),
            label
        ));
    }
    format!(
        "<style>\
         .dd-des-artifacts{{position:fixed;right:16px;bottom:16px;z-index:2147483647;\
         display:flex;gap:8px;flex-wrap:wrap;align-items:center;padding:8px;\
         border:1px solid rgba(139,148,158,.35);border-radius:8px;\
         background:rgba(13,17,23,.94);box-shadow:0 8px 28px rgba(0,0,0,.35);\
         font:13px system-ui,-apple-system,Segoe UI,sans-serif}}\
         .dd-des-artifacts a{{color:#e6edf3;text-decoration:none;border:1px solid #30363d;\
         border-radius:7px;padding:6px 9px;background:#161b22}}\
         .dd-des-artifacts a:hover{{border-color:#58a6ff;color:#fff}}\
         </style><nav class=\"dd-des-artifacts\" aria-label=\"Result artifacts\">{}</nav>",
        links.join("")
    )
}

fn rfind_ascii_case_insensitive(haystack: &str, needle: &str) -> Option<usize> {
    haystack
        .as_bytes()
        .windows(needle.len())
        .rposition(|window| window.eq_ignore_ascii_case(needle.as_bytes()))
}

fn inject_before_body(mut html: String, fragment: &str) -> String {
    if let Some(idx) = rfind_ascii_case_insensitive(&html, "</body>") {
        html.insert_str(idx, fragment);
    } else {
        html.push_str(fragment);
    }
    html
}

fn apply_output_headers(headers: &mut HeaderMap, path: &StdPath, content_length: Option<u64>) {
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(content_type(path)),
    );
    headers.insert(
        HeaderName::from_static("x-content-type-options"),
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=30"),
    );
    if let Some(len) = content_length {
        if let Ok(value) = HeaderValue::from_str(&len.to_string()) {
            headers.insert(header::CONTENT_LENGTH, value);
        }
    }
}

async fn serve_output_file(base: &StdPath, rel_path: &str, path: &StdPath) -> Response {
    if content_type(path).starts_with("text/html") {
        return match tokio::fs::read_to_string(path).await {
            Ok(html) => {
                let body = inject_before_body(html, &output_toolbar_html(base, rel_path));
                let len = body.len() as u64;
                let mut res = Body::from(body).into_response();
                apply_output_headers(res.headers_mut(), path, Some(len));
                res
            }
            Err(_) => (StatusCode::NOT_FOUND, "not found").into_response(),
        };
    }

    match tokio::fs::File::open(path).await {
        Ok(file) => {
            let len = file.metadata().await.ok().map(|m| m.len());
            let stream = ReaderStream::new(file);
            let mut res = Body::from_stream(stream).into_response();
            apply_output_headers(res.headers_mut(), path, len);
            res
        }
        Err(_) => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

const LISTED_EXTENSIONS: [&str; 6] = ["html", "json", "csv", "jsonl", "svg", "png"];

/// Recursively collect servable artifacts under `dir`, returned as
/// forward-slash relative paths sorted alphabetically for a stable listing.
pub(crate) fn collect_artifacts(dir: &StdPath, base: &StdPath, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_artifacts(&path, base, out);
        } else if path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| LISTED_EXTENSIONS.contains(&e))
            .unwrap_or(false)
        {
            if let Ok(rel) = path.strip_prefix(base) {
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
}

pub(crate) fn html_escape(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

pub(crate) async fn out_redirect() -> Response {
    Redirect::permanent("out/").into_response()
}

pub(crate) async fn delivery_planner_redirect() -> Response {
    Redirect::temporary("out/delivery-planner.html").into_response()
}

pub(crate) async fn out_index(State(state): State<AppState>) -> Response {
    let base: &StdPath = state.out_dir.as_path();

    // Always render a live listing of the current artifacts so simulations run
    // on demand (POST /simulate after startup) are immediately discoverable.
    // The curated build_site landing (out/index.html) is a startup-time
    // snapshot — it does not list sims run afterward — so we surface it as a
    // link at the top instead of serving it verbatim as the directory index.
    let mut files = Vec::new();
    collect_artifacts(base, base, &mut files);
    files.sort();

    let has_curated = files.iter().any(|f| f == "index.html");
    let artifacts: Vec<&String> = files
        .iter()
        .filter(|f| f.as_str() != "index.html")
        .collect();

    let mut header = String::new();
    if has_curated {
        header.push_str(
            "<p class=\"curated\"><a href=\"index.html\">Curated overview &rarr;</a> \
             <span class=\"hint\">(startup snapshot)</span></p>",
        );
    }

    let mut items = String::new();
    if artifacts.is_empty() {
        items.push_str(
            "<p class=\"empty\">No artifacts yet. Run a simulation, e.g. \
             <code>curl -X POST :PORT/simulate -H 'content-type: application/json' \
             -d '{\"name\":\"electric_circuit\"}'</code> or \
             <code>GET /simulations/build_site/run</code>.</p>",
        );
    } else {
        items.push_str("<ul>");
        for file in &artifacts {
            let safe = html_escape(file);
            items.push_str(&format!(
                "<li><a href=\"{href}\">{label}</a></li>",
                href = safe,
                label = safe
            ));
        }
        items.push_str("</ul>");
    }

    let body = format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <title>discrete-event-system.rs output</title><style>\
         body{{font-family:system-ui,-apple-system,Segoe UI,Roboto,sans-serif;margin:0;\
         background:#0d1117;color:#e6edf3;}}\
         main{{max-width:960px;margin:0 auto;padding:24px 20px 64px;}}\
         h1{{font-size:1.5rem;margin:0 0 4px;}}\
         p.sub{{color:#8b949e;margin:0 0 16px;font-size:.9rem;}}\
         p.curated{{margin:0 0 18px;}}\
         p.curated a{{color:#58a6ff;text-decoration:none;font-weight:600;}}\
         p.curated a:hover{{text-decoration:underline;}}\
         p.curated .hint{{color:#8b949e;font-size:.8rem;}}\
         code{{background:#161b22;padding:1px 5px;border-radius:4px;}}\
         ul{{list-style:none;padding:0;margin:0;}}\
         li{{border-bottom:1px solid #21262d;}}\
         li a{{display:block;padding:10px 8px;color:#58a6ff;text-decoration:none;\
         font-family:ui-monospace,SFMono-Regular,Menlo,monospace;font-size:.9rem;}}\
         li a:hover{{background:#161b22;}}\
         p.empty{{color:#8b949e;padding:16px 8px;}}</style></head><body><main>\
         <h1>discrete-event-system.rs output</h1>\
         <p class=\"sub\">Artifacts rendered by the Rust DES engine ({count} files, live listing).</p>\
         {header}{items}</main></body></html>",
        count = artifacts.len(),
        header = header,
        items = items
    );

    Html(body).into_response()
}

pub(crate) const SOCCER_SIM_META_JSON: &str = "soccer-sim.meta.json";
pub(crate) const SOCCER_SIM_FRAMES_JSONL: &str = "soccer-sim.frames.jsonl";

async fn ensure_soccer_playback_artifacts(state: &AppState) -> Result<(), String> {
    let html_path = state.out_dir.join("soccer-sim.html");
    let meta_path = state.out_dir.join(SOCCER_SIM_META_JSON);
    let frames_path = state.out_dir.join(SOCCER_SIM_FRAMES_JSONL);
    if html_path.is_file() && meta_path.is_file() && frames_path.is_file() {
        return Ok(());
    }

    let _guard = state.sim_lock.lock().await;
    if html_path.is_file() && meta_path.is_file() && frames_path.is_file() {
        return Ok(());
    }

    std::fs::create_dir_all(state.out_dir.as_path())
        .map_err(|e| format!("create output dir: {e}"))?;
    try_write_soccer_playback_artifacts().map_err(|e| format!("write soccer playback: {e}"))?;
    Ok(())
}

pub(crate) async fn out_file(State(state): State<AppState>, Path(rel_path): Path<String>) -> Response {
    if matches!(
        rel_path.as_str(),
        "soccer-sim.html" | SOCCER_SIM_META_JSON | SOCCER_SIM_FRAMES_JSONL
    ) {
        if let Err(err) = ensure_soccer_playback_artifacts(&state).await {
            tracing::error!("[dd-des-rs] soccer playback render failed: {err}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "soccer playback render failed",
            )
                .into_response();
        }
    }

    let base: &StdPath = state.out_dir.as_path();

    let Some(target) = resolve_within(base, &base.join(&rel_path)) else {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    };

    if target.is_dir() {
        if let Some(index) = resolve_within(base, &target.join("index.html")) {
            if index.is_file() {
                let rel = path_to_output_rel(base, &index);
                return serve_output_file(base, &rel, &index).await;
            }
        }
        return (StatusCode::NOT_FOUND, "not found").into_response();
    }

    if !target.is_file() {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    }

    let rel = path_to_output_rel(base, &target);
    serve_output_file(base, &rel, &target).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::now_ms;

    #[test]
    fn content_type_maps_known_and_unknown_extensions() {
        assert_eq!(
            content_type(StdPath::new("a/b.html")),
            "text/html; charset=utf-8"
        );
        assert_eq!(content_type(StdPath::new("a.svg")), "image/svg+xml");
        assert_eq!(
            content_type(StdPath::new("a.json")),
            "application/json; charset=utf-8"
        );
        assert_eq!(
            content_type(StdPath::new("a.frames.jsonl")),
            "application/x-ndjson; charset=utf-8"
        );
        assert_eq!(
            content_type(StdPath::new("a.bin")),
            "application/octet-stream"
        );
    }

    #[test]
    fn output_toolbar_links_are_relative_to_the_current_page() {
        assert_eq!(
            relative_output_href("shadow-eval/report.html", "shadow-eval/report.json"),
            "report.json"
        );
        assert_eq!(
            relative_output_href("shadow-eval/report.html", "two-disease.frames.jsonl"),
            "../two-disease.frames.jsonl"
        );
        assert_eq!(output_index_href("shadow-eval/report.html"), "../");
        assert_eq!(output_index_href("two-disease.html"), "./");
    }

    #[test]
    fn related_data_artifacts_find_sibling_json_and_jsonl() {
        let root = std::env::temp_dir().join(format!(
            "des-rs-artifact-links-{}-{}",
            std::process::id(),
            now_ms()
        ));
        let base = root.join("out");
        let dir = base.join("shadow-eval");
        std::fs::create_dir_all(&dir).expect("create output dir");
        std::fs::write(dir.join("report.html"), b"<html><body>report</body></html>")
            .expect("write html");
        std::fs::write(dir.join("report.json"), b"{}").expect("write json");
        std::fs::write(dir.join("report.frames.jsonl"), b"{}\n").expect("write jsonl");
        std::fs::write(dir.join("other.json"), b"{}").expect("write unrelated json");

        let rels = related_data_artifacts(&base, "shadow-eval/report.html");
        assert_eq!(
            rels,
            vec![
                "shadow-eval/report.frames.jsonl".to_string(),
                "shadow-eval/report.json".to_string()
            ]
        );

        let toolbar = output_toolbar_html(&base, "shadow-eval/report.html");
        assert!(toolbar.contains("href=\"../\""));
        assert!(toolbar.contains("href=\"report.json\""));
        assert!(toolbar.contains("href=\"report.frames.jsonl\""));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn soccer_toolbar_links_lazy_trace_artifacts() {
        assert_eq!(
            related_data_artifacts(StdPath::new("/unused"), "soccer-sim.html"),
            vec![
                SOCCER_SIM_META_JSON.to_string(),
                SOCCER_SIM_FRAMES_JSONL.to_string()
            ]
        );
    }

    #[test]
    fn resolve_within_confines_to_base_and_blocks_traversal() {
        let root =
            std::env::temp_dir().join(format!("des-rs-test-{}-{}", std::process::id(), now_ms()));
        let base = root.join("out");
        std::fs::create_dir_all(base.join("sub")).expect("create base");
        std::fs::write(base.join("index.html"), b"<h1>ok</h1>").expect("write index");
        std::fs::write(base.join("sub/page.html"), b"<h1>sub</h1>").expect("write sub");
        std::fs::write(root.join("secret.txt"), b"secret").expect("write secret");

        assert!(resolve_within(&base, &base.join("index.html")).is_some());
        assert!(resolve_within(&base, &base.join("sub/page.html")).is_some());
        assert!(resolve_within(&base, &base.join("../secret.txt")).is_none());
        assert!(resolve_within(&base, &base.join("nope.html")).is_none());

        let _ = std::fs::remove_dir_all(&root);
    }
}
