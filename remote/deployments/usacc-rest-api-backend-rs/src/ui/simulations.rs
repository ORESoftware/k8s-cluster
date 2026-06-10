//! Simulations: a parameter form that runs the deterministic DES-backed
//! court simulation and renders its metrics. Mirrors the JSON
//! `POST /api/usacc/simulations` (optionally persisting the run).

use std::sync::atomic::Ordering;

use axum::extract::State;
use axum::Form;
use dd_pg_defs::USACC_SIMULATION_RUNS_TABLE;
use maud::{html, Markup};
use serde::Deserialize;
use serde_json::Value;

use crate::models::SimulationRunRequest;
use crate::simulation::run_simulation;
use crate::state::AppState;

use super::layout::{self, caption, section_header, NavSection, Ui};

pub async fn page(State(state): State<AppState>) -> Markup {
    let ui = Ui::new(&state.config.app_base_path);
    let body = html! {
        h1 { "Simulations" }
        (caption("Run a deterministic discrete-event court simulation. Same seed + parameters always yield the same result."))

        div class="split" {
            div {
                (section_header("Parameters", None))
                form class="card stacked"
                    hx-post=(ui.url("/app/simulations"))
                    hx-target="#sim-result"
                    hx-swap="innerHTML"
                {
                    div class="grid" style="grid-template-columns:1fr 1fr;" {
                        label class="field" { "Seed"
                            input type="text" name="seed" placeholder="auto";
                        }
                        label class="field" { "Horizon (days)"
                            input type="text" name="horizon_days" value="365";
                        }
                    }
                    div class="grid" style="grid-template-columns:1fr 1fr;" {
                        label class="field" { "Actor count"
                            input type="text" name="actor_count" value="64";
                        }
                        label class="field" { "Target signatures"
                            input type="text" name="target_signatures" value="100000";
                        }
                    }
                    div class="grid" style="grid-template-columns:1fr 1fr;" {
                        label class="field" { "Panel size"
                            input type="text" name="panel_size" value="12";
                        }
                        label class="field" { "Conviction threshold"
                            input type="text" name="conviction_threshold_count" value="9";
                        }
                    }
                    label class="field" {
                        span { input type="checkbox" name="persist" value="true"; " Persist this run to the database" }
                    }
                    div {
                        button type="submit" class="btn-primary" { "Run simulation" }
                        span class="htmx-indicator" style="margin-left:8px;" { "running…" }
                    }
                }
            }
            div {
                (section_header("Result", None))
                div #sim-result { (caption("Run a simulation to see metrics here.")) }
            }
        }
    };
    layout::page(ui, "Simulations", NavSection::Simulations, body)
}

#[derive(Deserialize)]
pub struct SimForm {
    seed: Option<String>,
    horizon_days: Option<String>,
    actor_count: Option<String>,
    target_signatures: Option<String>,
    panel_size: Option<String>,
    conviction_threshold_count: Option<String>,
    persist: Option<String>,
}

pub async fn run(State(state): State<AppState>, Form(form): Form<SimForm>) -> Markup {
    let want_persist = form.persist.as_deref() == Some("true");
    let request = SimulationRunRequest {
        case_id: None,
        seed: parse(form.seed.as_deref()),
        horizon_days: parse(form.horizon_days.as_deref()),
        actor_count: parse(form.actor_count.as_deref()),
        target_signatures: parse(form.target_signatures.as_deref()),
        sponsor_response_rate: None,
        admission_approval_rate: None,
        judge_conviction_rate: None,
        panel_size: parse(form.panel_size.as_deref()),
        conviction_threshold_count: parse(form.conviction_threshold_count.as_deref()),
        persist: Some(want_persist),
        input: None,
    };

    let mut response = run_simulation(request);
    state.metrics.simulations_total.fetch_add(1, Ordering::Relaxed);

    let mut persist_note = None;
    if want_persist {
        if let Some(pool) = state.pool.as_ref() {
            let seed_i64 = response.seed.min(i64::MAX as u64) as i64;
            let sql = format!(
                "insert into {USACC_SIMULATION_RUNS_TABLE} \
                 (status, mode, seed, horizon_days, actor_count, event_count, metrics, trace, input, started_at, finished_at) \
                 values ('succeeded', 'sim', $1, $2, $3, $4, $5::jsonb, $6::jsonb, '{{}}'::jsonb, now(), now()) returning id::text"
            );
            match sqlx::query_scalar::<_, String>(&sql)
                .bind(seed_i64)
                .bind(response.horizon_days)
                .bind(response.actor_count)
                .bind(response.event_count.min(i32::MAX as u64) as i32)
                .bind(&response.metrics)
                .bind(&response.trace)
                .fetch_one(pool)
                .await
            {
                Ok(run_id) => {
                    response.persisted = true;
                    persist_note = Some(layout::flash_ok(format!("Persisted as run {run_id}.")));
                }
                Err(e) => persist_note = Some(super::report_db_error("persist the run", e)),
            }
        } else {
            persist_note = Some(layout::flash_error("No database configured — run not persisted."));
        }
    }

    html! {
        @if let Some(note) = persist_note { (note) }
        div class="card" {
            dl class="kv" {
                dt { "Seed" } dd { (response.seed) }
                dt { "Horizon" } dd { (response.horizon_days) " days" }
                dt { "Actors" } dd { (response.actor_count) }
                dt { "Events" } dd { (response.event_count) }
            }
        }
        (section_header("Metrics", None))
        (metrics_table(&response.metrics))
    }
}

fn metrics_table(metrics: &Value) -> Markup {
    let entries: Vec<(String, String)> = match metrics {
        Value::Object(map) => map
            .iter()
            .map(|(k, v)| (k.clone(), scalar(v)))
            .collect(),
        _ => Vec::new(),
    };
    html! {
        @if entries.is_empty() {
            (caption("No scalar metrics reported."))
        } @else {
            div class="table-wrap" {
                table {
                    thead { tr { th { "Metric" } th class="num" { "Value" } } }
                    tbody {
                        @for (k, v) in &entries {
                            tr { td { (k) } td class="num" { (v) } }
                        }
                    }
                }
            }
        }
    }
}

fn scalar(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Null => "—".to_string(),
        other => other.to_string(),
    }
}

fn parse<T: std::str::FromStr>(v: Option<&str>) -> Option<T> {
    v.map(str::trim).filter(|s| !s.is_empty()).and_then(|s| s.parse::<T>().ok())
}
