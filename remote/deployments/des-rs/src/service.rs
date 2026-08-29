use axum::{
    extract::State,
    response::{Html, IntoResponse, Response},
    Json,
};
use serde_json::json;

use des_engine::des::model::with_builtins;
use des_engine::des::service::DD_API_DOCS_HEADER;
use des_engine::des::streaming::streaming_model_names;

use crate::docs::apply_discovery_headers;
use crate::pages::LANDING_HTML;
use crate::sims::sim_names;
use crate::soccer::full_stack_build_json;
use crate::state::{now_ms, AppState};

// =============================================================================
// JSON / control routes
// =============================================================================

pub(crate) async fn healthz() -> impl IntoResponse {
    Json(json!({ "ok": true, "service": "dd-des-rs", "atMs": now_ms() }))
}

/// Human-facing landing page: featured + full catalogue with "Run" buttons
/// (each does a relative `fetch` so it works at `/` locally and behind the
/// gateway at `/des-rs/`), plus a link to the rendered `out/` results. The
/// canonical landing route also carries the discovery headers, so a machine
/// that hits only `/` learns where the docs live.
pub(crate) async fn root(State(state): State<AppState>) -> Response {
    let mut res = Html(LANDING_HTML).into_response();
    apply_discovery_headers(res.headers_mut(), &state);
    res
}

/// Machine-readable service info (the old JSON root), including the discovery
/// hints that are also returned as HTTP response headers.
pub(crate) async fn info(State(state): State<AppState>) -> Response {
    let mut res = Json(json!({
        "ok": true,
        "service": "dd-des-rs",
        "mode": "runs the discrete-event-system.rs engine (library) and serves rendered HTML",
        "engineSimulations": sim_names().len(),
        "modelKinds": with_builtins().kinds(),
        "streamingSolvers": streaming_model_names(),
        "build": full_stack_build_json(),
        "endpoints": {
            "build": "GET /api/build  (release identity: git commit + timestamps for web server, soccer engine, des engine)",
            "landing": "GET /",
            "healthz": "GET /healthz",
            "simulations": "GET /simulations",
            "simulate": "POST /simulate  {\"name\":\"<filter>\",\"exact\":false}",
            "runNamed": "GET /simulations/:name/run?exact=1",
            "models": "GET /models",
            "runModel": "GET /models/:kind/run  (POST a JSON spec to run your own; ?format=json for the artifact)",
            "streaming": "GET /streaming",
            "streamModel": "POST /streaming/:name  (JSONL in -> JSONL out)",
            "elevatorFel": "GET /elevator-fel  (new next-event elevator sim, animated)",
            "soccerVideogame": "GET /out/soccer-sim.html  (2D 11v11 soccer videogame / learning sim artifact)",
            "soccerLive": "GET /soccer/live  (live 2D 11v11 soccer UI with soft-real-time controls)",
            "soccerLiveApi": "GET/POST /api/state, /api/step, /api/reset, /api/input/*, /api/team-policy/*  (live soccer bridge)",
            "soccerVideogameMetadata": "GET /out/soccer-sim.meta.json  (config, summary, events, run metadata)",
            "soccerVideogameFrames": "GET /out/soccer-sim.frames.jsonl  (streamed frame records)",
            "soccerPlanner": "GET /soccer/planner  (11-a-side rotation planner UI)",
            "soccerPlannerSolve": "POST /soccer/planner/solve  (re-solve with constraints)",
            "soccerPlannerStream": "POST /soccer/planner/stream  (planner JSONL command stream)",
            "musicProduction": "GET /music  (microtonal music-production workbench UI)",
            "musicSampleSeed": "POST /music/sample-seed  (multipart sample=<10-50s mp4> or source_url, optional auth headers/cookies, prompt, duration_seconds -> WAV)",
            "deliveryPlanner": "GET /delivery-planner.html  (redirects to out/delivery-planner.html)",
            "deliverPlannerAlias": "GET /deliver-planner.html  (typo-compatible redirect)",
            "elevatorMdp": "GET /elevator-mdp  (elevator-dispatch MDP player)",
            "elevatorPomdp": "GET /elevator-pomdp  (elevator-dispatch POMDP player)",
            "renderedOutputIndex": "GET /out/",
            "renderedOutputFile": "GET /out/*path",
            "apiDocs": "GET /docs/api",
            "apiDocsJson": "GET /api/docs.json"
        },
        "discovery": {
            "linkHeader": &*state.link_header,
            "ddHeader": DD_API_DOCS_HEADER,
            "ddHeaderValue": &*state.dd_docs_header,
            "note": "GET / and GET /info also return these as HTTP response headers (RFC 8288 Link with service-doc/service-desc relations); relative targets resolve under the gateway prefix."
        },
        "atMs": now_ms()
    }))
    .into_response();
    apply_discovery_headers(res.headers_mut(), &state);
    res
}
