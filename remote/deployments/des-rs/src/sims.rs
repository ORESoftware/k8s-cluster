use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    panic::{catch_unwind, AssertUnwindSafe},
    path::Path as StdPath,
    time::{Instant, UNIX_EPOCH},
};

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};

use des_engine::des::simulations::{run_simulations_matching, simulation_catalogue, SimOutcome};

use crate::output::{collect_artifacts, SOCCER_SIM_FRAMES_JSONL, SOCCER_SIM_META_JSON};
use crate::state::{now_ms, AppState};

/// Fast, HTML-producing simulations run once at startup so `/out/` has content
/// immediately. `main_build_site` is run last because it assembles the curated
/// `out/index.html` from whatever HTML the earlier sims rendered. Heavy sims
/// (e.g. `main_dispatch_combo`, `main_stochastic_sde*`) are intentionally
/// excluded; trigger those on demand via `/simulate`. Override with
/// `DES_STARTUP_SIMS` (comma-separated name filters), or set it empty to skip.
pub(crate) const DEFAULT_STARTUP_SIMS: &str = "main_wind_mppt_anim,main_temp_control_anim,main_observability_controllability_anim,main_empirical_control_report,main_elevator_highrise,main_two_disease,main_build_site";

const MAX_FILTER_LEN: usize = 96;
const MAX_SIMULATE_MATCHES: usize = 8;

/// All simulation names from the engine catalogue, in catalogue order.
pub(crate) fn sim_names() -> Vec<&'static str> {
    simulation_catalogue()
        .into_iter()
        .map(|(name, _)| name)
        .collect()
}

fn matching_sim_names(needle: &str, exact: bool) -> Vec<&'static str> {
    simulation_catalogue()
        .into_iter()
        .filter(|(name, _)| {
            if exact {
                *name == needle
            } else {
                name.contains(needle)
            }
        })
        .map(|(name, _)| name)
        .collect()
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum SimMatchError {
    NoMatches,
    TooMany {
        count: usize,
        preview: Vec<&'static str>,
    },
}

pub(crate) fn checked_sim_names(needle: &str, exact: bool) -> Result<Vec<&'static str>, SimMatchError> {
    let matches = matching_sim_names(needle, exact);
    if matches.is_empty() {
        return Err(SimMatchError::NoMatches);
    }
    if !exact && matches.len() > MAX_SIMULATE_MATCHES {
        return Err(SimMatchError::TooMany {
            count: matches.len(),
            preview: matches.into_iter().take(MAX_SIMULATE_MATCHES).collect(),
        });
    }
    Ok(matches)
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ArtifactFingerprint {
    len: u64,
    modified_ms: u128,
}

fn artifact_fingerprint(path: &StdPath) -> Option<ArtifactFingerprint> {
    let meta = fs::metadata(path).ok()?;
    let modified_ms = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis())
        .unwrap_or_default();
    Some(ArtifactFingerprint {
        len: meta.len(),
        modified_ms,
    })
}

fn artifact_snapshot(base: &StdPath) -> BTreeMap<String, ArtifactFingerprint> {
    let mut files = Vec::new();
    collect_artifacts(base, base, &mut files);
    files
        .into_iter()
        .filter_map(|rel| artifact_fingerprint(&base.join(&rel)).map(|fp| (rel, fp)))
        .collect()
}

fn changed_artifacts(
    before: &BTreeMap<String, ArtifactFingerprint>,
    after: &BTreeMap<String, ArtifactFingerprint>,
) -> Vec<String> {
    after
        .iter()
        .filter(|(rel, fp)| before.get(*rel) != Some(*fp))
        .map(|(rel, _)| rel.clone())
        .collect()
}

fn simulation_output_candidates(name: &str) -> &'static [&'static str] {
    match name {
        "main_bathrooms" => &["bathrooms.html"],
        "main_two_bathrooms" => &["two-bathrooms.html"],
        "main_build_site" => &["index.html"],
        "main_delivery_planner" => &["delivery-planner.html"],
        "main_empirical_control_report" => &[
            "empirical-control/report.html",
            "empirical-control/player.html",
            "empirical-control/player.frames.jsonl",
        ],
        "main_elevator_highrise" => &["elevator-highrise.html", "elevator-highrise-results.json"],
        "main_factmachine_markets" => &[
            "factmachine-markets.html",
            "factmachine-markets-results.json",
        ],
        "main_factory_floor_track3t" => &[
            "factory-floor-track3t.html",
            "factory-floor-track3t.json",
            "factory-floor-track3t.frames.jsonl",
        ],
        "main_shadow_eval" => &["shadow-eval/report.html", "shadow-eval/report.json"],
        "main_soccer" => &[
            "soccer-sim.html",
            "soccer-sim.meta.json",
            "soccer-sim.frames.jsonl",
        ],
        "main_soccer_planner" => &["soccer-planner.html"],
        "main_soccer_rotation_anim" => &[
            "soccer-IP-MIP-feasible.html",
            "soccer-IP-MIP-feasible.frames.jsonl",
            "soccer-IP-MIP-feasible-solver.html",
            "soccer-IP-MIP-feasible-solver.frames.jsonl",
        ],
        "main_temp_control_anim" => &[
            "temp-control/animation.html",
            "temp-control/animation.frames.jsonl",
            "temp-control/animation-heat-cool.html",
            "temp-control/animation-heat-cool.frames.jsonl",
        ],
        "main_traffic" => &[
            "traffic-flow-five-intersection.html",
            "traffic-flow-five-intersection.frames.jsonl",
            "smart-traffic-flow.html",
            "smart-traffic-flow.frames.jsonl",
        ],
        "main_two_disease" => &[
            "two-disease.html",
            "two-disease.frames.jsonl",
            "two-disease-framework.json",
        ],
        "main_wind_mppt_anim" => &[
            "wind-mppt/animation-optimal-torque.html",
            "wind-mppt/animation-optimal-torque.frames.jsonl",
            "wind-mppt/animation-pi.html",
            "wind-mppt/animation-pi.frames.jsonl",
        ],
        _ => &[],
    }
}

fn fallback_artifacts(
    after: &BTreeMap<String, ArtifactFingerprint>,
    sim_names: &[&str],
) -> Vec<String> {
    let mut rels = BTreeSet::new();
    for name in sim_names {
        for rel in simulation_output_candidates(name) {
            let lazy_soccer_trace = *name == "main_soccer"
                && matches!(*rel, SOCCER_SIM_META_JSON | SOCCER_SIM_FRAMES_JSONL);
            if after.contains_key(*rel) || lazy_soccer_trace {
                rels.insert((*rel).to_string());
            }
        }
    }
    rels.into_iter().collect()
}

pub(crate) fn artifact_ext(rel: &str) -> Option<&str> {
    StdPath::new(rel).extension().and_then(|ext| ext.to_str())
}

fn out_href(rel: &str) -> String {
    format!("out/{rel}")
}

fn choose_primary_artifact(rels: &[String]) -> Option<String> {
    rels.iter()
        .find(|rel| artifact_ext(rel.as_str()) == Some("html") && rel.as_str() != "index.html")
        .or_else(|| rels.iter().find(|rel| rel.as_str() == "index.html"))
        .or_else(|| {
            rels.iter()
                .find(|rel| artifact_ext(rel.as_str()) == Some("json"))
        })
        .or_else(|| {
            rels.iter()
                .find(|rel| artifact_ext(rel.as_str()) == Some("jsonl"))
        })
        .or_else(|| rels.first())
        .map(|rel| out_href(rel))
}

fn artifact_hrefs_for_ext(rels: &[String], ext: &str) -> Vec<String> {
    rels.iter()
        .filter(|rel| artifact_ext(rel.as_str()) == Some(ext))
        .map(|rel| out_href(rel))
        .collect()
}

fn artifact_summary(rels: Vec<String>) -> Value {
    let mut rels = rels;
    rels.sort();
    rels.dedup();
    json!({
        "primary": choose_primary_artifact(&rels),
        "html": artifact_hrefs_for_ext(&rels, "html"),
        "json": artifact_hrefs_for_ext(&rels, "json"),
        "jsonl": artifact_hrefs_for_ext(&rels, "jsonl"),
        "paths": rels,
    })
}

fn outcome_json(outcomes: &[SimOutcome], artifacts: &Value) -> Vec<Value> {
    outcomes
        .iter()
        .map(|o| {
            json!({
                "name": o.name,
                "ok": o.ok,
                "millis": o.millis,
                "artifacts": artifacts,
            })
        })
        .collect()
}

/// Run exactly the catalogue entry whose name equals `needle` (0 or 1 sims),
/// with the same panic isolation + timing as the engine's serial driver. Used
/// by the UI "Run" buttons so e.g. `main` does not match every `main_*` name.
fn run_exact(needle: &str) -> Vec<SimOutcome> {
    simulation_catalogue()
        .into_iter()
        .filter(|(name, _)| *name == needle)
        .map(|(name, sim)| {
            let start = Instant::now();
            let ok = catch_unwind(AssertUnwindSafe(sim)).is_ok();
            SimOutcome {
                name,
                ok,
                millis: start.elapsed().as_millis(),
            }
        })
        .collect()
}

/// Run catalogue sims in series on a blocking thread, holding the serial
/// simulation lock. `exact` runs only the exactly-named entry; otherwise every
/// sim whose name *contains* `needle` runs (the engine's filter semantics).
pub(crate) async fn run_filter(state: &AppState, needle: String, exact: bool) -> Vec<SimOutcome> {
    let _guard = state.sim_lock.lock().await;
    tokio::task::spawn_blocking(move || {
        if exact {
            run_exact(&needle)
        } else {
            run_simulations_matching(&needle)
        }
    })
    .await
    .unwrap_or_default()
}

pub(crate) async fn list_simulations() -> impl IntoResponse {
    let names = sim_names();
    Json(json!({
        "ok": true,
        "count": names.len(),
        "simulations": names,
    }))
}

#[derive(Debug, Deserialize)]
pub(crate) struct SimulateRequest {
    name: String,
    #[serde(default)]
    exact: bool,
}

#[derive(Debug, Deserialize)]
pub(crate) struct RunQuery {
    exact: Option<String>,
}

fn truthy(value: &Option<String>) -> bool {
    matches!(value.as_deref(), Some("1" | "true" | "yes"))
}

fn validate_filter(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("name (a simulation-name filter) must not be empty".to_string());
    }
    if trimmed.len() > MAX_FILTER_LEN {
        return Err(format!("name must be at most {MAX_FILTER_LEN} bytes"));
    }
    if trimmed.chars().any(|c| c.is_control()) {
        return Err("name must not contain control characters".to_string());
    }
    Ok(trimmed.to_string())
}

async fn run_response(state: &AppState, needle: String, exact: bool) -> Response {
    match checked_sim_names(&needle, exact) {
        Ok(_) => {}
        Err(SimMatchError::NoMatches) => {
            let how = if exact { "named" } else { "matching" };
            return (
                StatusCode::NOT_FOUND,
                Json(json!({
                    "ok": false,
                    "error": format!("no simulation {how} `{needle}`"),
                    "simulations": sim_names(),
                })),
            )
                .into_response();
        }
        Err(SimMatchError::TooMany { count, preview }) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "ok": false,
                    "error": format!(
                        "simulation filter `{needle}` matches {count} simulations; refine the name or use exact=true for a single catalogue entry"
                    ),
                    "matchCount": count,
                    "maxMatches": MAX_SIMULATE_MATCHES,
                    "preview": preview,
                })),
            )
                .into_response();
        }
    }
    let before = artifact_snapshot(state.out_dir.as_path());
    let outcomes = run_filter(state, needle.clone(), exact).await;
    let after = artifact_snapshot(state.out_dir.as_path());
    let successful_names: Vec<&str> = outcomes.iter().filter(|o| o.ok).map(|o| o.name).collect();
    let mut rels = changed_artifacts(&before, &after);
    rels.extend(fallback_artifacts(&after, &successful_names));
    let artifacts = artifact_summary(rels);
    let all_ok = outcomes.iter().all(|o| o.ok);
    Json(json!({
        "ok": all_ok,
        "filter": needle,
        "exact": exact,
        "ran": outcome_json(&outcomes, &artifacts),
        "artifacts": artifacts,
        "outputIndex": "out/",
        "atMs": now_ms(),
    }))
    .into_response()
}

pub(crate) async fn simulate(State(state): State<AppState>, Json(req): Json<SimulateRequest>) -> Response {
    match validate_filter(&req.name) {
        Ok(needle) => run_response(&state, needle, req.exact).await,
        Err(error) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "ok": false, "error": error })),
        )
            .into_response(),
    }
}

pub(crate) async fn run_named(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Query(query): Query<RunQuery>,
) -> Response {
    match validate_filter(&name) {
        Ok(needle) => run_response(&state, needle, truthy(&query.exact)).await,
        Err(error) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "ok": false, "error": error })),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalogue_is_exposed_and_nonempty() {
        let names = sim_names();
        assert!(names.len() >= 56, "expected the full engine catalogue");
        assert!(names.contains(&"main_build_site"));
        assert!(names.contains(&"main_electric_circuit"));
    }

    #[test]
    fn filter_validation_rejects_empty_and_oversize() {
        assert!(validate_filter("  ").is_err());
        assert!(validate_filter(&"x".repeat(MAX_FILTER_LEN + 1)).is_err());
        assert_eq!(
            validate_filter("  electric_circuit ").unwrap(),
            "electric_circuit"
        );
    }

    #[test]
    fn broad_simulation_filters_are_capped_before_running() {
        assert!(matches!(
            checked_sim_names("main", false),
            Err(SimMatchError::TooMany { count, .. }) if count > MAX_SIMULATE_MATCHES
        ));
        assert_eq!(
            checked_sim_names("main_electric_circuit", true).unwrap(),
            vec!["main_electric_circuit"]
        );
    }

    #[test]
    fn artifact_summary_prefers_html_and_exposes_data_links() {
        let summary = artifact_summary(vec![
            "shadow-eval/report.json".to_string(),
            "shadow-eval/report.html".to_string(),
            "shadow-eval/report.frames.jsonl".to_string(),
        ]);

        assert_eq!(
            summary["primary"].as_str(),
            Some("out/shadow-eval/report.html")
        );
        assert_eq!(
            summary["html"].as_array().unwrap()[0].as_str(),
            Some("out/shadow-eval/report.html")
        );
        assert_eq!(
            summary["json"].as_array().unwrap()[0].as_str(),
            Some("out/shadow-eval/report.json")
        );
        assert_eq!(
            summary["jsonl"].as_array().unwrap()[0].as_str(),
            Some("out/shadow-eval/report.frames.jsonl")
        );
    }
}
