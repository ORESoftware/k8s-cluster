//! API documentation surface.
//!
//! Per the repo's API Docs Contract, deployments expose generated docs at
//! `/docs/api` + `/api/docs` and machine-readable metadata at
//! `/api/docs.json`. The route inventory below is the single source of truth
//! for both renderings (and mirrors what `generated/api-docs.json` ships).

use axum::response::{IntoResponse, Json};
use serde_json::json;

struct RouteDoc {
    method: &'static str,
    path: &'static str,
    auth: bool,
    summary: &'static str,
}

const ROUTES: &[RouteDoc] = &[
    RouteDoc { method: "GET", path: "/healthz", auth: false, summary: "Liveness probe (no dependencies)." },
    RouteDoc { method: "GET", path: "/readyz", auth: false, summary: "Readiness probe; checks Qdrant reachability." },
    RouteDoc { method: "GET", path: "/metrics", auth: false, summary: "Prometheus metrics (text exposition)." },
    RouteDoc { method: "GET", path: "/api/docs.json", auth: false, summary: "This document, as JSON." },
    RouteDoc { method: "GET", path: "/api/docs", auth: false, summary: "Human-readable API docs." },
    RouteDoc { method: "GET", path: "/api/providers", auth: true, summary: "List configured embedding + rerank providers, default models, and aliases." },
    RouteDoc { method: "POST", path: "/api/embeddings", auth: true, summary: "Generate embeddings. Body: { provider, input[], model?, dimensions?, input_type? }." },
    RouteDoc { method: "POST", path: "/api/rerank", auth: true, summary: "Rerank documents against a query. Body: { provider, query, documents[], model?, top_n? }." },
    RouteDoc { method: "POST", path: "/api/rag/index", auth: true, summary: "Embed documents and upsert into a Qdrant collection. Body: { collection, provider, documents[], model?, dimensions?, distance? }." },
    RouteDoc { method: "POST", path: "/api/rag/search", auth: true, summary: "Embed a query and retrieve nearest neighbors. Body: { collection, provider, query, top_k?, model?, dimensions? }." },
    RouteDoc { method: "POST", path: "/api/rag/delete", auth: true, summary: "Delete points by id from a collection. Body: { collection, ids[] }." },
    RouteDoc { method: "GET", path: "/api/rag/collections", auth: true, summary: "List vector-store collections." },
    RouteDoc { method: "DELETE", path: "/api/rag/collections/{collection}", auth: true, summary: "Delete an entire collection." },
];

fn doc_value() -> serde_json::Value {
    let routes: Vec<_> = ROUTES
        .iter()
        .map(|r| json!({ "method": r.method, "path": r.path, "auth": r.auth, "summary": r.summary }))
        .collect();
    json!({
        "service": "dd-embeddings-rs",
        "version": env!("CARGO_PKG_VERSION"),
        "description": "Multi-provider embedding gateway and RAG indexing service.",
        "notes": [
            "Provider `anthropic` is an alias for `voyage`: Anthropic has no \
             first-party embeddings API and recommends Voyage AI.",
            "Providers are opt-in: only those with credentials in the secret are registered."
        ],
        "routes": routes,
    })
}

pub async fn docs_json() -> impl IntoResponse {
    Json(doc_value())
}

pub fn docs_html_string() -> String {
    let mut rows = String::new();
    for r in ROUTES {
        let lock = if r.auth { " 🔒" } else { "" };
        rows.push_str(&format!(
            "<tr><td><code>{}</code></td><td><code>{}</code>{}</td><td>{}</td></tr>",
            r.method, r.path, lock, html_escape(r.summary)
        ));
    }
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\">\
         <title>dd-embeddings-rs API</title>\
         <style>body{{font-family:system-ui,sans-serif;max-width:60rem;margin:2rem auto;padding:0 1rem}}\
         table{{border-collapse:collapse;width:100%}}td,th{{border:1px solid #ddd;padding:.4rem .6rem;text-align:left}}\
         code{{background:#f4f1ea;padding:.1rem .3rem;border-radius:.2rem}}</style></head>\
         <body><h1>dd-embeddings-rs</h1>\
         <p>Multi-provider embedding gateway + RAG indexing service. \
         <code>🔒</code> routes require a bearer token when one is configured. \
         The <code>anthropic</code> provider id is an alias for <code>voyage</code> \
         (Anthropic ships no embeddings API).</p>\
         <table><thead><tr><th>Method</th><th>Path</th><th>Summary</th></tr></thead>\
         <tbody>{rows}</tbody></table>\
         <p>Machine-readable: <a href=\"/api/docs.json\"><code>/api/docs.json</code></a></p>\
         </body></html>"
    )
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}
