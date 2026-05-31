//! `dd-des-rs` — HTTP server that runs the `discrete-event-system.rs` engine as
//! a library and serves the HTML result pages its simulations render.
//!
//! Unlike `dd-des-simulator` (which has its own generic event-queue engine and
//! serves the *TypeScript* submodule's pre-committed `out/`), this service
//! imports the real Rust `des_engine` crate (the `discrete-event-system.rs`
//! submodule) and *runs* its simulation catalogue on demand. Each simulation
//! writes its artifacts (`out/*.html`, `out/*-framework.json`, JSONL frames,
//! …) into a writable working directory, and the service serves them.
//!
//! ## HTTP API
//!
//! - `GET /healthz` — readiness/liveness probe.
//! - `GET /` — interactive landing page with per-simulation "Run" buttons.
//! - `GET /info` — service info + endpoint map (JSON).
//! - `GET /simulations` — the engine's full simulation catalogue.
//! - `POST /simulate` — run sims by `name` (filter, or exact via `{"exact":true}`), in series.
//! - `GET /simulations/:name/run` — convenience GET form (`?exact=1` for one entry).
//! - `GET /models` — first-class model registry (mdp, pomdp, hybrid, studio) with example specs.
//! - `GET /models/:kind/run` — run a kind's example spec and render an interactive player (`?format=json` for the raw artifact).
//! - `POST /models/:kind/run` — run a user-supplied JSON spec for a kind (renders a player; `?format=json` for the artifact).
//! - `GET /streaming` — JSONL streaming-solver contracts (lp, milp, mdp, pomdp).
//! - `POST /streaming/:name` — stream JSONL commands to a solver; responds with a JSONL frame stream.
//! - `GET /elevator-fel` — the new next-event (FEL) elevator simulation, animated.
//! - `GET /elevator-mdp` — elevator-dispatch MDP player (value-iterated).
//! - `GET /elevator-pomdp` — elevator-dispatch POMDP player (noisy call button; belief-tracked).
//! - `GET /out`, `/out/`, `/out/*path` — serve rendered artifacts (curated `index.html` if present, else a listing).
//! - `GET /docs/api`, `/api/docs` — generated HTML API docs.
//! - `GET /api/docs.json` — machine-readable API docs.
//!
//! Simulations are serialized behind a single lock: the engine drives a
//! process-global clock / RNG and `println!`s its report, so running two at
//! once would interleave output and race shared state (the engine's own
//! `run_all_simulations` is likewise strictly serial).

use std::{
    env,
    net::SocketAddr,
    panic::{catch_unwind, AssertUnwindSafe},
    path::{Path as StdPath, PathBuf},
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use axum::{
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{header, HeaderMap, HeaderName, HeaderValue, StatusCode},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::Mutex;

use des_engine::des::fel::elevator::{
    elevator_mdp_spec, elevator_pomdp_spec, render_elevator_html, run_fel_elevator, ElevatorConfig,
};
use des_engine::des::model::{with_builtins, CitizenError};
use des_engine::des::service::{
    Capability, DesExtension, EndpointKind, EngineCatalogExtension, ServiceBuilder,
    ServiceDescriptor, ServiceInfo, DD_API_DOCS_HEADER,
};
use des_engine::des::simulations::{run_simulations_matching, simulation_catalogue, SimOutcome};
use des_engine::des::streaming::{run_named_jsonl, streaming_contracts, streaming_model_names};

/// Fast, HTML-producing simulations run once at startup so `/out/` has content
/// immediately. `main_build_site` is run last because it assembles the curated
/// `out/index.html` from whatever HTML the earlier sims rendered. Heavy sims
/// (e.g. `main_dispatch_combo`, `main_stochastic_sde*`) are intentionally
/// excluded; trigger those on demand via `/simulate`. Override with
/// `DES_STARTUP_SIMS` (comma-separated name filters), or set it empty to skip.
const DEFAULT_STARTUP_SIMS: &str = "main_wind_mppt_anim,main_temp_control_anim,main_observability_controllability_anim,main_empirical_control_report,main_elevator_highrise,main_two_disease,main_build_site";

// Generous enough for model specs and JSONL streaming command batches, while
// still bounding memory per request (simulations themselves take no body).
const MAX_HTTP_BODY_BYTES: usize = 2 * 1024 * 1024;
const MAX_FILTER_LEN: usize = 96;

/// Interactive landing page. All `fetch`/link URLs are RELATIVE so the page
/// works both at `/` (local `cargo run`) and behind the gateway at `/des-rs/`
/// (which strips the prefix). "Run" buttons hit `simulations/<name>/run?exact=1`
/// so a click runs exactly one catalogue entry.
const LANDING_HTML: &str = r####"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>discrete-event-system.rs — DES engine</title>
<style>
:root{color-scheme:dark;--bg:#0b1021;--panel:#0f1422;--line:#21262d;--ink:#e6edf3;--dim:#9aa4b2;--accent:#1f6feb;--accent2:#388bfd;--ok:#3fb950;--err:#f85149}
*{box-sizing:border-box}
body{font-family:system-ui,-apple-system,'Segoe UI',Roboto,sans-serif;margin:0;color:var(--ink);
  background:
    radial-gradient(1100px 520px at 12% -8%,rgba(31,111,235,.20),transparent 60%),
    radial-gradient(900px 480px at 96% 0%,rgba(126,231,135,.10),transparent 55%),
    var(--bg)}
main{max-width:1060px;margin:0 auto;padding:30px 22px 80px}
.hero{border:1px solid var(--line);border-radius:16px;padding:24px 24px 22px;margin:6px 0 28px;
  background:linear-gradient(180deg,rgba(31,111,235,.10),rgba(15,20,34,.6));backdrop-filter:blur(2px)}
.hero .top{display:flex;align-items:center;gap:12px;flex-wrap:wrap}
h1{font-size:1.8rem;margin:0;letter-spacing:-.01em;background:linear-gradient(90deg,#e6edf3,#9ecbff);-webkit-background-clip:text;background-clip:text;-webkit-text-fill-color:transparent}
.pill-health{display:inline-flex;align-items:center;gap:7px;font-size:.78rem;color:var(--dim);border:1px solid var(--line);border-radius:999px;padding:4px 11px;background:#0b1021}
.pill-health .dot{width:8px;height:8px;border-radius:50%;background:#6b7689;box-shadow:0 0 0 0 rgba(63,185,80,.5)}
.pill-health.up .dot{background:var(--ok);animation:pulse 2.2s infinite}
.pill-health.down .dot{background:var(--err)}
.pill-health.up{color:#b7f0c2;border-color:rgba(63,185,80,.35)}
@keyframes pulse{0%{box-shadow:0 0 0 0 rgba(63,185,80,.45)}70%{box-shadow:0 0 0 7px rgba(63,185,80,0)}100%{box-shadow:0 0 0 0 rgba(63,185,80,0)}}
.sub{color:var(--dim);margin:12px 0 16px;font-size:.95rem;line-height:1.55;max-width:74ch}
.actions{display:flex;gap:10px;flex-wrap:wrap;margin:0}
a.btn,button.btn{font:inherit;font-size:.9rem;cursor:pointer;border-radius:8px;padding:9px 14px;text-decoration:none;border:1px solid #2b3344;background:#161b22;color:#e6edf3}
a.btn.primary{background:#1f6feb;border-color:#1f6feb;color:#fff}
a.btn:hover,button.btn:hover{border-color:#3b82f6}
h2{font-size:1.06rem;margin:34px 0 12px;color:#c9d4e3;display:flex;align-items:center;gap:9px}
h2::before{content:"";width:4px;height:16px;border-radius:3px;background:linear-gradient(180deg,var(--accent),var(--accent2))}
.muted{color:#6b7689;font-weight:400;font-size:.85rem}
.grid{display:grid;grid-template-columns:repeat(auto-fill,minmax(244px,1fr));gap:12px}
.sim{display:flex;flex-direction:column;gap:8px;border:1px solid var(--line);border-radius:12px;padding:14px;background:var(--panel);transition:transform .14s ease,border-color .14s ease,box-shadow .14s ease}
.sim:hover{transform:translateY(-2px);border-color:#30496f;box-shadow:0 10px 26px rgba(0,0,0,.35)}
.sim.feat{background:linear-gradient(180deg,rgba(31,111,235,.07),var(--panel))}
.sim .label{font-size:.92rem;text-transform:capitalize}
.sim .name{font-family:ui-monospace,SFMono-Regular,Menlo,monospace;font-size:.78rem;color:#9ecbff;word-break:break-all}
.sim .desc{font-size:.8rem;color:#8b949e;line-height:1.45;flex:1}
.sim .row{display:flex;align-items:center;gap:8px;justify-content:flex-end;margin-top:2px}
.sim .open{font:inherit;font-size:.82rem;cursor:pointer;border-radius:7px;padding:6px 14px;border:1px solid #1f6feb;background:#1f6feb;color:#fff;text-decoration:none}
.sim .open:hover{background:#388bfd;border-color:#388bfd}
.sim .json{font:inherit;font-size:.8rem;border-radius:7px;padding:6px 10px;border:1px solid #2b3344;background:#161b22;color:#9aa4b2;text-decoration:none}
.sim .json:hover{border-color:#3b82f6;color:#e6edf3}
.list{display:flex;flex-direction:column;gap:8px}
.strow{border:1px solid #21262d;border-radius:9px;padding:10px 14px;background:#0f1422;font-size:.86rem;line-height:1.45;color:#c9d4e3}
.strow code{font-family:ui-monospace,SFMono-Regular,Menlo,monospace;color:#9ecbff;background:#161b22;padding:1px 7px;border-radius:5px}
.strow .ops{color:#6b7689;font-size:.78rem;margin-left:6px}
.run{font:inherit;font-size:.82rem;cursor:pointer;border-radius:7px;padding:6px 14px;border:1px solid #238636;background:#238636;color:#fff}
.run:hover{background:#2ea043}
.run[disabled]{opacity:.55;cursor:default}
.st{font-size:.8rem;color:#9aa4b2;min-height:1.1em}
.st.ok{color:#3fb950}.st.err{color:#f85149}
.filterbar{position:sticky;top:0;z-index:5;display:flex;align-items:center;gap:10px;flex-wrap:wrap;margin:0 0 12px;padding:10px 0;background:linear-gradient(180deg,var(--bg) 70%,rgba(11,16,33,0))}
#filter{font:inherit;font-size:.9rem;background:#0f1422;border:1px solid var(--line);border-radius:8px;color:var(--ink);padding:9px 12px;width:280px;max-width:60vw}
#filter:focus{outline:none;border-color:var(--accent);box-shadow:0 0 0 3px rgba(31,111,235,.25)}
.filterbar .kbd{font-family:ui-monospace,Menlo,monospace;font-size:.72rem;color:#8b949e;border:1px solid var(--line);border-bottom-width:2px;border-radius:6px;padding:1px 6px;background:#0b1021}
.filterbar .shown{color:#8b949e;font-size:.82rem}
.toast{position:fixed;left:50%;bottom:26px;transform:translateX(-50%) translateY(160%);transition:transform .25s;background:#161b22;border:1px solid #2b3344;border-radius:10px;padding:12px 18px;box-shadow:0 8px 30px rgba(0,0,0,.5);font-size:.9rem}
.toast.show{transform:translateX(-50%) translateY(0)}
.toast a{color:#58a6ff}
</style>
</head>
<body>
<main>
<div class="hero">
  <div class="top">
    <h1>discrete-event-system.rs</h1>
    <span id="health" class="pill-health"><span class="dot"></span><span class="txt">checking…</span></span>
  </div>
  <p class="sub">A Rust modeling &amp; simulation engine, imported here as a <strong>library</strong> (git submodule) and run <strong>in-process</strong>. Run a <strong>first-class model</strong> for an interactive player, stream commands to a <strong>solver</strong>, or run any catalogue <strong>simulation</strong> and open the rendered HTML/JSON.</p>
  <div class="actions">
    <a class="btn primary" href="out/">View rendered results &rarr;</a>
    <a class="btn" href="docs/api">API docs</a>
    <a class="btn" href="info">Service info</a>
    <a class="btn" href="models">Models JSON</a>
    <a class="btn" href="streaming">Streaming JSON</a>
    <a class="btn" href="simulations">Catalogue JSON</a>
  </div>
</div>
<h2>Elevator <span class="muted">— next-event (FEL) sim + dispatch decision models</span></h2>
<div class="grid">
  <div class="sim">
    <div class="label">FEL elevator</div>
    <div class="name">des::fel::elevator</div>
    <div class="desc">A next-event single-car elevator under a LOOK (collective-control) policy. The clock jumps event-to-event (arrival / car-step / doors-close) and skips idle time. Animated shaft with boarding/alighting and live charts.</div>
    <div class="row"><a class="open" href="elevator-fel" target="_blank" rel="noopener">Open animation &#8599;</a></div>
  </div>
  <div class="sim">
    <div class="label">Elevator dispatch MDP</div>
    <div class="name">des/mdp/v1 &middot; value-iteration</div>
    <div class="desc">Fully-observed dispatch as a 12-state MDP (car floor &times; pending call). Value iteration recovers the drive-to-the-call-and-serve policy, animated over the state graph.</div>
    <div class="row"><a class="open" href="elevator-mdp" target="_blank" rel="noopener">Open player &#8599;</a></div>
  </div>
  <div class="sim">
    <div class="label">Elevator dispatch POMDP</div>
    <div class="name">des/pomdp/v1 &middot; belief tracking</div>
    <div class="desc">Dispatch under a noisy hall-call button: hidden demand is empty / waiting / crowded and the button false-triggers and misses. Belief over hidden states drives the hold-vs-dispatch decision.</div>
    <div class="row"><a class="open" href="elevator-pomdp" target="_blank" rel="noopener">Open player &#8599;</a></div>
  </div>
</div>
<h2>Control &amp; estimation <span class="muted">— back-EMF DC motor, controllability/observability, shadow Gramians</span></h2>
<div id="control" class="grid"></div>
<h2>First-class models <span class="muted">— describe &rarr; run &rarr; interactive player</span></h2>
<div id="models" class="grid"></div>
<h2>Streaming solvers <span class="muted">— JSONL commands in, JSONL frames out</span></h2>
<div id="streaming" class="list"></div>
<h2>Featured</h2>
<div id="featured" class="grid"></div>
<h2>All simulations <span id="count" class="muted"></span></h2>
<div class="filterbar">
  <input id="filter" placeholder="filter by name…" oninput="filterSims()" autocomplete="off" spellcheck="false">
  <span class="kbd">/</span><span class="shown" id="shown"></span>
</div>
<div id="all" class="grid"></div>
</main>
<div id="toast" class="toast"></div>
<script>
const FEATURED=[["main_build_site","Build site index"],["main_elevator_highrise","Elevator high-rise"],["main_factmachine_markets","FactMachine markets"],["main_two_disease","Two-disease epidemic"],["main_electric_circuit","Electric circuit"],["main_traffic","Traffic network"],["main_court_mdp","Court MDP"],["main_convolution","Convolution"]];
const CONTROL=[
  ["main_shadow_eval","Shadow Gramians","Probe each plant as a black box: recover controllability/observability Gramians from perturbed shadow copies, cross-check against the analytic model, then re-ask via a nested MDP/POMDP of the motor's speed regimes."],
  ["main_observability_controllability_anim","Obs / ctrl (animated)","Kalman rank tests for controllability & observability of a state-space model, animated step by step."],
  ["main_empirical_control_report","Empirical control report","Monte-Carlo trials + Gramian eigenvalue degrees that quantify how much control and observation authority a system actually has."],
  ["main_dc_motor_anim","DC motor (back-EMF)","Separately-excited DC motor with explicit back-EMF coupling (E = K_e·ω), RK4-integrated and animated."],
  ["main_wind_mppt_anim","Wind MPPT","Maximum-power-point-tracking controller on a wind turbine, animated."]
];
function toast(html){const t=document.getElementById('toast');t.innerHTML=html;t.classList.add('show');clearTimeout(window.__tt);window.__tt=setTimeout(function(){t.classList.remove('show');},6000);}
function simCard(name,label,desc,feat){
  const card=document.createElement('div');card.className=feat?'sim feat':'sim';card.dataset.name=name;
  const lab=document.createElement('div');lab.className='label';lab.textContent=label||name;
  const nm=document.createElement('div');nm.className='name';nm.textContent=name;
  card.appendChild(lab);card.appendChild(nm);
  if(desc){const d=document.createElement('div');d.className='desc';d.textContent=desc;card.appendChild(d);}
  const row=document.createElement('div');row.className='row';
  const st=document.createElement('span');st.className='st';
  const btn=document.createElement('button');btn.className='run';btn.textContent='Run';
  btn.onclick=function(){run(name,btn,st);};
  row.appendChild(st);row.appendChild(btn);
  card.appendChild(row);
  return card;
}
async function run(name,btn,st){
  btn.disabled=true;const old=btn.textContent;btn.textContent='Running…';st.className='st';st.textContent='running…';
  try{
    const r=await fetch('simulations/'+encodeURIComponent(name)+'/run?exact=1');
    const d=await r.json();
    const o=(d.ran&&d.ran[0])||{};
    if(d.ok){st.className='st ok';st.textContent='\u2713 '+(o.millis!=null?o.millis+' ms':'done');toast('Ran <code>'+name+'</code> — <a href="out/">view results &rarr;</a>');}
    else{st.className='st err';st.textContent='\u2717 '+(d.error||'failed');}
  }catch(e){st.className='st err';st.textContent='\u2717 '+e;}
  finally{btn.disabled=false;btn.textContent=old;}
}
function filterSims(){
  const q=document.getElementById('filter').value.toLowerCase();
  let shown=0,total=0;
  document.querySelectorAll('#all .sim').forEach(function(c){total++;const m=c.dataset.name.indexOf(q)>=0;c.style.display=m?'':'none';if(m)shown++;});
  document.getElementById('shown').textContent=q?(shown+' / '+total+' shown'):'';
}
function modelCard(m){
  const card=document.createElement('div');card.className='sim';
  const lab=document.createElement('div');lab.className='label';lab.textContent=m.title||m.kind;
  const nm=document.createElement('div');nm.className='name';nm.textContent=m.kind+' \u00b7 '+(m.specSchema||'')+(m.methods&&m.methods.length?' \u00b7 '+m.methods.join(', '):'');
  const desc=document.createElement('div');desc.className='desc';desc.textContent=m.description||'';
  const row=document.createElement('div');row.className='row';
  const js=document.createElement('a');js.className='json';js.textContent='JSON';js.href='models/'+encodeURIComponent(m.kind)+'/run?format=json';js.target='_blank';js.rel='noopener';
  const open=document.createElement('a');open.className='open';open.textContent='Open player \u2197';open.href='models/'+encodeURIComponent(m.kind)+'/run';open.target='_blank';open.rel='noopener';
  row.appendChild(js);row.appendChild(open);
  card.appendChild(lab);card.appendChild(nm);card.appendChild(desc);card.appendChild(row);
  return card;
}
function streamRow(c){
  const row=document.createElement('div');row.className='strow';
  const ops=(c.inputOps&&c.inputOps.length)||0;
  row.innerHTML='POST <code>streaming/'+c.model+'</code><span class="ops">'+ops+' command op(s)</span><br>'+
    (c.description||'').replace(/[<>&]/g,function(ch){return {'<':'&lt;','>':'&gt;','&':'&amp;'}[ch];});
  return row;
}
(async function(){
  try{
    const r=await fetch('models');const d=await r.json();
    const wrap=document.getElementById('models');
    (d.models||[]).forEach(function(m){wrap.appendChild(modelCard(m));});
  }catch(e){document.getElementById('models').textContent='failed to load models';}
  try{
    const r=await fetch('streaming');const d=await r.json();
    const wrap=document.getElementById('streaming');
    (d.streaming||[]).forEach(function(c){wrap.appendChild(streamRow(c));});
  }catch(e){document.getElementById('streaming').textContent='failed to load streaming contracts';}
})();
(function(){
  const c=document.getElementById('control');
  CONTROL.forEach(function(p){c.appendChild(simCard(p[0],p[1],p[2],true));});
})();
(async function(){
  const f=document.getElementById('featured');
  FEATURED.forEach(function(p){f.appendChild(simCard(p[0],p[1],null,true));});
  try{
    const r=await fetch('simulations');const d=await r.json();
    document.getElementById('count').textContent='('+d.count+')';
    const all=document.getElementById('all');
    d.simulations.forEach(function(n){all.appendChild(simCard(n,n.replace(/^main_/,'').replace(/_/g,' ')));});
  }catch(e){document.getElementById('count').textContent='(failed to load)';}
})();
(async function(){
  const el=document.getElementById('health');
  try{
    const r=await fetch('healthz',{cache:'no-store'});
    const d=await r.json();
    el.className='pill-health '+(d&&d.ok?'up':'down');
    el.querySelector('.txt').textContent=d&&d.ok?'healthy':'unhealthy';
  }catch(e){el.className='pill-health down';el.querySelector('.txt').textContent='offline';}
})();
document.addEventListener('keydown',function(e){
  const fi=document.getElementById('filter');
  if(e.key==='/'&&document.activeElement!==fi){e.preventDefault();fi.focus();fi.select();}
  else if(e.key==='Escape'&&document.activeElement===fi){fi.value='';filterSims();fi.blur();}
});
</script>
</body>
</html>"####;

#[derive(Clone)]
struct AppState {
    /// Absolute path to the directory the engine writes artifacts into
    /// (`<work>/out`). Held as an absolute path so request handlers are immune
    /// to the process `chdir` done at startup.
    out_dir: Arc<PathBuf>,
    /// Serializes simulation runs (the engine is single-clock / single-RNG).
    sim_lock: Arc<Mutex<()>>,
    /// Discovery `Link` header (relative RFC 8288 targets) emitted on the
    /// canonical landing routes so a machine can find the docs from `/` alone.
    link_header: Arc<str>,
    /// `dd-server-api-docs` discovery header value (relative).
    dd_docs_header: Arc<str>,
    /// The independently-rendered HTML docs page (a view over the descriptor).
    docs_html: Arc<str>,
    /// The canonical machine-readable descriptor JSON (`/api/docs.json`).
    docs_json: Arc<str>,
    /// Pre-rendered HTML for the new FEL elevator artifacts. These are
    /// deterministic (fixed seeds / tabular solves), so they are rendered once
    /// at startup and served verbatim — no per-request engine run, no lock.
    elevator_fel_html: Arc<str>,
    elevator_mdp_html: Arc<str>,
    elevator_pomdp_html: Arc<str>,
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn env_value(key: &str, fallback: &str) -> String {
    env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| fallback.to_string())
}

/// All simulation names from the engine catalogue, in catalogue order.
fn sim_names() -> Vec<&'static str> {
    simulation_catalogue()
        .into_iter()
        .map(|(name, _)| name)
        .collect()
}

fn outcome_json(outcomes: &[SimOutcome]) -> Vec<Value> {
    outcomes
        .iter()
        .map(|o| json!({ "name": o.name, "ok": o.ok, "millis": o.millis }))
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
async fn run_filter(state: &AppState, needle: String, exact: bool) -> Vec<SimOutcome> {
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

// =============================================================================
// JSON / control routes
// =============================================================================

async fn healthz() -> impl IntoResponse {
    Json(json!({ "ok": true, "service": "dd-des-rs", "atMs": now_ms() }))
}

/// Human-facing landing page: featured + full catalogue with "Run" buttons
/// (each does a relative `fetch` so it works at `/` locally and behind the
/// gateway at `/des-rs/`), plus a link to the rendered `out/` results. The
/// canonical landing route also carries the discovery headers, so a machine
/// that hits only `/` learns where the docs live.
async fn root(State(state): State<AppState>) -> Response {
    let mut res = Html(LANDING_HTML).into_response();
    apply_discovery_headers(res.headers_mut(), &state);
    res
}

/// Machine-readable service info (the old JSON root), including the discovery
/// hints that are also returned as HTTP response headers.
async fn info(State(state): State<AppState>) -> Response {
    let mut res = Json(json!({
        "ok": true,
        "service": "dd-des-rs",
        "mode": "runs the discrete-event-system.rs engine (library) and serves rendered HTML",
        "engineSimulations": sim_names().len(),
        "modelKinds": with_builtins().kinds(),
        "streamingSolvers": streaming_model_names(),
        "endpoints": {
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

async fn list_simulations() -> impl IntoResponse {
    let names = sim_names();
    Json(json!({
        "ok": true,
        "count": names.len(),
        "simulations": names,
    }))
}

#[derive(Debug, Deserialize)]
struct SimulateRequest {
    name: String,
    #[serde(default)]
    exact: bool,
}

#[derive(Debug, Deserialize)]
struct RunQuery {
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
    let outcomes = run_filter(state, needle.clone(), exact).await;
    if outcomes.is_empty() {
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
    let all_ok = outcomes.iter().all(|o| o.ok);
    Json(json!({
        "ok": all_ok,
        "filter": needle,
        "exact": exact,
        "ran": outcome_json(&outcomes),
        "outputIndex": "out/",
        "atMs": now_ms(),
    }))
    .into_response()
}

async fn simulate(State(state): State<AppState>, Json(req): Json<SimulateRequest>) -> Response {
    match validate_filter(&req.name) {
        Ok(needle) => run_response(&state, needle, req.exact).await,
        Err(error) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "ok": false, "error": error })),
        )
            .into_response(),
    }
}

async fn run_named(
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
struct FormatQuery {
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
async fn list_models() -> impl IntoResponse {
    let descriptors = with_builtins().descriptors();
    Json(json!({
        "ok": true,
        "count": descriptors.len(),
        "models": descriptors,
        "note": "Run a model: GET models/<kind>/run renders its example spec as an interactive player; POST models/<kind>/run with a JSON spec runs your own (add ?format=json for the raw artifact).",
    }))
}

/// `GET /streaming` — the JSONL streaming-solver contracts (lp, milp/mip/ip,
/// mdp, pomdp): each is an iterative solver fed a JSONL command stream.
async fn list_streaming() -> impl IntoResponse {
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
        catch_unwind(AssertUnwindSafe(|| with_builtins().run(&kind_for_run, &spec)))
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
async fn model_run_example(
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
async fn model_run_post(
    State(state): State<AppState>,
    Path(kind): Path<String>,
    Query(query): Query<FormatQuery>,
    Json(spec): Json<Value>,
) -> Response {
    run_model_spec(&state, kind, spec, wants_json(&query)).await
}

/// `POST /streaming/:name` — feed a JSONL command stream to a named solver and
/// return its JSONL result stream. Body is `text/plain`/`application/x-ndjson`.
async fn streaming_run(
    State(state): State<AppState>,
    Path(name): Path<String>,
    body: String,
) -> Response {
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

// =============================================================================
// Elevator showcase: the new FEL elevator sim + its MDP/POMDP dispatch models.
//
// All three are rendered once at startup into `AppState` (deterministic), so
// these routes just serve cached HTML — fast, lock-free, and always available.
// =============================================================================

/// `GET /elevator-fel` — the next-event (future-event-list) single-car elevator
/// under a LOOK policy, as a self-contained animated page.
async fn elevator_fel(State(state): State<AppState>) -> Html<String> {
    Html(state.elevator_fel_html.to_string())
}

/// `GET /elevator-mdp` — the fully-observed elevator-dispatch MDP, value-iterated
/// and rendered as an animated state-graph rollout player.
async fn elevator_mdp(State(state): State<AppState>) -> Html<String> {
    Html(state.elevator_mdp_html.to_string())
}

/// `GET /elevator-pomdp` — elevator dispatch under a noisy hall-call button,
/// rendered as a belief-tracking player.
async fn elevator_pomdp(State(state): State<AppState>) -> Html<String> {
    Html(state.elevator_pomdp_html.to_string())
}

/// Render the elevator MDP/POMDP players at startup, degrading to a small error
/// page (rather than panicking the server) if a solve ever fails.
fn render_model_player(kind: &str, spec: &Value) -> String {
    match with_builtins().run(kind, spec) {
        Ok(artifact) => artifact.to_player_html(),
        Err(err) => format!(
            "<!doctype html><html><head><meta charset=\"utf-8\"><title>{kind} unavailable</title>\
             </head><body style=\"font-family:system-ui;background:#0b1021;color:#e6edf3;padding:40px\">\
             <h1>elevator {kind} model unavailable</h1><p>{}</p></body></html>",
            html_escape(&err.to_string())
        ),
    }
}

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

fn serve_file(path: &StdPath) -> Response {
    match std::fs::read(path) {
        Ok(bytes) => (
            [
                ("content-type", content_type(path)),
                ("x-content-type-options", "nosniff"),
                ("cache-control", "public, max-age=30"),
            ],
            bytes,
        )
            .into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

const LISTED_EXTENSIONS: [&str; 6] = ["html", "json", "csv", "jsonl", "svg", "png"];

/// Recursively collect servable artifacts under `dir`, returned as
/// forward-slash relative paths sorted alphabetically for a stable listing.
fn collect_artifacts(dir: &StdPath, base: &StdPath, out: &mut Vec<String>) {
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

fn html_escape(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

async fn out_redirect() -> Response {
    Redirect::permanent("out/").into_response()
}

async fn out_index(State(state): State<AppState>) -> Response {
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
    let artifacts: Vec<&String> = files.iter().filter(|f| f.as_str() != "index.html").collect();

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

async fn out_file(State(state): State<AppState>, Path(rel_path): Path<String>) -> Response {
    let base: &StdPath = state.out_dir.as_path();

    let Some(target) = resolve_within(base, &base.join(&rel_path)) else {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    };

    if target.is_dir() {
        if let Some(index) = resolve_within(base, &target.join("index.html")) {
            if index.is_file() {
                return serve_file(&index);
            }
        }
        return (StatusCode::NOT_FOUND, "not found").into_response();
    }

    if !target.is_file() {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    }

    serve_file(&target)
}

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
fn build_descriptor() -> ServiceDescriptor {
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
            "First-class model registry (mdp, pomdp, hybrid, studio) with example specs.",
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
fn apply_discovery_headers(headers: &mut HeaderMap, state: &AppState) {
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
fn render_docs_html(descriptor: &ServiceDescriptor) -> String {
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

async fn api_docs_html(State(state): State<AppState>) -> Html<String> {
    Html(state.docs_html.to_string())
}

async fn api_docs_json(State(state): State<AppState>) -> Response {
    let mut res = state.docs_json.to_string().into_response();
    res.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json; charset=utf-8"),
    );
    res
}

// =============================================================================

/// Resolve the writable working directory the engine renders artifacts into.
/// Honors `DES_WORK_DIR`, else a per-process temp dir (the engine writes
/// CWD-relative `out/`, so the process `chdir`s here at startup).
fn work_dir() -> PathBuf {
    if let Ok(dir) = env::var("DES_WORK_DIR") {
        if !dir.trim().is_empty() {
            return PathBuf::from(dir.trim());
        }
    }
    env::temp_dir().join("dd-des-rs")
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let host = env_value("HOST", "0.0.0.0");
    let port = env_value("PORT", "8112").parse::<u16>()?;

    // The engine writes artifacts to `out/` relative to the process CWD. Point
    // the process at a writable working dir so this works under a read-only
    // root filesystem (k8s mounts the repo read-only and gives us /tmp).
    let work = work_dir();
    std::fs::create_dir_all(work.join("out"))?;
    env::set_current_dir(&work)?;
    let out_dir = work
        .join("out")
        .canonicalize()
        .unwrap_or_else(|_| work.join("out"));

    // Build the machine-readable service descriptor once (the JSON-first
    // contract owned by the engine library), then precompute the HTML view and
    // the discovery headers so request handlers stay allocation-light.
    let descriptor = build_descriptor();
    let link_header: Arc<str> = Arc::from(descriptor.link_header_relative());
    let dd_docs_header: Arc<str> = Arc::from(descriptor.dd_api_docs_relative());
    let docs_html: Arc<str> = Arc::from(render_docs_html(&descriptor));
    let docs_json: Arc<str> = Arc::from(descriptor.to_json_string());

    // Pre-render the (deterministic) FEL elevator artifacts once. Done before
    // the server starts serving and before the startup catalogue task spawns, so
    // there is no contention on the engine's process-global clock/RNG.
    let elevator_fel_html: Arc<str> =
        Arc::from(render_elevator_html(&run_fel_elevator(&ElevatorConfig::default())));
    let elevator_mdp_html: Arc<str> = Arc::from(render_model_player("mdp", &elevator_mdp_spec()));
    let elevator_pomdp_html: Arc<str> =
        Arc::from(render_model_player("pomdp", &elevator_pomdp_spec()));

    let state = AppState {
        out_dir: Arc::new(out_dir),
        sim_lock: Arc::new(Mutex::new(())),
        link_header,
        dd_docs_header,
        docs_html,
        docs_json,
        elevator_fel_html,
        elevator_mdp_html,
        elevator_pomdp_html,
    };

    // Populate `out/` in the background so /healthz comes up immediately while
    // the startup catalogue renders.
    let startup = env_value("DES_STARTUP_SIMS", DEFAULT_STARTUP_SIMS);
    if !startup.is_empty() {
        let startup_state = state.clone();
        tokio::spawn(async move {
            for needle in startup
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
            {
                let outcomes = run_filter(&startup_state, needle.clone(), false).await;
                println!(
                    "[dd-des-rs] startup `{needle}`: ran {} sim(s)",
                    outcomes.len()
                );
            }
            println!("[dd-des-rs] startup catalogue complete");
        });
    }

    let app = Router::new()
        .route("/", get(root))
        .route("/info", get(info))
        .route("/healthz", get(healthz))
        .route("/simulations", get(list_simulations))
        .route("/simulate", post(simulate))
        .route("/simulations/:name/run", get(run_named))
        .route("/models", get(list_models))
        .route("/models/:kind/run", get(model_run_example).post(model_run_post))
        .route("/streaming", get(list_streaming))
        .route("/streaming/:name", post(streaming_run))
        .route("/elevator-fel", get(elevator_fel))
        .route("/elevator-mdp", get(elevator_mdp))
        .route("/elevator-pomdp", get(elevator_pomdp))
        .route("/out", get(out_redirect))
        .route("/out/", get(out_index))
        .route("/out/*path", get(out_file))
        .route("/docs/api", get(api_docs_html))
        .route("/api/docs", get(api_docs_html))
        .route("/api/docs.json", get(api_docs_json))
        .layer(DefaultBodyLimit::max(MAX_HTTP_BODY_BYTES))
        .with_state(state);

    let addr: SocketAddr = format!("{host}:{port}").parse()?;
    println!(
        "dd-des-rs listening on http://{addr} (out dir: {})",
        work.join("out").display()
    );
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    tokio::time::sleep(Duration::from_millis(10)).await;
    Ok(())
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
    fn model_registry_and_streaming_solvers_are_exposed() {
        let kinds = with_builtins().kinds();
        for expected in ["mdp", "pomdp", "hybrid", "studio"] {
            assert!(kinds.contains(&expected.to_string()), "missing kind {expected}");
        }
        assert!(streaming_model_names().contains(&"lp"));
        assert!(streaming_model_names().contains(&"mdp"));
        assert!(
            streaming_contracts().len() >= 4,
            "expected lp/milp/mdp/pomdp streaming contracts"
        );
    }

    #[test]
    fn descriptor_advertises_model_and_streaming_endpoints() {
        let descriptor = build_descriptor();
        let paths: Vec<&str> = descriptor.endpoints.iter().map(|e| e.path.as_str()).collect();
        assert!(paths.contains(&"/models"));
        assert!(paths.contains(&"/models/:kind/run"));
        assert!(paths.contains(&"/streaming"));
        assert!(paths.contains(&"/streaming/:name"));
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
            content_type(StdPath::new("a.bin")),
            "application/octet-stream"
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
