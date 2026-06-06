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
//! - `GET /models` — first-class model registry with example specs.
//! - `GET /models/:kind/run` — run a kind's example spec and render an interactive player (`?format=json` for the raw artifact).
//! - `POST /models/:kind/run` — run a user-supplied JSON spec for a kind (renders a player; `?format=json` for the artifact).
//! - `GET /streaming` — JSONL streaming-solver contracts (lp, milp, mdp, pomdp, soccer-planner).
//! - `POST /streaming/:name` — stream JSONL commands to a solver; responds with a JSONL frame stream.
//! - `GET /soccer/planner` — interactive 11-a-side rotation planner UI.
//! - `POST /soccer/planner/solve` — re-solve the planner request with the Rust IP/MIP solver.
//! - `POST /soccer/planner/stream` — soccer planner JSONL stream alias.
//! - `GET /music` — generative music production workbench UI.
//! - `POST /music/sample-seed` — upload or link a 10-50s MP4 plus a prompt and render a WAV variation.
//!   Public and authenticated social/media links are supported via direct HTTP
//!   headers or `yt-dlp` cookies.
//! - `GET /delivery-planner.html` — friendly redirect to the delivery planner artifact.
//! - `GET /deliver-planner.html` — typo-compatible redirect to the delivery planner artifact.
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
    collections::{BTreeMap, BTreeSet},
    env, fs,
    net::{IpAddr, SocketAddr},
    panic::{catch_unwind, AssertUnwindSafe},
    path::{Path as StdPath, PathBuf},
    process::{Command, Stdio},
    sync::Arc,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use axum::{
    extract::{rejection::JsonRejection, DefaultBodyLimit, Multipart, Path, Query, State},
    http::{header, HeaderMap, HeaderName, HeaderValue, StatusCode},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::{io::AsyncWriteExt, sync::Mutex};

use des_engine::des::fel::elevator::{
    elevator_mdp_spec, elevator_pomdp_spec, render_elevator_html, run_fel_elevator, ElevatorConfig,
};
use des_engine::des::general::music_production::{
    analyze_music_sample_prompt, derive_music_sample_seed_from_mp4, generate_microtonal_song,
    song_spec_from_music_sample_seed_with_prompt, ArrangementSummary,
};
use des_engine::des::general::soccer::{run_default_simulation, SimulationTrace};
use des_engine::des::model::{with_builtins, CitizenError};
use des_engine::des::service::{
    Capability, DesExtension, EndpointKind, EngineCatalogExtension, ServiceBuilder,
    ServiceDescriptor, ServiceInfo, DD_API_DOCS_HEADER,
};
use des_engine::des::simulations::{run_simulations_matching, simulation_catalogue, SimOutcome};
use des_engine::des::soccer_planner::{
    planner_page_html, planner_response_to_json, solve_planner, PlannerRequest,
};
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
const MAX_MUSIC_UPLOAD_BYTES: usize = 96 * 1024 * 1024;
const MAX_MUSIC_SOURCE_URL_CHARS: usize = 4096;
const MAX_MUSIC_TITLE_CHARS: usize = 160;
const MAX_MUSIC_PROMPT_CHARS: usize = 12_000;
const MAX_MUSIC_AUTH_CHARS: usize = 32_000;
const MAX_MUSIC_AUTH_HEADER_NAME_CHARS: usize = 64;
const MAX_MUSIC_COOKIE_BYTES: usize = 512 * 1024;
const MUSIC_DOWNLOAD_TIMEOUT_SECS: u64 = 180;
const MAX_FILTER_LEN: usize = 96;
const MAX_SIMULATE_MATCHES: usize = 8;
const SOCCER_PLANNER_HTTP_SOLVE_BUDGET_MS: f64 = 90_000.0;

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
.sim .row{display:flex;align-items:center;gap:8px;justify-content:flex-end;flex-wrap:wrap;margin-top:2px}
.sim .links{display:flex;align-items:center;gap:6px;flex-wrap:wrap}
.sim .links:empty{display:none}
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
.st{font-size:.8rem;color:#9aa4b2;min-height:1.1em;flex:1;min-width:86px}
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
    <a class="btn primary" href="out/">All rendered results &rarr;</a>
    <a class="btn" href="docs/api">API docs</a>
    <a class="btn" href="info">Service info</a>
    <a class="btn" href="models">Models JSON</a>
    <a class="btn" href="streaming">Streaming JSON</a>
    <a class="btn" href="music">Music production</a>
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
<h2>Soccer <span class="muted">— videogame, learning sim, rotation planner</span></h2>
<div class="grid">
  <div class="sim feat">
    <div class="label">Soccer videogame</div>
    <div class="name">out/soccer-sim.html &middot; json/jsonl</div>
    <div class="desc">Playable 2D 11v11 match artifact with MDP/POMDP player learning, ball physics, possession chains, shots, officials, and controller slots.</div>
    <div class="row">
      <a class="open" href="out/soccer-sim.html" target="_blank" rel="noopener">Open game &#8599;</a>
      <a class="open" href="out/soccer-sim.json" target="_blank" rel="noopener">Trace JSON &#8599;</a>
      <a class="open" href="out/soccer-sim.frames.jsonl" target="_blank" rel="noopener">Frames JSONL &#8599;</a>
    </div>
  </div>
  <div class="sim feat">
    <div class="label">Interactive planner</div>
    <div class="name">soccer/planner</div>
    <div class="desc">11-a-side (4-4-2), max 7 subs. Mark players AWOL/injured/guest, lock positions, ban roles, set per-position scores and chemistry rules (9/10 if partner in slot Y). Re-solve with IP/MIP; toggle Pitch vs solver view.</div>
    <div class="row"><a class="open" href="soccer/planner" target="_blank" rel="noopener">Open planner &#8599;</a></div>
  </div>
</div>
<h2>Music production <span class="muted">— microtonal generator, breakbeat/DnB album runs, sample-seed workflow</span></h2>
<div class="grid">
  <div class="sim feat">
    <div class="label">Generative song workbench</div>
    <div class="name">music-production</div>
    <div class="desc">Build 3-minute instrumental albums with FFT-backed analysis, synthetic instrument discovery, meter changes, drum fills, reduced percussion gain, and a 10-50s MP4 music-sample-seed variation path.</div>
    <div class="row"><a class="open" href="music" target="_blank" rel="noopener">Open workbench &#8599;</a></div>
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
const FEATURED=[["main_factory_floor_track3t","Track3t warehouse"],["main_build_site","Build site index"],["main_elevator_highrise","Elevator high-rise"],["main_factmachine_markets","FactMachine markets"],["main_two_disease","Two-disease epidemic"],["main_electric_circuit","Electric circuit"],["main_traffic","Traffic network"],["main_court_mdp","Court MDP"],["main_convolution","Convolution"]];
const CONTROL=[
  ["main_shadow_eval","Shadow Gramians","Probe each plant as a black box: recover controllability/observability Gramians from perturbed shadow copies, cross-check against the analytic model, then re-ask via a nested MDP/POMDP of the motor's speed regimes."],
  ["main_observability_controllability_anim","Obs / ctrl (animated)","Kalman rank tests for controllability & observability of a state-space model, animated step by step."],
  ["main_empirical_control_report","Empirical control report","Monte-Carlo trials + Gramian eigenvalue degrees that quantify how much control and observation authority a system actually has."],
  ["main_dc_motor_anim","DC motor (back-EMF)","Separately-excited DC motor with explicit back-EMF coupling (E = K_e·ω), RK4-integrated and animated."],
  ["main_wind_mppt_anim","Wind MPPT","Maximum-power-point-tracking controller on a wind turbine, animated."]
];
function toast(html){const t=document.getElementById('toast');t.innerHTML=html;t.classList.add('show');clearTimeout(window.__tt);window.__tt=setTimeout(function(){t.classList.remove('show');},6000);}
function esc(s){return String(s||'').replace(/[<>&"]/g,function(ch){return {'<':'&lt;','>':'&gt;','&':'&amp;','"':'&quot;'}[ch];});}
function shortName(href){return String(href||'').split('/').filter(Boolean).pop()||href;}
function artifactAnchor(href,label,cls){return '<a class="'+cls+'" href="'+esc(href)+'" target="_blank" rel="noopener">'+esc(label)+' &#8599;</a>';}
function artifactButtons(artifacts){
  artifacts=artifacts||{};
  const html=(artifacts.html||[]).slice(0,3);
  const json=(artifacts.json||[]).slice(0,3);
  const jsonl=(artifacts.jsonl||[]).slice(0,3);
  const out=[];
  html.forEach(function(h,i){out.push(artifactAnchor(h,i===0?'View results':shortName(h),'open'));});
  json.forEach(function(h){out.push(artifactAnchor(h,'JSON','json'));});
  jsonl.forEach(function(h){out.push(artifactAnchor(h,'JSONL','json'));});
  return out.join('');
}
function setArtifactLinks(el,artifacts){
  if(!el)return;
  el.innerHTML=artifactButtons(artifacts);
}
function simCard(name,label,desc,feat){
  const card=document.createElement('div');card.className=feat?'sim feat':'sim';card.dataset.name=name;
  const lab=document.createElement('div');lab.className='label';lab.textContent=label||name;
  const nm=document.createElement('div');nm.className='name';nm.textContent=name;
  card.appendChild(lab);card.appendChild(nm);
  if(desc){const d=document.createElement('div');d.className='desc';d.textContent=desc;card.appendChild(d);}
  const row=document.createElement('div');row.className='row';
  const st=document.createElement('span');st.className='st';
  const links=document.createElement('span');links.className='links';
  const btn=document.createElement('button');btn.className='run';btn.textContent='Run';
  btn.onclick=function(){run(name,btn,st,links);};
  row.appendChild(st);row.appendChild(links);row.appendChild(btn);
  card.appendChild(row);
  return card;
}
async function run(name,btn,st,links){
  btn.disabled=true;const old=btn.textContent;btn.textContent='Running…';st.className='st';st.textContent='running…';
  setArtifactLinks(links,null);
  try{
    const r=await fetch('simulations/'+encodeURIComponent(name)+'/run?exact=1');
    const d=await r.json();
    const o=(d.ran&&d.ran[0])||{};
    if(d.ok){
      const artifacts=o.artifacts||d.artifacts||{};
      const primary=artifacts.primary||'out/';
      setArtifactLinks(links,artifacts);
      st.className='st ok';st.textContent='\u2713 '+(o.millis!=null?o.millis+' ms':'done');
      const buttons=artifactButtons(artifacts)||('<a href="'+esc(primary)+'">view results &rarr;</a>');
      toast('Ran <code>'+esc(name)+'</code> — '+buttons);
    }
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

const MUSIC_PRODUCTION_HTML: &str = r####"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>DES music production</title>
<style>
:root{color-scheme:dark;--bg:#090d16;--panel:#101724;--panel2:#151b24;--line:#273140;--ink:#eef3f8;--dim:#9ba7b5;--accent:#24a0ed;--hot:#e65f7a;--ok:#39d98a}
*{box-sizing:border-box}
body{margin:0;background:linear-gradient(180deg,#090d16,#10151f 48%,#0c1018);color:var(--ink);font-family:system-ui,-apple-system,"Segoe UI",Roboto,sans-serif}
main{max-width:1180px;margin:0 auto;padding:24px 18px 56px}
.top{display:flex;align-items:flex-start;justify-content:space-between;gap:14px;margin-bottom:18px}
.crumb{color:var(--dim);text-decoration:none;font-size:.86rem}
h1{font-size:1.65rem;margin:3px 0 7px;letter-spacing:0}
.sub{max-width:76ch;color:var(--dim);line-height:1.5;margin:0}
.pill{display:inline-flex;align-items:center;gap:7px;border:1px solid var(--line);border-radius:999px;padding:6px 10px;background:#0e141e;color:#b9c6d4;font-size:.78rem;white-space:nowrap}
.dot{width:8px;height:8px;border-radius:50%;background:var(--ok)}
.layout{display:grid;grid-template-columns:minmax(280px,360px) 1fr;gap:14px;align-items:start}
.panel{border:1px solid var(--line);background:var(--panel);border-radius:12px;padding:15px}
.panel h2{font-size:.96rem;margin:0 0 12px;color:#dce7f2;letter-spacing:0}
label{display:block;color:#bac5d1;font-size:.8rem;margin:12px 0 5px}
input,select,textarea{width:100%;font:inherit;color:var(--ink);background:#0b111b;border:1px solid #2d3949;border-radius:8px;padding:9px 10px}
textarea{min-height:108px;resize:vertical}
input[type=range]{padding:0;accent-color:var(--accent)}
.row{display:grid;grid-template-columns:1fr 1fr;gap:10px}
.actions{display:flex;gap:9px;flex-wrap:wrap;margin-top:14px}
button,a.btn{font:inherit;font-size:.86rem;border-radius:8px;border:1px solid #344153;background:#151d29;color:#eef3f8;padding:9px 12px;text-decoration:none;cursor:pointer}
button.primary{background:var(--accent);border-color:var(--accent);color:#041018}
button:hover,a.btn:hover{border-color:#58b8f2}
.meters{display:grid;grid-template-columns:repeat(4,minmax(0,1fr));gap:10px;margin-bottom:14px}
.meter{border:1px solid var(--line);background:var(--panel2);border-radius:10px;padding:12px;min-height:74px}
.meter b{display:block;font-size:1.12rem;margin-bottom:5px;color:#fff}
.meter span{font-size:.75rem;color:var(--dim)}
.wide{display:grid;grid-template-columns:1fr 1fr;gap:14px}
pre{margin:0;white-space:pre-wrap;word-break:break-word;background:#080d14;border:1px solid #263242;border-radius:10px;padding:13px;color:#c9e6ff;font-size:.82rem;line-height:1.45;min-height:132px}
.timeline{height:76px;border:1px solid var(--line);border-radius:10px;background:#0b111b;position:relative;overflow:hidden;margin-top:6px}
.section{position:absolute;top:0;bottom:0;border-right:1px solid rgba(255,255,255,.18)}
.section:nth-child(1){background:#1b4d89}.section:nth-child(2){background:#75415f}.section:nth-child(3){background:#2d705d}.section:nth-child(4){background:#725d24}.section:nth-child(5){background:#41386b}
.section span{position:absolute;left:8px;bottom:7px;font-size:.7rem;color:#f3f7fb}
.seed-status{font-size:.8rem;color:var(--dim);min-height:1.2em;margin-top:8px}
.seed-status.ok{color:var(--ok)}.seed-status.err{color:var(--hot)}
.auth-panel{border:1px solid var(--line);background:#0c131d;border-radius:8px;padding:10px;margin-top:10px}
.auth-panel[hidden]{display:none}
.auth-panel textarea{min-height:74px}
.result{font-size:.8rem;color:#c9e6ff;line-height:1.45;margin-top:10px;word-break:break-word}
.result a{color:#76c7ff}
.result.ok{color:var(--ok)}.result.err{color:var(--hot)}
.note{font-size:.78rem;color:var(--dim);line-height:1.45;margin-top:10px}
@media(max-width:860px){.layout,.wide{grid-template-columns:1fr}.meters{grid-template-columns:1fr 1fr}.top{flex-direction:column}}
</style>
</head>
<body>
<main>
  <div class="top">
    <div>
      <a class="crumb" href="./">des-rs</a>
      <h1>Music Production Workbench</h1>
      <p class="sub">Microtonal, mostly instrumental generation with synthetic instruments, FFT spectrum checks, breakbeat and drum-n-bass album recipes, richer meter changes, legal sample provenance, and a 10-50 second MP4/link music-sample-seed path.</p>
    </div>
    <span class="pill"><span class="dot"></span>ready for local renders</span>
  </div>

  <div class="layout">
    <section class="panel">
      <h2>Render Setup</h2>
      <label for="mode">Mode</label>
      <select id="mode">
        <option value="album-more">10-track breakbeat/DnB album</option>
        <option value="album">10-track broad microtonal album</option>
        <option value="sample">MP4 music-sample-seed variation</option>
      </select>
      <div class="row">
        <div>
          <label for="seed">Seed</label>
          <input id="seed" value="20260602" inputmode="numeric">
        </div>
        <div>
          <label for="duration">Song seconds</label>
          <input id="duration" type="number" min="30" max="480" step="1" value="180">
        </div>
      </div>
      <label for="percussion">Main percussion gain</label>
      <input id="percussion" type="range" min="80" max="90" value="84">
	      <label for="variation">Drum variation target</label>
	      <input id="variation" type="range" min="10" max="20" value="10">
	      <label for="sourceUrl">Link seed</label>
	      <input id="sourceUrl" type="url" placeholder="https://www.youtube.com/watch?v=... or https://x.com/...">
	      <div class="row">
	        <div>
	          <label for="sourceAccess">Link access</label>
	          <select id="sourceAccess">
	            <option value="public">Public</option>
	            <option value="authenticated">Authenticated</option>
	          </select>
	        </div>
	        <div>
	          <label for="sourcePlatform">Source</label>
	          <select id="sourcePlatform">
	            <option value="auto">Auto</option>
	            <option value="youtube">YouTube</option>
	            <option value="x">X.com</option>
	            <option value="facebook">Facebook</option>
	            <option value="instagram">Instagram</option>
	            <option value="direct">Direct media</option>
	          </select>
	        </div>
	      </div>
	      <div id="authPanel" class="auth-panel" hidden>
	        <label for="authHeaderName">Source auth header name</label>
	        <input id="authHeaderName" autocomplete="off" spellcheck="false" value="Auth">
	        <label for="authHeader">Source auth header value</label>
	        <input id="authHeader" type="password" autocomplete="off" spellcheck="false" placeholder="shared secret or bearer token">
	        <label for="cookieHeader">Cookie header</label>
	        <textarea id="cookieHeader" autocomplete="off" spellcheck="false" placeholder="name=value; name2=value2"></textarea>
	        <label for="sourceCookies">yt-dlp cookies.txt</label>
	        <input id="sourceCookies" type="file" accept=".txt,text/plain">
	      </div>
	      <label for="sample">MP4 seed upload (10-50s)</label>
	      <input id="sample" type="file" accept="video/mp4,audio/mp4,.mp4">
      <div id="sampleStatus" class="seed-status"></div>
      <label for="prompt">Prompt / direction</label>
      <textarea id="prompt" placeholder="Expand the seed into faster jungle, keep the melody bright, use 13/16 stutter fills, soften the main drums, add massive synth pressure."></textarea>
      <div class="actions">
        <button class="primary" onclick="renderSampleSeed()">Render via server</button>
        <button onclick="update()">Refresh recipe</button>
        <button onclick="copyCommand()">Copy command</button>
        <a class="btn" href="out/" target="_blank" rel="noopener">Open output</a>
      </div>
	      <div id="serverResult" class="result"></div>
	      <p class="note">Use sources you own or are licensed to transform. Links use direct HTTP for media files and yt-dlp when available for YouTube, Facebook, Instagram, X, and similar pages.</p>
    </section>

    <div>
      <div class="meters">
        <div class="meter"><b id="tracks">10</b><span>tracks</span></div>
        <div class="meter"><b id="meter">7/8+</b><span>meter changes</span></div>
        <div class="meter"><b id="drums">10%</b><span>less repetition target</span></div>
        <div class="meter"><b id="gain">0.84</b><span>percussion gain</span></div>
      </div>
      <section class="panel">
        <h2>Song Shape</h2>
        <div class="timeline" aria-label="song timeline">
          <div class="section" style="left:0;width:16%"><span>intro</span></div>
          <div class="section" style="left:16%;width:22%"><span>pressure</span></div>
          <div class="section" style="left:38%;width:28%"><span>collage</span></div>
          <div class="section" style="left:66%;width:20%"><span>swerve</span></div>
          <div class="section" style="left:86%;width:14%"><span>outro</span></div>
        </div>
      </section>
      <div class="wide" style="margin-top:14px">
        <section class="panel">
          <h2>Command</h2>
          <pre id="command"></pre>
        </section>
        <section class="panel">
          <h2>Manifest Preview</h2>
          <pre id="manifest"></pre>
        </section>
      </div>
    </div>
  </div>
</main>
<script>
const $=id=>document.getElementById(id);
let sampleOk=false;
let sampleName="";
const savedPrompt=localStorage.getItem("desMusicPrompt")||"";
$("prompt").value=savedPrompt;
function clampInt(value,fallback){const n=parseInt(value,10);return Number.isFinite(n)?n:fallback;}
function shellQuote(value){return "'"+String(value).replace(/'/g,"'\\''")+"'";}
function hashText(value){
  let h=0x811c9dc5;
  for(const ch of value){h^=ch.charCodeAt(0);h=Math.imul(h,0x01000193)>>>0;}
  return h>>>0;
}
function promptTags(value){
  const l=value.toLowerCase();
  const tags=[];
  [["expand",["expand","longer arc","build out"]],["alter",["alter","mutate","transform"]],["slice",["slice","chop","cut-up","collage"]],["melody",["melody","melodic","hook","theme"]],["massive-synth",["massive synth","big synth","wall of synth"]],["space",["space","reverb","wide","dub"]],["less-drums",["less drums","softer drums","lower drums"]],["more-drums",["more drums","drum fills","busier drums"]]].forEach(([tag,words])=>{if(words.some(w=>l.includes(w)))tags.push(tag);});
  return tags;
}
function promptText(){return $("prompt").value.trim();}
function sourceUrl(){return $("sourceUrl").value.trim();}
function sourceAccess(){return $("sourceAccess").value;}
function sourcePlatform(){return $("sourcePlatform").value;}
function authCredentials(){
  const cookieFile=$("sourceCookies").files&&$("sourceCookies").files[0];
  const authHeaderName=($("authHeaderName").value.trim()||"Auth");
  const authHeader=$("authHeader").value.trim();
  const cookieHeader=$("cookieHeader").value.trim();
  return {
    auth_header_name: authHeaderName,
    auth_header: Boolean(authHeader),
    cookie_header: Boolean(cookieHeader),
    cookies_file: cookieFile ? cookieFile.name : null,
    has: Boolean(authHeader||cookieHeader||cookieFile)
  };
}
function updateAuthVisibility(){
  $("authPanel").hidden=sourceAccess()!=="authenticated";
}
function command(){
  const mode=$("mode").value;
  const duration=clampInt($("duration").value,180);
  const seed=clampInt($("seed").value,20260602);
  if(mode==="sample"){
    const url=sourceUrl();
    const source=url?"out/music-sample-seed-source.mp4":(sampleName||"/absolute/path/to/seed.mp4");
    const prompt=promptText();
    const access=sourceAccess();
    const cookieFile=$("sourceCookies").files&&$("sourceCookies").files[0];
    const authHeaderName=($("authHeaderName").value.trim()||"Auth");
    const authHeader=$("authHeader").value.trim();
    const promptPath="out/music-sample-seed-prompt.txt";
    const cookieFlag=access==="authenticated"?` --cookies "${cookieFile?cookieFile.name:"/absolute/path/to/cookies.txt"}"`:"";
    const headerFlag=access==="authenticated"&&authHeader?` --add-header ${shellQuote(authHeaderName+": "+authHeader)}`:"";
    const urlPrefix=url?`mkdir -p out\nyt-dlp --no-playlist --force-overwrites --merge-output-format mp4${cookieFlag}${headerFlag} -o ${source} ${shellQuote(url)}\n`:"";
    const promptPrefix=prompt?`mkdir -p out\nprintf %s ${shellQuote(prompt)} > ${promptPath}\n`:"";
    const promptFlag=prompt?` --prompt-file ${promptPath}`:"";
    return `${urlPrefix}${promptPrefix}cargo run --bin main_music_production -- --sample-seed "${source}" out/music-sample-seed-variation.wav ${duration}${promptFlag}`;
  }
  const out=mode==="album-more"?"out/music-production-ten-more-breaks":"out/music-production-ten-songs";
  const flag=mode==="album-more"?"--album-more":"--album";
  return `cargo run --bin main_music_production -- ${flag} ${out} ${seed} ${duration}`;
}
function update(){
  updateAuthVisibility();
  const mode=$("mode").value;
  const duration=clampInt($("duration").value,180);
  const percussion=(clampInt($("percussion").value,84)/100).toFixed(2);
  const variation=clampInt($("variation").value,10);
  const prompt=promptText();
  const url=sourceUrl();
  const access=sourceAccess();
  const auth=authCredentials();
  localStorage.setItem("desMusicPrompt", $("prompt").value);
  $("tracks").textContent=mode==="sample"?"1":"10";
  $("drums").textContent=variation+"%";
  $("gain").textContent=percussion;
  $("meter").textContent=mode==="sample"?"seeded":"7/8+";
  $("command").textContent=command();
  $("manifest").textContent=JSON.stringify({
    mode,
    duration_seconds: duration,
    percussion_gain: Number(percussion),
    drum_repetition_reduction_target: variation/100,
    synthesis: ["microtonal", "pitch-bend", "FFT spectrum", "invented instruments"],
    http_endpoint: mode==="sample" ? "POST music/sample-seed" : null,
    sample_seed: mode==="sample" ? {
      required_seconds: "10-50",
      valid_loaded_file: sampleOk,
      file: sampleName || null,
      source_url: url || null,
      source_platform: sourcePlatform(),
      access,
      authenticated: access==="authenticated",
      auth: access==="authenticated" ? {
        auth_header_name: auth.auth_header_name,
        auth_header: auth.auth_header,
        cookie_header: auth.cookie_header,
        cookies_file: auth.cookies_file
      } : null
    } : null,
    prompt: prompt ? { chars: [...prompt].length, hash: hashText(prompt), tags: promptTags(prompt) } : null
  }, null, 2);
}
$("sample").addEventListener("change", function(){
  const file=this.files&&this.files[0];
  sampleOk=false;sampleName=file?file.name:"";
  const status=$("sampleStatus");
  if(!file){status.className="seed-status";status.textContent="";update();return;}
  const url=URL.createObjectURL(file);
  const video=document.createElement("video");
  video.preload="metadata";
  video.onloadedmetadata=function(){
    URL.revokeObjectURL(url);
    const d=video.duration||0;
    sampleOk=d>=10&&d<=50;
    status.className="seed-status "+(sampleOk?"ok":"err");
    status.textContent=sampleOk?`loaded ${file.name} (${d.toFixed(2)}s)`:`${file.name} is ${d.toFixed(2)}s; expected 10-50s`;
    update();
  };
  video.onerror=function(){URL.revokeObjectURL(url);status.className="seed-status err";status.textContent="could not read MP4 metadata";update();};
  video.src=url;
});
["mode","seed","duration","percussion","variation","prompt","sourceUrl","sourceAccess","sourcePlatform","authHeaderName","authHeader","cookieHeader"].forEach(id=>$(id).addEventListener("input",update));
$("sourceCookies").addEventListener("change",update);
async function copyCommand(){
  const text=command();
  try{await navigator.clipboard.writeText(text);$("command").textContent=text+"\n\ncopied";}
  catch(e){$("command").textContent=text;}
}
function escapeHtml(value){return String(value).replace(/[&<>"']/g,ch=>({"&":"&amp;","<":"&lt;",">":"&gt;","\"":"&quot;","'":"&#39;"}[ch]));}
async function renderSampleSeed(){
  const result=$("serverResult");
  const file=$("sample").files&&$("sample").files[0];
  const url=sourceUrl();
  if($("mode").value!=="sample"){
    result.className="result err";
    result.textContent="Switch mode to MP4 music-sample-seed variation.";
    return;
  }
  if(!file&&!url){
    result.className="result err";
    result.textContent="Choose a 10-50s MP4 seed or paste a public media link first.";
    return;
  }
  const access=sourceAccess();
  const auth=authCredentials();
  if(url&&access==="authenticated"&&!auth.has){
    result.className="result err";
    result.textContent="Add an Authorization header, Cookie header, or yt-dlp cookies.txt for an authenticated link.";
    return;
  }
  const fd=new FormData();
  if(file) fd.append("sample",file,file.name);
  if(url) fd.append("source_url",url);
  fd.append("source_auth_mode",access);
  fd.append("source_platform",sourcePlatform());
  const authHeaderName=($("authHeaderName").value.trim()||"Auth");
  const authHeader=$("authHeader").value.trim();
  const cookieHeader=$("cookieHeader").value.trim();
  const cookieFile=$("sourceCookies").files&&$("sourceCookies").files[0];
  if(authHeaderName) fd.append("source_auth_header_name",authHeaderName);
  if(authHeader) fd.append("source_auth_header",authHeader);
  if(cookieHeader) fd.append("source_cookie_header",cookieHeader);
  if(cookieFile) fd.append("source_cookies",cookieFile,cookieFile.name);
  fd.append("prompt",$("prompt").value);
  fd.append("duration_seconds",String(clampInt($("duration").value,180)));
  fd.append("title","music-sample-seed variation");
  result.className="result";
  result.textContent="rendering on des-rs...";
  try{
    const headers={};
    if(authHeader&&authHeaderName.toLowerCase()==="auth") headers.Auth=authHeader;
    const r=await fetch("music/sample-seed",{method:"POST",body:fd,headers});
    const d=await r.json();
    if(!r.ok||!d.ok){throw new Error(d.error||("HTTP "+r.status));}
    result.className="result ok";
    result.innerHTML=`Wrote <a href="${escapeHtml(d.wav_url)}" target="_blank" rel="noopener">${escapeHtml(d.wav_url)}</a><br>genre ${escapeHtml(d.summary.genre)} · bpm ${Number(d.summary.bpm).toFixed(1)} · prompt hash ${d.prompt&&d.prompt.hash?d.prompt.hash:"none"}`;
    $("manifest").textContent=JSON.stringify(d,null,2);
  }catch(e){
    result.className="result err";
    result.textContent="render failed: "+e.message;
  }
}
update();
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
    /// Interactive 11-a-side rotation planner (roster constraints + re-solve).
    soccer_planner_html: Arc<str>,
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

fn env_value_or_empty(key: &str, fallback: &str) -> String {
    env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .unwrap_or_else(|| fallback.to_string())
}

/// All simulation names from the engine catalogue, in catalogue order.
fn sim_names() -> Vec<&'static str> {
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
enum SimMatchError {
    NoMatches,
    TooMany {
        count: usize,
        preview: Vec<&'static str>,
    },
}

fn checked_sim_names(needle: &str, exact: bool) -> Result<Vec<&'static str>, SimMatchError> {
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
            "soccer-sim.json",
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
                && matches!(*rel, SOCCER_SIM_TRACE_JSON | SOCCER_SIM_FRAMES_JSONL);
            if after.contains_key(*rel) || lazy_soccer_trace {
                rels.insert((*rel).to_string());
            }
        }
    }
    rels.into_iter().collect()
}

fn artifact_ext(rel: &str) -> Option<&str> {
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

async fn music_production_page(State(state): State<AppState>) -> Response {
    let mut res = Html(MUSIC_PRODUCTION_HTML).into_response();
    apply_discovery_headers(res.headers_mut(), &state);
    res
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MusicSourceAuthMode {
    Public,
    Authenticated,
}

impl MusicSourceAuthMode {
    fn as_str(self) -> &'static str {
        match self {
            MusicSourceAuthMode::Public => "public",
            MusicSourceAuthMode::Authenticated => "authenticated",
        }
    }
}

#[derive(Clone, Debug)]
struct MusicSourceAuth {
    mode: MusicSourceAuthMode,
    auth_header_name: Option<HeaderName>,
    auth_header: Option<String>,
    cookie_header: Option<String>,
    cookies_file: Option<PathBuf>,
}

impl MusicSourceAuth {
    fn has_credentials(&self) -> bool {
        self.auth_header.is_some() || self.cookie_header.is_some() || self.cookies_file.is_some()
    }

    fn effective_mode(&self) -> MusicSourceAuthMode {
        if self.mode == MusicSourceAuthMode::Authenticated || self.has_credentials() {
            MusicSourceAuthMode::Authenticated
        } else {
            MusicSourceAuthMode::Public
        }
    }

    fn summary_json(&self) -> Value {
        json!({
            "mode": self.effective_mode().as_str(),
            "auth_header": self.auth_header.as_ref().map(|_| {
                self.auth_header_name
                    .as_ref()
                    .map(|name| name.as_str())
                    .unwrap_or(header::AUTHORIZATION.as_str())
            }),
            "cookie_header": self.cookie_header.is_some(),
            "cookies_file": self.cookies_file.is_some(),
        })
    }
}

fn parse_music_source_auth_mode(raw: &str) -> Result<MusicSourceAuthMode, String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "" | "public" => Ok(MusicSourceAuthMode::Public),
        "authenticated" | "auth" | "private" => Ok(MusicSourceAuthMode::Authenticated),
        other => Err(format!(
            "source_auth_mode must be public or authenticated, got {other:?}"
        )),
    }
}

fn clean_music_auth_field(raw: String, label: &str) -> Result<Option<String>, String> {
    let value = raw.trim().to_string();
    if value.is_empty() {
        return Ok(None);
    }
    if value.chars().count() > MAX_MUSIC_AUTH_CHARS {
        return Err(format!(
            "{label} must be at most {MAX_MUSIC_AUTH_CHARS} characters"
        ));
    }
    if value.chars().any(|ch| ch.is_control()) {
        return Err(format!("{label} must be a single HTTP header value"));
    }
    Ok(Some(value))
}

fn clean_music_auth_header_name_field(raw: String) -> Result<Option<HeaderName>, String> {
    let value = raw.trim();
    if value.is_empty() {
        return Ok(None);
    }
    if value.chars().count() > MAX_MUSIC_AUTH_HEADER_NAME_CHARS {
        return Err(format!(
            "source_auth_header_name must be at most {MAX_MUSIC_AUTH_HEADER_NAME_CHARS} characters"
        ));
    }
    HeaderName::from_bytes(value.as_bytes())
        .map(Some)
        .map_err(|e| format!("invalid source_auth_header_name: {e}"))
}

fn clean_music_source_url_field(raw: String) -> Result<Option<String>, String> {
    let value = raw.trim().to_string();
    if value.is_empty() {
        return Ok(None);
    }
    if value.chars().count() > MAX_MUSIC_SOURCE_URL_CHARS {
        return Err(format!(
            "source_url must be at most {MAX_MUSIC_SOURCE_URL_CHARS} characters"
        ));
    }
    Ok(Some(value))
}

fn redacted_source_url(raw: Option<&String>) -> Option<String> {
    raw.map(|value| redacted_source_url_value(value))
}

fn redacted_source_url_value(value: &str) -> String {
    match reqwest::Url::parse(value) {
        Ok(mut url) => {
            let _ = url.set_username("");
            let _ = url.set_password(None);
            if url.query().is_some() {
                url.set_query(Some("redacted=1"));
            }
            url.to_string()
        }
        Err(_) => "<invalid-url>".to_string(),
    }
}

fn sanitize_url_in_error(value: &str, raw_url: &str, redacted_url: &str) -> String {
    if raw_url == redacted_url {
        value.to_string()
    } else {
        value.replace(raw_url, redacted_url)
    }
}

async fn music_sample_seed_render(
    State(state): State<AppState>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Response {
    let mut sample_bytes: Option<Vec<u8>> = None;
    let mut source_url: Option<String> = None;
    let mut source_auth_mode = MusicSourceAuthMode::Public;
    let mut source_auth_header_name: Option<HeaderName> = None;
    let mut source_auth_header: Option<String> = None;
    let mut source_cookie_header: Option<String> = None;
    let mut source_cookies: Option<Vec<u8>> = None;
    let mut prompt = String::new();
    let mut title = "music-sample-seed variation".to_string();
    let mut duration_seconds = 180.0;

    while let Some(field) = match multipart.next_field().await {
        Ok(field) => field,
        Err(e) => {
            return json_error(
                StatusCode::BAD_REQUEST,
                format!("invalid multipart body: {e}"),
            )
        }
    } {
        let name = field.name().unwrap_or("").to_string();
        match name.as_str() {
            "sample" => match field.bytes().await {
                Ok(bytes) => sample_bytes = Some(bytes.to_vec()),
                Err(e) => {
                    return json_error(
                        StatusCode::BAD_REQUEST,
                        format!("failed to read sample upload: {e}"),
                    )
                }
            },
            "source_url" => match field.text().await {
                Ok(text) => match clean_music_source_url_field(text) {
                    Ok(value) => source_url = value,
                    Err(e) => return json_error(StatusCode::BAD_REQUEST, e),
                },
                Err(e) => {
                    return json_error(
                        StatusCode::BAD_REQUEST,
                        format!("failed to read source_url: {e}"),
                    )
                }
            },
            "source_auth_mode" | "auth_mode" | "source_access" => match field.text().await {
                Ok(text) => match parse_music_source_auth_mode(&text) {
                    Ok(mode) => source_auth_mode = mode,
                    Err(e) => return json_error(StatusCode::BAD_REQUEST, e),
                },
                Err(e) => {
                    return json_error(
                        StatusCode::BAD_REQUEST,
                        format!("failed to read source_auth_mode: {e}"),
                    )
                }
            },
            "source_auth_header" | "auth_header" | "authorization" => match field.text().await {
                Ok(text) => match clean_music_auth_field(text, "source_auth_header") {
                    Ok(value) => source_auth_header = value,
                    Err(e) => return json_error(StatusCode::BAD_REQUEST, e),
                },
                Err(e) => {
                    return json_error(
                        StatusCode::BAD_REQUEST,
                        format!("failed to read source_auth_header: {e}"),
                    )
                }
            },
            "source_auth_header_name" | "auth_header_name" | "authorization_header_name" => {
                match field.text().await {
                    Ok(text) => match clean_music_auth_header_name_field(text) {
                        Ok(value) => source_auth_header_name = value,
                        Err(e) => return json_error(StatusCode::BAD_REQUEST, e),
                    },
                    Err(e) => {
                        return json_error(
                            StatusCode::BAD_REQUEST,
                            format!("failed to read source_auth_header_name: {e}"),
                        )
                    }
                }
            }
            "source_cookie_header" | "cookie_header" => match field.text().await {
                Ok(text) => match clean_music_auth_field(text, "source_cookie_header") {
                    Ok(value) => source_cookie_header = value,
                    Err(e) => return json_error(StatusCode::BAD_REQUEST, e),
                },
                Err(e) => {
                    return json_error(
                        StatusCode::BAD_REQUEST,
                        format!("failed to read source_cookie_header: {e}"),
                    )
                }
            },
            "source_cookies" | "auth_cookies" | "cookies" => match field.bytes().await {
                Ok(bytes) if bytes.is_empty() => {}
                Ok(bytes) if bytes.len() <= MAX_MUSIC_COOKIE_BYTES => {
                    source_cookies = Some(bytes.to_vec())
                }
                Ok(bytes) => {
                    return json_error(
                        StatusCode::BAD_REQUEST,
                        format!(
                            "source_cookies is too large ({} bytes; max {MAX_MUSIC_COOKIE_BYTES})",
                            bytes.len()
                        ),
                    )
                }
                Err(e) => {
                    return json_error(
                        StatusCode::BAD_REQUEST,
                        format!("failed to read source_cookies: {e}"),
                    )
                }
            },
            "prompt" => match field.text().await {
                Ok(text) => prompt = text,
                Err(e) => {
                    return json_error(
                        StatusCode::BAD_REQUEST,
                        format!("failed to read prompt: {e}"),
                    )
                }
            },
            "title" => match field.text().await {
                Ok(text) if !text.trim().is_empty() => title = text.trim().to_string(),
                Ok(_) => {}
                Err(e) => {
                    return json_error(
                        StatusCode::BAD_REQUEST,
                        format!("failed to read title: {e}"),
                    )
                }
            },
            "duration_seconds" => match field.text().await {
                Ok(text) => match text.trim().parse::<f64>() {
                    Ok(value) if (15.0..=240.0).contains(&value) => duration_seconds = value,
                    Ok(_) => {
                        return json_error(
                            StatusCode::BAD_REQUEST,
                            "duration_seconds must be between 15 and 240".to_string(),
                        )
                    }
                    Err(e) => {
                        return json_error(
                            StatusCode::BAD_REQUEST,
                            format!("invalid duration_seconds: {e}"),
                        )
                    }
                },
                Err(e) => {
                    return json_error(
                        StatusCode::BAD_REQUEST,
                        format!("failed to read duration_seconds: {e}"),
                    )
                }
            },
            _ => {}
        }
    }

    if prompt.chars().count() > MAX_MUSIC_PROMPT_CHARS {
        return json_error(
            StatusCode::BAD_REQUEST,
            format!("prompt must be at most {MAX_MUSIC_PROMPT_CHARS} characters"),
        );
    }
    if title.chars().count() > MAX_MUSIC_TITLE_CHARS {
        return json_error(
            StatusCode::BAD_REQUEST,
            format!("title must be at most {MAX_MUSIC_TITLE_CHARS} characters"),
        );
    }

    if source_auth_header.is_none() {
        let auth_header_name = HeaderName::from_static("auth");
        if let Some(value) = headers.get(&auth_header_name) {
            let value = match value.to_str() {
                Ok(value) => value.to_string(),
                Err(e) => {
                    return json_error(
                        StatusCode::BAD_REQUEST,
                        format!("invalid Auth request header: {e}"),
                    )
                }
            };
            match clean_music_auth_field(value, "Auth request header") {
                Ok(Some(value)) => {
                    source_auth_header = Some(value);
                    source_auth_header_name = Some(auth_header_name);
                }
                Ok(None) => {}
                Err(e) => return json_error(StatusCode::BAD_REQUEST, e),
            }
        }
    }

    let now = now_ms();
    let upload_dir = env::temp_dir().join("dd-des-rs-music-uploads");
    if let Err(e) = fs::create_dir_all(&upload_dir) {
        return json_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to create upload dir: {e}"),
        );
    }
    let upload_path = upload_dir.join(format!("music-sample-seed-{now}.mp4"));
    let auth_cookie_path = if let Some(bytes) = source_cookies {
        let path = upload_dir.join(format!("music-sample-seed-{now}-cookies.txt"));
        if let Err(e) = fs::write(&path, &bytes) {
            return json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to persist source_cookies: {e}"),
            );
        }
        Some(path)
    } else {
        None
    };
    let source_auth = MusicSourceAuth {
        mode: source_auth_mode,
        auth_header_name: source_auth_header_name,
        auth_header: source_auth_header,
        cookie_header: source_cookie_header,
        cookies_file: auth_cookie_path,
    };
    if source_url.is_some()
        && source_auth.mode == MusicSourceAuthMode::Authenticated
        && !source_auth.has_credentials()
    {
        return json_error(
            StatusCode::BAD_REQUEST,
            "authenticated source_url requires an Authorization header, Cookie header, or source_cookies file".to_string(),
        );
    }
    let source_kind = if let Some(sample_bytes) = sample_bytes {
        if sample_bytes.is_empty() {
            if let Some(path) = &source_auth.cookies_file {
                let _ = fs::remove_file(path);
            }
            return json_error(
                StatusCode::BAD_REQUEST,
                "sample upload is empty".to_string(),
            );
        }
        if let Err(e) = fs::write(&upload_path, &sample_bytes) {
            if let Some(path) = &source_auth.cookies_file {
                let _ = fs::remove_file(path);
            }
            return json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to persist upload: {e}"),
            );
        }
        "upload".to_string()
    } else if let Some(url) = &source_url {
        match download_music_source_url(url, &upload_path, &source_auth).await {
            Ok(kind) => kind,
            Err(e) => {
                if let Some(path) = &source_auth.cookies_file {
                    let _ = fs::remove_file(path);
                }
                let _ = fs::remove_file(&upload_path);
                return json_error(StatusCode::BAD_REQUEST, e);
            }
        }
    } else {
        if let Some(path) = &source_auth.cookies_file {
            let _ = fs::remove_file(path);
        }
        return json_error(
            StatusCode::BAD_REQUEST,
            "provide multipart field `sample` or `source_url`".to_string(),
        );
    };
    if let Some(path) = &source_auth.cookies_file {
        let _ = fs::remove_file(path);
    }

    let render_dir = state.out_dir.join("music-production").join("sample-seed");
    if let Err(e) = fs::create_dir_all(&render_dir) {
        let _ = fs::remove_file(&upload_path);
        return json_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to create music output dir: {e}"),
        );
    }
    let wav_path = render_dir.join(format!("sample-seed-{now}.wav"));
    let manifest_path = render_dir.join(format!("sample-seed-{now}.json"));
    let wav_url = out_url(&state, &wav_path).unwrap_or_else(|| "out/".to_string());
    let manifest_url = out_url(&state, &manifest_path).unwrap_or_else(|| "out/".to_string());
    let prompt_for_render = prompt.trim().to_string();
    let source_auth_summary = source_auth.summary_json();
    let source_url_for_manifest = redacted_source_url(source_url.as_ref());

    let _guard = state.sim_lock.lock().await;
    let render_result: Result<Value, String> = tokio::task::spawn_blocking(move || {
        let result = (|| {
            let sample = derive_music_sample_seed_from_mp4(&upload_path)
                .map_err(|e| format!("failed to derive music-sample-seed: {e}"))?;
            let prompt_influence = if prompt_for_render.is_empty() {
                None
            } else {
                analyze_music_sample_prompt(&prompt_for_render)
            };
            let spec = song_spec_from_music_sample_seed_with_prompt(
                &sample,
                title,
                duration_seconds,
                if prompt_for_render.is_empty() {
                    None
                } else {
                    Some(prompt_for_render.as_str())
                },
            );
            let render = generate_microtonal_song(spec);
            render
                .audio
                .write_wav16(&wav_path)
                .map_err(|e| format!("failed to write wav: {e}"))?;
            let response = json!({
                "ok": true,
                "wav_url": wav_url,
                "manifest_url": manifest_url,
                "wav_path": wav_path.display().to_string(),
                "sample": {
                    "source_kind": source_kind,
                    "source_url": source_url_for_manifest,
                    "source_auth": source_auth_summary,
                    "source_duration_seconds": sample.duration_seconds,
                    "seed": sample.seed,
                    "byte_entropy": sample.byte_entropy,
                    "suggested_genre": sample.suggested_genre.as_str(),
                    "suggested_bpm": sample.suggested_bpm,
                    "descriptors": sample.descriptors,
                    "source_audio_copied": false
                },
                "prompt": prompt_influence.map(|influence| json!({
                    "chars": influence.prompt_chars,
                    "hash": influence.prompt_hash,
                    "genre": influence.genre.map(|genre| genre.as_str()),
                    "bpm_delta": influence.bpm_delta,
                    "key_bias_delta": influence.key_bias_delta,
                    "meter_bias": influence.meter_bias.map(|(n, d)| format!("{n}/{d}")),
                    "tags": influence.feature_tags
                })),
                "summary": music_summary_json(&render.summary)
            });
            fs::write(
                &manifest_path,
                serde_json::to_string_pretty(&response)
                    .map_err(|e| format!("failed to serialize manifest: {e}"))?,
            )
            .map_err(|e| format!("failed to write manifest: {e}"))?;
            Ok(response)
        })();
        let _ = fs::remove_file(&upload_path);
        result
    })
    .await
    .unwrap_or_else(|e| Err(format!("music render task failed: {e}")));

    match render_result {
        Ok(value) => Json(value).into_response(),
        Err(error) => json_error(StatusCode::BAD_REQUEST, error),
    }
}

fn music_summary_json(summary: &ArrangementSummary) -> Value {
    json!({
        "title": &summary.title,
        "genre": summary.genre.as_str(),
        "duration_seconds": summary.duration_seconds,
        "bpm": summary.bpm,
        "scale": &summary.scale_name,
        "key_changes": summary.key_changes.len(),
        "time_signature_changes": summary.time_signature_changes.len(),
        "pauses": summary.pauses.len(),
        "drum_patterns": summary.drum_variation.pattern_names.len(),
        "drum_fills": summary.drum_variation.fills,
        "drum_micro_variations": summary.drum_variation.micro_variations,
        "drum_variation_ratio": summary.drum_variation.variation_ratio(),
        "percussion_gain": summary.drum_variation.percussion_gain,
        "instruments": &summary.instruments,
        "parts": summary.parts.iter().map(|part| json!({
            "name": &part.name,
            "role": part.role.as_str(),
            "instrument": &part.instrument,
            "events": part.events
        })).collect::<Vec<_>>(),
        "rendered_events": summary.rendered_events,
        "peak": summary.peak,
        "rms": summary.rms,
        "spectral_centroid_hz": summary.spectral_centroid_hz
    })
}

fn out_url(state: &AppState, path: &StdPath) -> Option<String> {
    path.strip_prefix(state.out_dir.as_path())
        .ok()
        .map(|rel| format!("out/{}", rel.to_string_lossy().replace('\\', "/")))
}

fn json_error(status: StatusCode, error: impl Into<String>) -> Response {
    (status, Json(json!({ "ok": false, "error": error.into() }))).into_response()
}

async fn download_music_source_url(
    raw: &str,
    path: &StdPath,
    auth: &MusicSourceAuth,
) -> Result<String, String> {
    let url = validate_public_music_url(raw)?;
    validate_public_music_url_dns(&url).await?;
    if prefers_ytdlp(&url) {
        match download_with_ytdlp(url.as_str().to_string(), path.to_path_buf(), auth).await {
            Ok(kind) => return Ok(format!("{kind}; access={}", auth.effective_mode().as_str())),
            Err(ytdlp_error) => match download_direct_media(&url, path, auth).await {
                Ok(kind) => return Ok(format!("{kind}; yt-dlp fallback reason: {ytdlp_error}")),
                Err(direct_error) => {
                    return Err(format!(
                            "could not download public media link. yt-dlp: {ytdlp_error}; direct HTTP: {direct_error}"
                        ));
                }
            },
        }
    }

    match download_direct_media(&url, path, auth).await {
        Ok(kind) => Ok(kind),
        Err(direct_error) => match download_with_ytdlp(
            url.as_str().to_string(),
            path.to_path_buf(),
            auth,
        )
        .await
        {
            Ok(kind) => Ok(format!(
                "{kind}; access={}; direct HTTP fallback reason: {direct_error}",
                auth.effective_mode().as_str()
            )),
            Err(ytdlp_error) => Err(format!(
                "could not download public media link. direct HTTP: {direct_error}; yt-dlp: {ytdlp_error}"
            )),
        },
    }
}

fn validate_public_music_url(raw: &str) -> Result<reqwest::Url, String> {
    let url = reqwest::Url::parse(raw.trim()).map_err(|e| format!("invalid source_url: {e}"))?;
    validate_public_music_url_parts(&url)?;
    Ok(url)
}

async fn validate_public_music_url_dns(url: &reqwest::Url) -> Result<(), String> {
    let host = url
        .host_str()
        .ok_or_else(|| "source_url must include a public host".to_string())?;
    if host.parse::<IpAddr>().is_ok() {
        return Ok(());
    }
    let port = url
        .port_or_known_default()
        .ok_or_else(|| "source_url must use a URL scheme with a known port".to_string())?;
    let addrs = tokio::net::lookup_host((host, port)).await.map_err(|e| {
        format!(
            "source_url host `{}` could not be resolved: {e}",
            truncate_for_error(host, 120)
        )
    })?;
    validate_music_resolved_addrs(host, addrs.map(|addr| addr.ip()))
}

fn validate_music_resolved_addrs<I>(host: &str, addrs: I) -> Result<(), String>
where
    I: IntoIterator<Item = IpAddr>,
{
    let mut saw_addr = false;
    for ip in addrs {
        saw_addr = true;
        if is_blocked_music_ip(ip) {
            return Err(format!(
                "source_url host `{}` resolves to localhost/private network",
                truncate_for_error(host, 120)
            ));
        }
    }
    if !saw_addr {
        return Err(format!(
            "source_url host `{}` resolved to no addresses",
            truncate_for_error(host, 120)
        ));
    }
    Ok(())
}

fn validate_public_music_url_parts(url: &reqwest::Url) -> Result<(), String> {
    match url.scheme() {
        "http" | "https" => {}
        _ => return Err("source_url must use http or https".to_string()),
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(
            "source_url must not embed credentials; use the dedicated auth fields".to_string(),
        );
    }
    let host = url
        .host_str()
        .ok_or_else(|| "source_url must include a public host".to_string())?
        .to_ascii_lowercase();
    if is_blocked_music_host(&host) {
        return Err(
            "source_url must point to a public resource, not localhost/private network".to_string(),
        );
    }
    Ok(())
}

fn is_blocked_music_host(host: &str) -> bool {
    let normalized = host.trim_matches(['[', ']']);
    if normalized == "localhost"
        || normalized.ends_with(".localhost")
        || normalized.ends_with(".local")
        || normalized.ends_with(".internal")
        || normalized == "metadata.google.internal"
    {
        return true;
    }
    normalized
        .parse::<IpAddr>()
        .map(is_blocked_music_ip)
        .unwrap_or(false)
}

fn is_blocked_music_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(addr) => {
            let octets = addr.octets();
            addr.is_loopback()
                || addr.is_private()
                || addr.is_link_local()
                || addr.is_broadcast()
                || addr.is_unspecified()
                || addr.is_multicast()
                || (octets[0] == 100 && (64..=127).contains(&octets[1]))
        }
        IpAddr::V6(addr) => {
            let segments = addr.segments();
            addr.is_loopback()
                || addr.is_unspecified()
                || addr.is_multicast()
                || (segments[0] & 0xfe00) == 0xfc00
                || (segments[0] & 0xffc0) == 0xfe80
        }
    }
}

fn music_redirect_policy(
    source_url: &reqwest::Url,
    authenticated: bool,
) -> reqwest::redirect::Policy {
    let source_host = source_url.host_str().map(|host| host.to_ascii_lowercase());
    reqwest::redirect::Policy::custom(move |attempt| {
        if attempt.previous().len() >= 8 {
            return attempt.error("too many redirects");
        }
        if validate_public_music_url_parts(attempt.url()).is_err() {
            return attempt.error("redirect target is not a public http/https URL");
        }
        if authenticated {
            let next_host = attempt
                .url()
                .host_str()
                .map(|host| host.to_ascii_lowercase());
            if next_host != source_host {
                return attempt
                    .error("authenticated source_url redirects must stay on the original host");
            }
        }
        attempt.follow()
    })
}

fn prefers_ytdlp(url: &reqwest::Url) -> bool {
    let host = url.host_str().unwrap_or("").to_ascii_lowercase();
    let social_host = [
        "youtube.com",
        "youtu.be",
        "facebook.com",
        "fb.watch",
        "instagram.com",
        "x.com",
        "twitter.com",
        "tiktok.com",
        "soundcloud.com",
        "vimeo.com",
    ]
    .iter()
    .any(|domain| host == *domain || host.ends_with(&format!(".{domain}")));
    social_host || !looks_like_direct_media_url(url)
}

fn looks_like_direct_media_url(url: &reqwest::Url) -> bool {
    let path = url.path().to_ascii_lowercase();
    [
        ".mp4", ".m4v", ".mov", ".webm", ".mkv", ".mp3", ".m4a", ".wav", ".aac", ".ogg",
    ]
    .iter()
    .any(|suffix| path.ends_with(suffix))
}

async fn download_direct_media(
    url: &reqwest::Url,
    path: &StdPath,
    auth: &MusicSourceAuth,
) -> Result<String, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(MUSIC_DOWNLOAD_TIMEOUT_SECS))
        .user_agent("dd-des-rs-music-sample-seed/0.1")
        .redirect(music_redirect_policy(
            url,
            auth.effective_mode() == MusicSourceAuthMode::Authenticated,
        ))
        .build()
        .map_err(|e| format!("failed to build HTTP client: {e}"))?;
    let mut request = client.get(url.clone());
    if let Some(value) = &auth.auth_header {
        let header_value =
            HeaderValue::from_str(value).map_err(|e| format!("invalid source_auth_header: {e}"))?;
        let header_name = auth
            .auth_header_name
            .clone()
            .unwrap_or(header::AUTHORIZATION);
        request = request.header(header_name, header_value);
    }
    if let Some(value) = &auth.cookie_header {
        let header_value = HeaderValue::from_str(value)
            .map_err(|e| format!("invalid source_cookie_header: {e}"))?;
        request = request.header(header::COOKIE, header_value);
    }
    let response = request
        .send()
        .await
        .map_err(|e| format!("GET failed: {}", e.without_url()))?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("GET returned HTTP {status}"));
    }
    if let Some(len) = response.content_length() {
        if len > MAX_MUSIC_UPLOAD_BYTES as u64 {
            return Err(format!(
                "resource is too large ({len} bytes; max {MAX_MUSIC_UPLOAD_BYTES})"
            ));
        }
    }
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !looks_like_direct_media_url(url)
        && !content_type.starts_with("video/")
        && !content_type.starts_with("audio/")
        && !content_type.contains("octet-stream")
    {
        return Err(format!(
            "direct HTTP resource is not advertised as audio/video (content-type {content_type:?})"
        ));
    }
    let mut file = tokio::fs::File::create(path)
        .await
        .map_err(|e| format!("failed to create downloaded media file: {e}"))?;
    let mut response = response;
    let mut downloaded = 0usize;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| format!("failed to read body: {}", e.without_url()))?
    {
        downloaded = downloaded
            .checked_add(chunk.len())
            .ok_or_else(|| "downloaded media size overflowed".to_string())?;
        if downloaded > MAX_MUSIC_UPLOAD_BYTES {
            let _ = tokio::fs::remove_file(path).await;
            return Err(format!(
                "resource is too large ({downloaded} bytes; max {MAX_MUSIC_UPLOAD_BYTES})"
            ));
        }
        file.write_all(&chunk)
            .await
            .map_err(|e| format!("failed to write downloaded media: {e}"))?;
    }
    file.flush()
        .await
        .map_err(|e| format!("failed to flush downloaded media: {e}"))?;
    Ok(format!(
        "direct-http; access={}",
        auth.effective_mode().as_str()
    ))
}

async fn download_with_ytdlp(
    url: String,
    path: PathBuf,
    auth: &MusicSourceAuth,
) -> Result<String, String> {
    let cookies_file = auth.cookies_file.clone();
    let auth_header_name = auth.auth_header_name.clone();
    let auth_header = auth.auth_header.clone();
    tokio::task::spawn_blocking(move || {
        run_ytdlp_download(
            &url,
            &path,
            cookies_file.as_deref(),
            auth_header_name.as_ref(),
            auth_header.as_deref(),
        )
    })
    .await
    .unwrap_or_else(|e| Err(format!("yt-dlp task failed: {e}")))
}

fn run_ytdlp_download(
    url: &str,
    path: &StdPath,
    cookies_file: Option<&StdPath>,
    auth_header_name: Option<&HeaderName>,
    auth_header: Option<&str>,
) -> Result<String, String> {
    let mut attempts = Vec::new();
    if let Ok(bin) = env::var("DES_YTDLP_BIN") {
        if !bin.trim().is_empty() {
            attempts.push(YtDlpCommand::Binary(bin));
        }
    }
    attempts.push(YtDlpCommand::Binary("yt-dlp".to_string()));
    attempts.push(YtDlpCommand::Binary("youtube-dl".to_string()));
    attempts.push(YtDlpCommand::PythonModule);

    let mut args = vec![
        "--no-playlist".to_string(),
        "--force-overwrites".to_string(),
        "--max-filesize".to_string(),
        format!("{}m", (MAX_MUSIC_UPLOAD_BYTES / (1024 * 1024)).max(1)),
        "--merge-output-format".to_string(),
        "mp4".to_string(),
        "--remux-video".to_string(),
        "mp4".to_string(),
        "-f".to_string(),
        "b[ext=mp4]/bv*[ext=mp4]+ba[ext=m4a]/best".to_string(),
        "-o".to_string(),
        path.display().to_string(),
    ];
    if let Some(cookies_file) = cookies_file {
        args.extend(["--cookies".to_string(), cookies_file.display().to_string()]);
    }
    if let Some(value) = auth_header {
        let name = auth_header_name
            .map(|name| name.as_str())
            .unwrap_or(header::AUTHORIZATION.as_str());
        args.extend(["--add-header".to_string(), format!("{name}: {value}")]);
    }
    args.push(url.to_string());

    let mut errors = Vec::new();
    let redacted_url = redacted_source_url_value(url);
    for attempt in attempts {
        match run_ytdlp_attempt(&attempt, &args) {
            Ok(()) => {
                if path.exists() {
                    return Ok(match attempt {
                        YtDlpCommand::Binary(name) => format!("yt-dlp:{name}"),
                        YtDlpCommand::PythonModule => "yt-dlp:python3 -m yt_dlp".to_string(),
                    });
                }
                errors.push(format!(
                    "{} exited successfully but did not create {}",
                    attempt.label(),
                    path.display()
                ));
            }
            Err(e) => errors.push(format!(
                "{}: {}",
                attempt.label(),
                sanitize_url_in_error(&e, url, &redacted_url)
            )),
        }
    }
    Err(errors.join("; "))
}

enum YtDlpCommand {
    Binary(String),
    PythonModule,
}

impl YtDlpCommand {
    fn label(&self) -> String {
        match self {
            YtDlpCommand::Binary(name) => name.clone(),
            YtDlpCommand::PythonModule => "python3 -m yt_dlp".to_string(),
        }
    }
}

fn run_ytdlp_attempt(command: &YtDlpCommand, args: &[String]) -> Result<(), String> {
    let mut cmd = match command {
        YtDlpCommand::Binary(name) => Command::new(name),
        YtDlpCommand::PythonModule => {
            let mut cmd = Command::new("python3");
            cmd.arg("-m").arg("yt_dlp");
            cmd
        }
    };
    let mut child = cmd
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to start: {e}"))?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => {
                let output = child
                    .wait_with_output()
                    .map_err(|e| format!("failed to collect output: {e}"))?;
                if output.status.success() {
                    return Ok(());
                }
                let stderr = String::from_utf8_lossy(&output.stderr);
                let stdout = String::from_utf8_lossy(&output.stdout);
                return Err(format!(
                    "exit {}: {}{}",
                    output.status,
                    truncate_for_error(stderr.trim(), 700),
                    if stdout.trim().is_empty() {
                        "".to_string()
                    } else {
                        format!("; stdout: {}", truncate_for_error(stdout.trim(), 300))
                    }
                ));
            }
            Ok(None) => {
                if started.elapsed() > Duration::from_secs(MUSIC_DOWNLOAD_TIMEOUT_SECS) {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!("timed out after {MUSIC_DOWNLOAD_TIMEOUT_SECS}s"));
                }
                thread::sleep(Duration::from_millis(250));
            }
            Err(e) => return Err(format!("wait failed: {e}")),
        }
    }
}

fn truncate_for_error(value: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for (i, ch) in value.chars().enumerate() {
        if i >= max_chars {
            out.push_str("...");
            break;
        }
        out.push(ch);
    }
    out
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
            "soccerVideogame": "GET /out/soccer-sim.html  (2D 11v11 soccer videogame / learning sim artifact)",
            "soccerVideogameTrace": "GET /out/soccer-sim.json  (full soccer match trace: config, summary, frames, events)",
            "soccerVideogameFrames": "GET /out/soccer-sim.frames.jsonl  (header + frame/event/summary records)",
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
/// mdp, pomdp, soccer-planner): each is an iterative solver fed a JSONL
/// command stream.
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

async fn run_streaming_model(state: AppState, name: String, body: String) -> Response {
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
async fn streaming_run(
    State(state): State<AppState>,
    Path(name): Path<String>,
    body: String,
) -> Response {
    run_streaming_model(state, name, body).await
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

/// `GET /soccer/planner` — interactive 11-a-side rotation planner UI.
async fn soccer_planner_page(State(state): State<AppState>) -> Html<String> {
    Html(state.soccer_planner_html.to_string())
}

/// `POST /soccer/planner/solve` — re-solve with roster/constraints from the UI.
async fn soccer_planner_solve(
    State(state): State<AppState>,
    request: Result<Json<PlannerRequest>, JsonRejection>,
) -> Response {
    let Json(mut req) = match request {
        Ok(req) => req,
        Err(err) => {
            return json_error(
                StatusCode::BAD_REQUEST,
                format!("invalid soccer planner request JSON: {err}"),
            );
        }
    };
    let requested_solver_time_limit_ms = req.solver_time_limit_ms;
    let solver_time_was_capped = requested_solver_time_limit_ms.is_finite()
        && requested_solver_time_limit_ms > SOCCER_PLANNER_HTTP_SOLVE_BUDGET_MS;
    if solver_time_was_capped {
        req.solver_time_limit_ms = SOCCER_PLANNER_HTTP_SOLVE_BUDGET_MS;
    }

    let _guard = state.sim_lock.lock().await;
    let result =
        tokio::task::spawn_blocking(move || catch_unwind(AssertUnwindSafe(|| solve_planner(&req))))
            .await;
    let mut resp = match result {
        Ok(Ok(r)) => r,
        Ok(Err(panic_payload)) => {
            let error = panic_payload
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| panic_payload.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_else(|| "soccer planner solve panicked".to_string());
            return json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("soccer planner solve panicked: {error}"),
            );
        }
        Err(e) => {
            return json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("soccer planner solve task failed: {e}"),
            );
        }
    };
    if solver_time_was_capped {
        resp.solver_notes.push(format!(
            "Server capped solverTimeLimitMs from {:.0}ms to {:.0}ms so the HTTP endpoint returns JSON before the gateway timeout.",
            requested_solver_time_limit_ms, SOCCER_PLANNER_HTTP_SOLVE_BUDGET_MS
        ));
    }
    let status = if resp.ok {
        StatusCode::OK
    } else {
        StatusCode::UNPROCESSABLE_ENTITY
    };
    (status, Json(planner_response_to_json(&resp))).into_response()
}

/// `POST /soccer/planner/stream` — planner-specific alias for the generic
/// `streaming/soccer-planner` JSONL endpoint.
async fn soccer_planner_stream(State(state): State<AppState>, body: String) -> Response {
    run_streaming_model(state, "soccer-planner".to_string(), body).await
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
            SOCCER_SIM_TRACE_JSON.to_string(),
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

fn serve_output_file(base: &StdPath, rel_path: &str, path: &StdPath) -> Response {
    match std::fs::read(path) {
        Ok(bytes) => (
            [
                ("content-type", content_type(path)),
                ("x-content-type-options", "nosniff"),
                ("cache-control", "public, max-age=30"),
            ],
            if content_type(path).starts_with("text/html") {
                match String::from_utf8(bytes) {
                    Ok(html) => {
                        inject_before_body(html, &output_toolbar_html(base, rel_path)).into_bytes()
                    }
                    Err(err) => err.into_bytes(),
                }
            } else {
                bytes
            },
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

async fn delivery_planner_redirect() -> Response {
    Redirect::temporary("out/delivery-planner.html").into_response()
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

const SOCCER_SIM_TRACE_JSON: &str = "soccer-sim.json";
const SOCCER_SIM_FRAMES_JSONL: &str = "soccer-sim.frames.jsonl";

fn soccer_trace_jsonl(trace: &SimulationTrace) -> String {
    let mut lines = Vec::with_capacity(trace.frames.len() + trace.events.len() + 2);
    lines.push(
        serde_json::to_string(&json!({
            "kind": "soccer-sim-header",
            "schema": "dd.des.soccer.sim.trace.v1",
            "config": &trace.config,
            "frameCount": trace.frames.len(),
            "eventCount": trace.events.len()
        }))
        .unwrap_or_else(|_| "{}".to_string()),
    );
    for (index, frame) in trace.frames.iter().enumerate() {
        lines.push(
            serde_json::to_string(&json!({
                "kind": "soccer-sim-frame",
                "index": index,
                "frame": frame
            }))
            .unwrap_or_else(|_| "{}".to_string()),
        );
    }
    for (index, event) in trace.events.iter().enumerate() {
        lines.push(
            serde_json::to_string(&json!({
                "kind": "soccer-sim-event",
                "index": index,
                "event": event
            }))
            .unwrap_or_else(|_| "{}".to_string()),
        );
    }
    lines.push(
        serde_json::to_string(&json!({
            "kind": "soccer-sim-summary",
            "summary": &trace.summary
        }))
        .unwrap_or_else(|_| "{}".to_string()),
    );
    let mut jsonl = lines.join("\n");
    jsonl.push('\n');
    jsonl
}

async fn ensure_soccer_trace_artifacts(state: &AppState) -> Result<(), String> {
    let trace_path = state.out_dir.join(SOCCER_SIM_TRACE_JSON);
    let frames_path = state.out_dir.join(SOCCER_SIM_FRAMES_JSONL);
    if trace_path.is_file() && frames_path.is_file() {
        return Ok(());
    }

    let _guard = state.sim_lock.lock().await;
    if trace_path.is_file() && frames_path.is_file() {
        return Ok(());
    }

    std::fs::create_dir_all(state.out_dir.as_path())
        .map_err(|e| format!("create output dir: {e}"))?;
    let trace = run_default_simulation();
    let json = serde_json::to_string_pretty(&trace).map_err(|e| format!("encode trace: {e}"))?;
    std::fs::write(&trace_path, json).map_err(|e| format!("write soccer trace json: {e}"))?;
    std::fs::write(&frames_path, soccer_trace_jsonl(&trace))
        .map_err(|e| format!("write soccer frames jsonl: {e}"))?;
    Ok(())
}

async fn out_file(State(state): State<AppState>, Path(rel_path): Path<String>) -> Response {
    if matches!(
        rel_path.as_str(),
        SOCCER_SIM_TRACE_JSON | SOCCER_SIM_FRAMES_JSONL
    ) {
        if let Err(err) = ensure_soccer_trace_artifacts(&state).await {
            eprintln!("[dd-des-rs] soccer trace render failed: {err}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "soccer trace render failed",
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
                return serve_output_file(base, &rel, &index);
            }
        }
        return (StatusCode::NOT_FOUND, "not found").into_response();
    }

    if !target.is_file() {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    }

    let rel = path_to_output_rel(base, &target);
    serve_output_file(base, &rel, &target)
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
            "List JSONL streaming-solver contracts (lp, milp/mip/ip, mdp, pomdp, soccer-planner).",
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
            "/out/soccer-sim.json",
            "Rendered soccer game trace JSON with config, summary, frames, and events.",
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
    let elevator_fel_html: Arc<str> = Arc::from(render_elevator_html(&run_fel_elevator(
        &ElevatorConfig::default(),
    )));
    let elevator_mdp_html: Arc<str> = Arc::from(render_model_player("mdp", &elevator_mdp_spec()));
    let elevator_pomdp_html: Arc<str> =
        Arc::from(render_model_player("pomdp", &elevator_pomdp_spec()));
    let soccer_planner_html: Arc<str> = Arc::from(planner_page_html());

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
        soccer_planner_html,
    };

    // Populate `out/` in the background so /healthz comes up immediately while
    // the startup catalogue renders.
    let startup = env_value_or_empty("DES_STARTUP_SIMS", DEFAULT_STARTUP_SIMS);
    if !startup.is_empty() {
        let startup_state = state.clone();
        tokio::spawn(async move {
            for needle in startup
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
            {
                if let Err(SimMatchError::TooMany { count, .. }) = checked_sim_names(&needle, false)
                {
                    eprintln!(
                        "[dd-des-rs] startup `{needle}` skipped: filter matches {count} sim(s); use narrower DES_STARTUP_SIMS entries"
                    );
                    continue;
                }
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
        .route(
            "/models/:kind/run",
            get(model_run_example).post(model_run_post),
        )
        .route("/streaming", get(list_streaming))
        .route("/streaming/:name", post(streaming_run))
        .route("/elevator-fel", get(elevator_fel))
        .route("/elevator-mdp", get(elevator_mdp))
        .route("/elevator-pomdp", get(elevator_pomdp))
        .route("/soccer/planner", get(soccer_planner_page))
        .route("/soccer/planner/solve", post(soccer_planner_solve))
        .route("/soccer/planner/stream", post(soccer_planner_stream))
        .route("/music", get(music_production_page))
        .route(
            "/music/sample-seed",
            post(music_sample_seed_render).layer(DefaultBodyLimit::max(MAX_MUSIC_UPLOAD_BYTES)),
        )
        .route("/delivery-planner.html", get(delivery_planner_redirect))
        .route("/deliver-planner.html", get(delivery_planner_redirect))
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
            assert!(
                kinds.contains(&expected.to_string()),
                "missing kind {expected}"
            );
        }
        assert!(streaming_model_names().contains(&"lp"));
        assert!(streaming_model_names().contains(&"mdp"));
        assert!(streaming_model_names().contains(&"soccer-planner"));
        assert!(
            streaming_contracts().len() >= 5,
            "expected lp/milp/mdp/pomdp/soccer-planner streaming contracts"
        );
    }

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
        assert!(paths.contains(&"/out/soccer-sim.html"));
        assert!(paths.contains(&"/out/soccer-sim.json"));
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
    fn music_source_url_validation_blocks_private_and_secret_bearing_urls() {
        for raw in [
            "ftp://example.com/seed.mp4",
            "http://localhost/seed.mp4",
            "http://127.1.2.3/seed.mp4",
            "http://10.1.2.3/seed.mp4",
            "http://172.20.1.2/seed.mp4",
            "http://192.168.0.2/seed.mp4",
            "http://169.254.169.254/latest/meta-data",
            "http://100.64.0.1/seed.mp4",
            "http://[::1]/seed.mp4",
            "https://user:pass@example.com/seed.mp4",
            "https://example.local/seed.mp4",
            "https://metadata.google.internal/computeMetadata/v1/",
        ] {
            assert!(
                validate_public_music_url(raw).is_err(),
                "expected {raw} to be rejected"
            );
        }

        assert!(validate_public_music_url("https://example.com/path/seed.mp4").is_ok());
    }

    #[test]
    fn music_source_dns_validation_blocks_private_resolutions() {
        assert!(validate_music_resolved_addrs(
            "media.example",
            ["93.184.216.34".parse::<IpAddr>().unwrap()]
        )
        .is_ok());
        assert!(validate_music_resolved_addrs(
            "media.example",
            [
                "93.184.216.34".parse::<IpAddr>().unwrap(),
                "10.1.2.3".parse::<IpAddr>().unwrap()
            ]
        )
        .is_err());
        assert!(
            validate_music_resolved_addrs("media.example", std::iter::empty::<IpAddr>()).is_err()
        );
    }

    #[test]
    fn music_source_url_redaction_removes_credentials_and_query() {
        assert_eq!(
            redacted_source_url_value("https://user:pass@example.com/watch?v=secret"),
            "https://example.com/watch?redacted=1"
        );
        assert_eq!(
            sanitize_url_in_error(
                "failed for https://example.com/watch?v=secret",
                "https://example.com/watch?v=secret",
                "https://example.com/watch?redacted=1"
            ),
            "failed for https://example.com/watch?redacted=1"
        );
    }

    #[test]
    fn music_auth_header_name_validation_accepts_auth_and_rejects_bad_names() {
        let header = clean_music_auth_header_name_field(" Auth ".to_string())
            .unwrap()
            .unwrap();
        assert_eq!(header.as_str(), "auth");
        assert!(clean_music_auth_header_name_field("Bad Header".to_string()).is_err());
        assert!(clean_music_auth_header_name_field("Auth:\nsecret".to_string()).is_err());
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
            content_type(StdPath::new("a.frames.jsonl")),
            "application/x-ndjson; charset=utf-8"
        );
        assert_eq!(
            content_type(StdPath::new("a.bin")),
            "application/octet-stream"
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
                SOCCER_SIM_TRACE_JSON.to_string(),
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
