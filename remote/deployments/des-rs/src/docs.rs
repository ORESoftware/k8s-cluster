use axum::{
    extract::State,
    http::{header, HeaderMap, HeaderName, HeaderValue},
    response::{Html, IntoResponse, Response},
};

use des_engine::des::model::with_builtins;
use des_engine::des::service::{
    Capability, DesExtension, EndpointKind, EngineCatalogExtension, ServiceBuilder,
    ServiceDescriptor, ServiceInfo, DD_API_DOCS_HEADER,
};
use des_engine::des::streaming::streaming_contracts;

use crate::output::html_escape;
use crate::state::AppState;

// =============================================================================
// Service descriptor + API docs.
//
// The engine library owns the machine-readable contract (`ServiceDescriptor`,
// JSON-first). This server (a) builds that descriptor from its own routes plus
// engine/extension contributions, (b) serves it verbatim at /api/docs.json,
// and (c) renders its OWN HTML docs page as a *view* over the descriptor. One
// source of truth (the JSON), two representations; presentation stays a server
// concern. New servers that embed the engine reuse the same descriptor + the
// same discovery convention for free.
// =============================================================================

/// Advertises the engine's first-class model citizens and streaming solvers as
/// discoverable capabilities, so `/api/docs` lists `model:<kind>` /
/// `streaming:<name>` alongside the simulation catalogue.
struct ModelRegistryExtension;

impl DesExtension for ModelRegistryExtension {
    fn name(&self) -> &str {
        "des-model-registry"
    }
    fn version(&self) -> &str {
        env!("CARGO_PKG_VERSION")
    }
    fn capabilities(&self) -> Vec<Capability> {
        let mut caps: Vec<Capability> = with_builtins()
            .descriptors()
            .into_iter()
            .map(|d| Capability {
                name: format!("model:{}", d.kind),
                description: format!("{} — {} (schema {})", d.title, d.description, d.spec_schema),
                provided_by: "des-model-registry".to_string(),
            })
            .collect();
        for contract in streaming_contracts() {
            caps.push(Capability {
                name: format!("streaming:{}", contract.model),
                description: contract.description.clone(),
                provided_by: "des-model-registry".to_string(),
            });
        }
        caps
    }
}

/// Server-local extension demonstrating the engine's plugin seam: it advertises
/// the curated rendered-output site this server layers on top of the engine.
struct RenderedSiteExtension;

impl DesExtension for RenderedSiteExtension {
    fn name(&self) -> &str {
        "dd-des-rs-rendered-site"
    }
    fn version(&self) -> &str {
        env!("CARGO_PKG_VERSION")
    }
    fn capabilities(&self) -> Vec<Capability> {
        vec![Capability {
            name: "rendered-output-site".to_string(),
            description:
                "Curated HTML index of the artifacts simulations render, served under /out/."
                    .to_string(),
            provided_by: "dd-des-rs-rendered-site".to_string(),
        }]
    }
}

/// Build this service's descriptor: its own (host) endpoints, the engine's
/// simulation catalogue (as capabilities), and this server's own extension.
pub(crate) fn build_descriptor() -> ServiceDescriptor {
    let mut builder = ServiceBuilder::new(ServiceInfo {
        name: "dd-des-rs".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        description: "Runs the discrete-event-system.rs engine as a library and serves the \
                      HTML/JSON result pages its simulations render."
            .to_string(),
    });
    builder
        .endpoint(
            "GET",
            "/",
            "Interactive landing page with run buttons.",
            EndpointKind::Service,
        )
        .endpoint(
            "GET",
            "/info",
            "Service info, endpoint map, and discovery hints (JSON).",
            EndpointKind::Service,
        )
        .endpoint(
            "GET",
            "/healthz",
            "Readiness/liveness probe.",
            EndpointKind::Service,
        )
        .endpoint(
            "GET",
            "/simulations",
            "List the engine's simulation catalogue.",
            EndpointKind::Service,
        )
        .endpoint(
            "POST",
            "/simulate",
            "Run sims by `name` (filter, or exact with `\"exact\":true`), in series.",
            EndpointKind::Action,
        )
        .endpoint(
            "GET",
            "/simulations/:name/run",
            "Convenience GET form of /simulate (`?exact=1` for exact name).",
            EndpointKind::Action,
        )
        .endpoint(
            "GET",
            "/models",
            "First-class model registry with example specs.",
            EndpointKind::Service,
        )
        .endpoint(
            "GET",
            "/models/:kind/run",
            "Run a kind's example spec and render an interactive player (`?format=json` for the artifact).",
            EndpointKind::Action,
        )
        .endpoint(
            "POST",
            "/models/:kind/run",
            "Run a JSON model spec for a kind; renders a player (`?format=json` for the artifact).",
            EndpointKind::Action,
        )
        .endpoint(
            "GET",
            "/streaming",
            "List JSONL streaming-solver contracts (lp, milp/mip/ip, mdp, pomdp).",
            EndpointKind::Service,
        )
        .endpoint(
            "POST",
            "/streaming/:name",
            "Stream JSONL commands to a solver; responds with a JSONL frame stream.",
            EndpointKind::Action,
        )
        .endpoint(
            "GET",
            "/elevator-fel",
            "Next-event (FEL) single-car elevator under a LOOK policy, animated.",
            EndpointKind::Service,
        )
        .endpoint(
            "GET",
            "/elevator-mdp",
            "Elevator-dispatch MDP player (value-iterated drive-to-the-call policy).",
            EndpointKind::Service,
        )
        .endpoint(
            "GET",
            "/elevator-pomdp",
            "Elevator-dispatch POMDP player (noisy hall-call button; belief-tracked).",
            EndpointKind::Service,
        )
        .endpoint(
            "GET",
            "/out/soccer-sim.html",
            "Rendered 2D 11v11 soccer videogame / learning simulation artifact.",
            EndpointKind::Service,
        )
        .endpoint(
            "GET",
            "/soccer/live",
            "Live 2D 11v11 soccer UI with soft-real-time controls and live learning state.",
            EndpointKind::Service,
        )
        .endpoint(
            "GET/POST",
            "/api/state|step|reset|input/*|team-policy/*",
            "Live soccer bridge API used by /soccer/live.",
            EndpointKind::Action,
        )
        .endpoint(
            "GET",
            "/api/build",
            "Release identity (git commit + commit date + build timestamp) for the web server, soccer engine, and des engine.",
            EndpointKind::Service,
        )
        .endpoint(
            "GET",
            "/out/soccer-sim.meta.json",
            "Rendered soccer game metadata JSON with config, summary, events, and run metadata.",
            EndpointKind::Service,
        )
        .endpoint(
            "GET",
            "/out/soccer-sim.frames.jsonl",
            "Rendered soccer game JSONL stream with header, frame, event, and summary records.",
            EndpointKind::Service,
        )
        .endpoint(
            "GET",
            "/soccer/planner",
            "Interactive 11-a-side rotation planner (pitch + IP/MIP solver tabs).",
            EndpointKind::Service,
        )
        .endpoint(
            "POST",
            "/soccer/planner/solve",
            "Re-solve optimal rotation from roster/constraints JSON.",
            EndpointKind::Action,
        )
        .endpoint(
            "POST",
            "/soccer/planner/stream",
            "Stream planner edits and solve via the soccer-planner JSONL model.",
            EndpointKind::Action,
        )
        .endpoint(
            "GET",
            "/music",
            "Generative music production workbench for microtonal albums and MP4 sample seeds.",
            EndpointKind::Service,
        )
        .endpoint(
            "POST",
            "/music/sample-seed",
            "Upload a 10-50s MP4 seed or public/authenticated media link plus prompt text; renders a WAV variation and JSON manifest.",
            EndpointKind::Action,
        )
        .endpoint(
            "GET",
            "/delivery-planner.html",
            "Friendly redirect to the generated delivery planner artifact.",
            EndpointKind::Service,
        )
        .endpoint(
            "GET",
            "/deliver-planner.html",
            "Typo-compatible redirect to the generated delivery planner artifact.",
            EndpointKind::Service,
        )
        .endpoint(
            "GET",
            "/out/",
            "Curated index.html, else a listing of rendered artifacts.",
            EndpointKind::Service,
        )
        .endpoint(
            "GET",
            "/out/*path",
            "Serve an individual rendered artifact.",
            EndpointKind::Service,
        );
    // Built-in engine catalogue + this server's own plugin. Registration only
    // fails on a duplicate extension name, which would be a programming error.
    builder
        .register(Box::new(EngineCatalogExtension))
        .expect("engine catalogue extension registers cleanly");
    builder
        .register(Box::new(ModelRegistryExtension))
        .expect("model-registry extension registers cleanly");
    builder
        .register(Box::new(RenderedSiteExtension))
        .expect("rendered-site extension registers cleanly");
    builder.build()
}

/// Insert the discovery headers (computed once at startup) onto a response so a
/// machine that hits the canonical landing route can find the docs from headers
/// alone. Relative targets resolve correctly behind the gateway's `/des-rs/`.
pub(crate) fn apply_discovery_headers(headers: &mut HeaderMap, state: &AppState) {
    if let Ok(value) = HeaderValue::from_str(&state.link_header) {
        headers.insert(header::LINK, value);
    }
    if let Ok(value) = HeaderValue::from_str(&state.dd_docs_header) {
        headers.insert(HeaderName::from_static(DD_API_DOCS_HEADER), value);
    }
}

fn kind_label(kind: EndpointKind) -> &'static str {
    match kind {
        EndpointKind::Service => "service",
        EndpointKind::Docs => "docs",
        EndpointKind::Action => "action",
        EndpointKind::Custom => "custom",
    }
}

/// Independently render the HTML docs page from the JSON descriptor. The engine
/// library deliberately ships no HTML; this is the server's own branded view,
/// guaranteed consistent with `/api/docs.json` because both come from the same
/// [`ServiceDescriptor`]. The JSON link is `../api/docs.json` so it resolves
/// from `/docs/api` (and `/api/docs`) at the root or behind the gateway prefix.
pub(crate) fn render_docs_html(descriptor: &ServiceDescriptor) -> String {
    let endpoint_rows = descriptor
        .endpoints
        .iter()
        .map(|e| {
            let provided = e
                .provided_by
                .as_deref()
                .map(|p| format!("<span class=\"by\">{}</span>", html_escape(p)))
                .unwrap_or_default();
            format!(
                "<tr><td><span class=\"m\">{method}</span></td><td><code>{path}</code></td>\
                 <td><span class=\"k k-{kind}\">{kind}</span></td><td>{desc}{provided}</td></tr>",
                method = html_escape(&e.method),
                path = html_escape(&e.path),
                kind = kind_label(e.kind),
                desc = html_escape(&e.description),
            )
        })
        .collect::<String>();

    let capability_rows = descriptor
        .capabilities
        .iter()
        .map(|c| {
            format!(
                "<tr><td><code>{name}</code></td><td>{desc}</td>\
                 <td><span class=\"by\">{by}</span></td></tr>",
                name = html_escape(&c.name),
                desc = html_escape(&c.description),
                by = html_escape(&c.provided_by),
            )
        })
        .collect::<String>();

    let extension_rows = descriptor
        .extensions
        .iter()
        .map(|x| {
            format!(
                "<li><code>{name}</code> <span class=\"by\">v{version}</span> — \
                 {ep} endpoint(s), {cap} capability(ies)</li>",
                name = html_escape(&x.name),
                version = html_escape(&x.version),
                ep = x.endpoint_count,
                cap = x.capability_count,
            )
        })
        .collect::<String>();

    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <title>{name} API</title><style>\
         :root{{color-scheme:dark}}body{{font-family:system-ui,-apple-system,'Segoe UI',sans-serif;margin:0;background:#0b1021;color:#e6edf3}}\
         main{{max-width:1040px;margin:0 auto;padding:28px 22px 72px}}\
         h1{{margin:0 0 4px}}h2{{margin:30px 0 10px;font-size:1.05rem}}\
         p.sub{{color:#9aa4b2;margin:0 0 10px}}a{{color:#58a6ff}}\
         table{{border-collapse:collapse;width:100%;font-size:.88rem}}\
         td,th{{text-align:left;padding:8px 10px;border-bottom:1px solid #21262d;vertical-align:top}}\
         th{{color:#9aa4b2;font-size:.72rem;text-transform:uppercase;letter-spacing:.04em}}\
         code{{color:#58a6ff;font-family:ui-monospace,Menlo,Consolas,monospace}}\
         .m{{font-weight:700}}\
         .k{{font-size:.72rem;border:1px solid #2b3344;border-radius:5px;padding:1px 6px;white-space:nowrap}}\
         .k-service{{color:#7ee787}}.k-docs{{color:#d2a8ff}}.k-action{{color:#ffa657}}.k-custom{{color:#9aa4b2}}\
         .by{{color:#6e7781;font-size:.78rem;margin-left:6px}}\
         .pill{{display:inline-block;border:1px solid #2b3344;border-radius:6px;padding:2px 8px;margin:0 6px 8px 0;font-size:.8rem;text-decoration:none}}\
         </style></head><body><main>\
         <h1>{name} <span class=\"by\">v{version}</span></h1>\
         <p class=\"sub\">{description}</p>\
         <div><span class=\"pill\">schema {schema}</span>\
         <span class=\"pill\">{n_ep} endpoints</span>\
         <span class=\"pill\">{n_cap} capabilities</span>\
         <a class=\"pill\" href=\"../api/docs.json\">machine descriptor (JSON) &rarr;</a></div>\
         <h2>Endpoints</h2>\
         <table><tr><th>Method</th><th>Path</th><th>Kind</th><th>Description</th></tr>{endpoint_rows}</table>\
         <h2>Capabilities</h2>\
         <table><tr><th>Name</th><th>Description</th><th>Source</th></tr>{capability_rows}</table>\
         <h2>Extensions</h2><ul>{extension_rows}</ul>\
         </main></body></html>",
        name = html_escape(&descriptor.info.name),
        version = html_escape(&descriptor.info.version),
        description = html_escape(&descriptor.info.description),
        schema = html_escape(&descriptor.schema),
        n_ep = descriptor.endpoints.len(),
        n_cap = descriptor.capabilities.len(),
    )
}

pub(crate) async fn api_docs_html(State(state): State<AppState>) -> Html<String> {
    Html(state.docs_html.to_string())
}

pub(crate) async fn api_docs_json(State(state): State<AppState>) -> Response {
    let mut res = state.docs_json.to_string().into_response();
    res.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json; charset=utf-8"),
    );
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptor_advertises_model_and_streaming_endpoints() {
        let descriptor = build_descriptor();
        let paths: Vec<&str> = descriptor
            .endpoints
            .iter()
            .map(|e| e.path.as_str())
            .collect();
        assert!(paths.contains(&"/models"));
        assert!(paths.contains(&"/models/:kind/run"));
        assert!(paths.contains(&"/streaming"));
        assert!(paths.contains(&"/streaming/:name"));
        assert!(paths.contains(&"/soccer/planner"));
        assert!(paths.contains(&"/soccer/planner/solve"));
        assert!(paths.contains(&"/soccer/planner/stream"));
        assert!(paths.contains(&"/soccer/live"));
        assert!(paths.contains(&"/api/state|step|reset|input/*|team-policy/*"));
        assert!(paths.contains(&"/out/soccer-sim.html"));
        assert!(paths.contains(&"/out/soccer-sim.meta.json"));
        assert!(paths.contains(&"/out/soccer-sim.frames.jsonl"));
        // The model-registry extension contributes `model:<kind>` capabilities.
        assert!(descriptor
            .capabilities
            .iter()
            .any(|c| c.name == "model:mdp"));
        assert!(descriptor
            .capabilities
            .iter()
            .any(|c| c.name.starts_with("streaming:")));
    }
}
