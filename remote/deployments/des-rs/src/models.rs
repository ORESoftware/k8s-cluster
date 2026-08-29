use std::panic::{catch_unwind, AssertUnwindSafe};

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{Html, IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};

use des_engine::des::model::{with_builtins, CitizenError};
use des_engine::des::streaming::{run_named_jsonl, streaming_contracts, streaming_model_names};

use crate::state::AppState;

// =============================================================================
// First-class models + streaming solvers.
//
// These expose the platform's "describe a model as JSON → run → interactive
// player" loop directly over HTTP, alongside the simulation catalogue.
// `with_builtins()` registers zero-sized citizens, so a fresh registry per
// request is cheap; runs are serialized behind `sim_lock` and panic-isolated on
// a blocking thread, exactly like the simulations, since the engine drives
// process-global state.
// =============================================================================

#[derive(Debug, Deserialize)]
pub(crate) struct FormatQuery {
    format: Option<String>,
}

fn wants_json(query: &FormatQuery) -> bool {
    matches!(query.format.as_deref(), Some("json"))
}

fn unknown_model_response(kind: &str) -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(json!({
            "ok": false,
            "error": format!("unknown model kind `{kind}`"),
            "models": with_builtins().kinds(),
        })),
    )
        .into_response()
}

/// `GET /models` — the model-citizen registry: every kind's descriptor (title,
/// schema, solve methods, and a runnable example spec the UI/LLM can target).
pub(crate) async fn list_models() -> impl IntoResponse {
    let descriptors = with_builtins().descriptors();
    Json(json!({
        "ok": true,
        "count": descriptors.len(),
        "models": descriptors,
        "note": "Run a model: GET models/<kind>/run renders its example spec as an interactive player; POST models/<kind>/run with a JSON spec runs your own (add ?format=json for the raw artifact).",
    }))
}

/// `GET /streaming` — the JSONL streaming-solver contracts (lp, milp/mip/ip,
/// mdp, pomdp): each is an iterative solver fed a JSONL
/// command stream.
pub(crate) async fn list_streaming() -> impl IntoResponse {
    let contracts = streaming_contracts();
    Json(json!({
        "ok": true,
        "count": contracts.len(),
        "streaming": contracts,
        "note": "POST streaming/<name> with a JSONL body (one command per line); the response is a JSONL stream of result frames.",
    }))
}

/// Validate, run, and render (or JSON-encode) a model spec. Serialized behind
/// the simulation lock and panic-isolated on a blocking thread.
async fn run_model_spec(state: &AppState, kind: String, spec: Value, as_json: bool) -> Response {
    let _guard = state.sim_lock.lock().await;
    let kind_for_run = kind.clone();
    let outcome = tokio::task::spawn_blocking(move || {
        catch_unwind(AssertUnwindSafe(|| {
            with_builtins().run(&kind_for_run, &spec)
        }))
    })
    .await;

    match outcome {
        Ok(Ok(Ok(artifact))) => {
            if as_json {
                Json(json!({
                    "ok": true,
                    "kind": artifact.kind,
                    "title": artifact.title,
                    "description": artifact.description,
                    "summary": artifact.summary,
                    "frameCount": artifact.frames.len(),
                    "results": artifact.results,
                }))
                .into_response()
            } else {
                Html(artifact.to_player_html()).into_response()
            }
        }
        Ok(Ok(Err(CitizenError::UnknownKind(k)))) => unknown_model_response(&k),
        Ok(Ok(Err(err))) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "ok": false, "kind": kind, "error": err.to_string() })),
        )
            .into_response(),
        Ok(Err(_)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "ok": false, "kind": kind, "error": "model run panicked" })),
        )
            .into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "ok": false, "kind": kind, "error": "model task failed to join" })),
        )
            .into_response(),
    }
}

/// `GET /models/:kind/run` — run the kind's built-in example spec (one-click
/// demo). `?format=json` returns the raw artifact instead of the player.
pub(crate) async fn model_run_example(
    State(state): State<AppState>,
    Path(kind): Path<String>,
    Query(query): Query<FormatQuery>,
) -> Response {
    // Pull the owned example spec out before any `.await` so the non-`Send`
    // registry never crosses the await point.
    let spec = {
        let reg = with_builtins();
        match reg.get(&kind) {
            Some(citizen) => citizen.descriptor().example_spec,
            None => return unknown_model_response(&kind),
        }
    };
    run_model_spec(&state, kind, spec, wants_json(&query)).await
}

/// `POST /models/:kind/run` — run a user-supplied JSON spec for the kind.
pub(crate) async fn model_run_post(
    State(state): State<AppState>,
    Path(kind): Path<String>,
    Query(query): Query<FormatQuery>,
    Json(spec): Json<Value>,
) -> Response {
    run_model_spec(&state, kind, spec, wants_json(&query)).await
}

pub(crate) async fn run_streaming_model(state: AppState, name: String, body: String) -> Response {
    let _guard = state.sim_lock.lock().await;
    let name_for_run = name.clone();
    let outcome = tokio::task::spawn_blocking(move || {
        let mut out: Vec<u8> = Vec::new();
        let handled = run_named_jsonl(&name_for_run, body.as_bytes(), &mut out);
        (handled, out)
    })
    .await;

    match outcome {
        Ok((Ok(true), out)) => (
            [
                ("content-type", "application/x-ndjson; charset=utf-8"),
                ("x-content-type-options", "nosniff"),
            ],
            out,
        )
            .into_response(),
        Ok((Ok(false), _)) => (
            StatusCode::NOT_FOUND,
            Json(json!({
                "ok": false,
                "error": format!("unknown streaming model `{name}`"),
                "streaming": streaming_model_names(),
            })),
        )
            .into_response(),
        Ok((Err(err), _)) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "ok": false, "error": format!("stream error: {err}") })),
        )
            .into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "ok": false, "error": "stream task failed to join" })),
        )
            .into_response(),
    }
}

/// `POST /streaming/:name` — feed a JSONL command stream to a named solver and
/// return its JSONL result stream. Body is `text/plain`/`application/x-ndjson`.
pub(crate) async fn streaming_run(
    State(state): State<AppState>,
    Path(name): Path<String>,
    body: String,
) -> Response {
    run_streaming_model(state, name, body).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_registry_and_streaming_solvers_are_exposed() {
        let kinds = with_builtins().kinds();
        for expected in ["mdp", "pomdp", "hybrid", "studio"] {
            assert!(
                kinds.contains(&expected.to_string()),
                "missing kind {expected}"
            );
        }
        let streaming_names = streaming_model_names();
        for expected in ["lp", "milp", "mdp", "pomdp"] {
            assert!(
                streaming_names.contains(&expected),
                "missing streaming model {expected}"
            );
        }
        let contracts = streaming_contracts();
        assert_eq!(
            contracts.len(),
            streaming_names.len(),
            "every advertised streaming name must expose exactly one contract"
        );
        for (name, contract) in streaming_names.iter().zip(&contracts) {
            let expected_model = format!("streaming-{name}");
            assert_eq!(
                contract.model.as_str(),
                expected_model.as_str(),
                "streaming contract identifier drifted for route name {name}"
            );
        }
    }

    #[test]
    fn every_model_kind_runs_its_example_and_renders_a_player() {
        let reg = with_builtins();
        for desc in reg.descriptors() {
            let artifact = reg
                .run(&desc.kind, &desc.example_spec)
                .unwrap_or_else(|e| panic!("kind {} failed: {e}", desc.kind));
            let html = artifact.to_player_html();
            assert!(
                html.contains("<html") || html.contains("<!DOCTYPE") || html.contains("<!doctype"),
                "kind {} did not render an HTML player",
                desc.kind
            );
        }
    }
}
