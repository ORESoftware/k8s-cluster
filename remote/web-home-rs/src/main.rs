use std::{env, net::SocketAddr, time::Instant};

use axum::{
    extract::State,
    http::StatusCode,
    http::{header, HeaderValue},
    response::{Html, IntoResponse, Response},
    routing::get,
    Json, Router,
};
use maud::{html, Markup, DOCTYPE};
use once_cell::sync::Lazy;
use prometheus::{Encoder, IntCounterVec, IntGauge, Opts, TextEncoder};
use serde::Serialize;

static STARTED_AT: Lazy<Instant> = Lazy::new(Instant::now);
static HTTP_REQUESTS: Lazy<IntCounterVec> = Lazy::new(|| {
    let counter = IntCounterVec::new(
        Opts::new(
            "dd_runtime_http_requests_total",
            "HTTP requests observed by the dd remote runtime.",
        ),
        &["service", "method", "path", "status"],
    )
    .expect("failed to create dd_runtime_http_requests_total");
    prometheus::default_registry()
        .register(Box::new(counter.clone()))
        .expect("failed to register dd_runtime_http_requests_total");
    counter
});
static UPTIME_SECONDS: Lazy<IntGauge> = Lazy::new(|| {
    let gauge = IntGauge::new(
        "dd_runtime_uptime_seconds",
        "Worker process uptime in seconds.",
    )
    .expect("failed to create dd_runtime_uptime_seconds");
    prometheus::default_registry()
        .register(Box::new(gauge.clone()))
        .expect("failed to register dd_runtime_uptime_seconds");
    gauge
});

#[derive(Clone)]
struct AppState {
    server_label: String,
    control_plane_label: String,
    workers_label: String,
    queue_consumer_label: String,
}

#[derive(Serialize)]
struct HealthResponse {
    ok: bool,
    service: String,
    mode: String,
}

fn redirect_home() -> Response {
    let mut response = Response::new(axum::body::Body::empty());
    *response.status_mut() = StatusCode::FOUND;
    response
        .headers_mut()
        .insert(header::LOCATION, HeaderValue::from_static("/home"));
    response
}

fn record_request(method: &str, path: &str, status: StatusCode) {
    HTTP_REQUESTS
        .with_label_values(&["dd-remote-web-home", method, path, status.as_str()])
        .inc();
}

async fn root() -> impl IntoResponse {
    record_request("GET", "/", StatusCode::FOUND);
    redirect_home()
}

async fn home(State(state): State<AppState>) -> impl IntoResponse {
    record_request("GET", "/home", StatusCode::OK);
    let body = format!(
        r#"<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>dd-remote-web</title>
    <style>
      :root {{
        color-scheme: dark;
        --bg: #0b1117;
        --panel: #111923;
        --panel-2: #0f1720;
        --line: rgba(148, 163, 184, 0.24);
        --text: #eef2f6;
        --muted: #a8b3c1;
        --accent: #5eead4;
        --warn: #fbbf24;
      }}
      * {{ box-sizing: border-box; }}
      body {{
        margin: 0;
        min-height: 100vh;
        background: var(--bg);
        color: var(--text);
        font-family: Inter, ui-sans-serif, system-ui, -apple-system, Segoe UI, sans-serif;
        padding: 24px;
      }}
      .shell {{ max-width: 1180px; margin: 0 auto; }}
      h1 {{ margin: 0 0 10px; font-size: 30px; }}
      h2 {{ margin: 0 0 12px; font-size: 17px; }}
      p {{ margin: 0 0 14px; color: var(--muted); line-height: 1.5; }}
      a {{ color: var(--accent); text-decoration: none; }}
      a:hover {{ text-decoration: underline; }}
      .grid {{
        display: grid;
        grid-template-columns: repeat(4, minmax(0, 1fr));
        gap: 14px;
        margin: 18px 0;
      }}
      .panel {{
        border: 1px solid var(--line);
        border-radius: 8px;
        background: var(--panel);
        padding: 14px;
      }}
      .label {{
        display: block;
        font-size: 11px;
        color: var(--muted);
        margin-bottom: 7px;
        text-transform: uppercase;
        letter-spacing: 0.08em;
      }}
      .value {{ font-size: 14px; line-height: 1.35; }}
      .band {{
        border: 1px solid var(--line);
        border-radius: 8px;
        background: var(--panel-2);
        padding: 16px;
        margin-top: 16px;
      }}
      table {{
        width: 100%;
        border-collapse: collapse;
        table-layout: fixed;
        font-size: 13px;
      }}
      th, td {{
        border-top: 1px solid var(--line);
        padding: 11px 10px;
        text-align: left;
        vertical-align: top;
        line-height: 1.4;
      }}
      th {{ color: var(--muted); font-weight: 600; }}
      code {{
        display: inline-block;
        max-width: 100%;
        overflow-wrap: anywhere;
        border: 1px solid rgba(148, 163, 184, 0.2);
        border-radius: 6px;
        padding: 2px 5px;
        background: #0a1017;
        color: #d7fbf4;
        font-size: 12px;
      }}
      .path-links {{
        display: flex;
        flex-wrap: wrap;
        gap: 6px;
      }}
      .path-links a {{
        text-decoration: none;
      }}
      .path-links a:hover code,
      .path-links a:focus-visible code {{
        border-color: rgba(94, 234, 212, 0.62);
        background: rgba(94, 234, 212, 0.1);
      }}
      .pill {{
        display: inline-flex;
        align-items: center;
        min-height: 24px;
        border-radius: 999px;
        border: 1px solid rgba(94, 234, 212, 0.35);
        padding: 2px 8px;
        color: var(--accent);
        background: rgba(94, 234, 212, 0.08);
        font-size: 12px;
      }}
      .pill.warn {{
        border-color: rgba(251, 191, 36, 0.35);
        color: var(--warn);
        background: rgba(251, 191, 36, 0.08);
      }}
      ol {{
        margin: 8px 0 0;
        padding-left: 22px;
        color: var(--muted);
        line-height: 1.55;
      }}
      @media (max-width: 880px) {{
        .grid {{ grid-template-columns: 1fr; }}
        table, thead, tbody, th, td, tr {{ display: block; }}
        th {{ display: none; }}
        td {{ border-top: 0; padding: 5px 0; }}
        tr {{ border-top: 1px solid var(--line); padding: 10px 0; }}
      }}
    </style>
  </head>
  <body>
    <main class="shell">
      <h1>dd remote service directory</h1>
      <p>Public entrypoint for the EC2 Kubernetes runtime. <code>/</code>, <code>/home</code>, <code>/agents/tasks</code>, <code>/agents/threads</code>, <code>/api/agents/tasks</code>, and <code>/webrtc/</code> are open. Authenticated entries include <code>/lambdas/functions</code>, <code>/lambdas/invoke/&lt;function-id&gt;</code>, and <code>/scrape</code>; ops paths stay behind internal gateway access.</p>
      <div class="grid">
        <section class="panel">
          <span class="label">Web Deployment</span>
          <div class="value">{}</div>
        </section>
        <section class="panel">
          <span class="label">K8s Routing</span>
          <div class="value">{}</div>
        </section>
        <section class="panel">
          <span class="label">Workers</span>
          <div class="value">{}</div>
        </section>
        <section class="panel">
          <span class="label">Queue Consumer</span>
          <div class="value">{}</div>
        </section>
      </div>
      <section class="band">
        <h2>Deployments</h2>
        <table>
          <thead>
            <tr>
              <th style="width: 25%">Deployment</th>
              <th style="width: 22%">Service</th>
              <th style="width: 16%">Access</th>
              <th>Notes</th>
            </tr>
          </thead>
          <tbody>
            <tr>
              <td><code>dd-web-scraper</code></td>
              <td><code>dd-web-scraper:8097</code></td>
              <td><span class="pill warn">server auth</span></td>
              <td>Long-running Fastify scraper deployment with <code>SCRAPER_PARSER_WORKERS=2</code>, browser strategies, DOM strategies, native fetch, Cheerio, and Browserless support.</td>
            </tr>
            <tr>
              <td><code>dd-gleam-lambda-runner</code></td>
              <td><code>dd-gleam-lambda-runner:8083</code></td>
              <td><span class="pill warn">server auth</span></td>
              <td>Gleam child-process runner deployment for <code>POST /lambdas/invoke/&lt;function-id&gt;</code>. It uses its own Argo CD app and <code>dd-gleam-lambda-runner-secrets</code>.</td>
            </tr>
          </tbody>
        </table>
      </section>
      <section class="band">
        <h2>Paths</h2>
        <table>
          <thead>
            <tr>
              <th style="width: 27%">Path</th>
              <th style="width: 25%">Target</th>
              <th style="width: 16%">Access</th>
              <th>Notes</th>
            </tr>
          </thead>
          <tbody>
            <tr>
              <td><span class="path-links"><a href="/"><code>/</code></a><a href="/home"><code>/home</code></a><a href="/agents/tasks"><code>/agents/tasks</code></a><a href="/agents/threads"><code>/agents/threads</code></a></span></td>
              <td>Rust web homepage deployment</td>
              <td><span class="pill">public</span></td>
              <td>Service directory plus cluster-served task/thread/PR UI. Browser UIs call JSON APIs for stored state while runtime invocation paths stay separate.</td>
            </tr>
              <tr>
                <td><span class="path-links"><a href="/tasks"><code>/tasks</code></a><a href="/status"><code>/status</code></a><a href="/stream/example-task-id"><code>/stream/&lt;uuid&gt;</code></a></span></td>
                <td>Node.js Coding Agent Task Manager</td>
                <td><span class="pill warn">server auth</span></td>
                <td>Runs inside the already-selected worker container. It executes prompts, tracks taskIds, streams events, and rejects requests for the wrong pinned thread.</td>
              </tr>
              <tr>
                <td><span class="path-links"><a href="/api/agents/tasks"><code>/api/agents/tasks</code></a><a href="/api/agents/threads/example-thread-id/context"><code>/api/agents/threads/&lt;uuid&gt;/context</code></a></span></td>
                <td>Rust REST API (JSON only)</td>
                <td><span class="pill">public</span></td>
                <td>JSON-only boundary for task snapshots and thread context. The browser UI lives at <code>/agents/tasks</code>; storage can move from RDS to in-cluster Postgres without changing the HTML server.</td>
              </tr>
              <tr>
                <td><span class="path-links"><a href="/lambdas/functions"><code>/lambdas/functions</code></a><a href="/api/lambdas/functions"><code>/api/lambdas/functions</code></a><a href="/lambdas/invoke/00000000-0000-0000-0000-000000000000"><code>POST /lambdas/invoke/&lt;function-id&gt;</code></a></span></td>
                <td>dd-gleam-lambda-runner deployment + Rust REST API</td>
                <td><span class="pill warn">server auth</span></td>
                <td>CRUD/read models stay in the REST API. Invocation traffic is routed directly by the gateway to the Gleam child-process runner.</td>
              </tr>
              <tr>
                <td><span class="path-links"><a href="/auth?return=/home"><code>/auth</code></a></span></td>
                <td>Rust PIN auth service</td>
                <td><span class="pill">public</span></td>
                <td>Sets the temporary <code>dd_auth</code> cookie after PIN auth. The gateway accepts that cookie instead of requiring browsers to send the legacy <code>Auth</code> header.</td>
              </tr>
              <tr>
                <td><span class="path-links"><code>dd.remote.thread.*.tasks</code><a href="/api/agents/threads/example-thread-id/prepare"><code>POST /api/agents/threads/&lt;uuid&gt;/prepare</code></a></span></td>
                <td>Rust NATS Queue Consumer</td>
                <td><span class="pill warn">internal access</span></td>
                <td>Shadow consumer deployment <code>dd-remote-queue-consumer</code> reads task messages, keeps thread affinity with queue group <code>dd-remote-thread-preparer</code>, and prepares the matching UUID-bound worker. It does not execute prompts.</td>
              </tr>
              <tr>
                <td><span class="path-links"><a href="/dd-thread/example"><code>/dd-thread/&lt;short&gt;</code></a><a href="/dd-thread/example/tasks"><code>/dd-thread/&lt;short&gt;/tasks</code></a><a href="/dd-thread/example/stream/example-task-id"><code>/dd-thread/&lt;short&gt;/stream/&lt;taskId&gt;</code></a></span></td>
                <td>Kubernetes per-thread Ingress</td>
                <td><span class="pill warn">server auth</span></td>
                <td>Target shape for chat dispatch: Ingress selects the UUID-bound worker Service; Node.js handles only the task inside that selected container.</td>
              </tr>
              <tr>
                <td><span class="path-links"><a href="/gleam/home"><code>/gleam/home</code></a><a href="/gleam/healthz"><code>/gleam/healthz</code></a><a href="/gleam/metrics"><code>/gleam/metrics</code></a><a href="/gleam/ws"><code>/gleam/ws</code></a></span></td>
                <td>Gleam WebSocket service</td>
                <td><span class="pill warn">internal access</span></td>
                <td>WebSocket endpoint: <code>wss://54.91.17.58/gleam/ws</code>.</td>
              </tr>
              <tr>
                <td><span class="path-links"><a href="/mcp"><code>/mcp</code></a><a href="/mcp/home"><code>/mcp/home</code></a><a href="/mcp/healthz"><code>/mcp/healthz</code></a><a href="/mcp/metrics"><code>/mcp/metrics</code></a></span></td>
                <td>Gleam MCP service</td>
                <td><span class="pill warn">internal access</span></td>
                <td>Dedicated MCP deployment with read-only runtime tools, Prometheus metrics, and Loki-collected stdout logs.</td>
              </tr>
              <tr>
                <td><span class="path-links"><a href="/webrtc/"><code>/webrtc/</code></a><a href="/webrtc/healthz"><code>/webrtc/healthz</code></a><a href="/webrtc/metrics"><code>/webrtc/metrics</code></a><code>wss://54.91.17.58/webrtc/signal</code></span></td>
                <td>Rust WebRTC signaling service</td>
                <td><span class="pill">public</span></td>
                <td>Room WebSocket signaling for browser/mobile peer handshakes. Media and data channels stay peer-to-peer; add TURN later for strict NATs.</td>
              </tr>
              <tr>
                <td><span class="path-links"><a href="/mdp/"><code>/mdp/</code></a><a href="/mdp/healthz"><code>/mdp/healthz</code></a><a href="/mdp/metrics"><code>/mdp/metrics</code></a><a href="/mdp/optimize"><code>POST /mdp/optimize</code></a><a href="/mdp/telemetry/learn"><code>POST /mdp/telemetry/learn</code></a><code>dd.remote.mdp.optimize</code><code>dd.remote.telemetry.mdp</code></span></td>
                <td>Rust MDP/POMDP optimizer</td>
                <td><span class="pill">public</span></td>
                <td>Async value-iteration, Q-value, policy, belief-state, and telemetry-risk optimizer. It subscribes to NATS optimization and telemetry jobs, then publishes results/events back to the runtime queue.</td>
              </tr>
              <tr>
                <td><span class="path-links"><a href="/scrape"><code>POST /scrape</code></a><a href="/scrape/strategies"><code>/scrape/strategies</code></a><a href="/scrape/healthz"><code>/scrape/healthz</code></a><a href="/scrape/metrics"><code>/scrape/metrics</code></a></span></td>
                <td>dd-web-scraper Fastify deployment</td>
                <td><span class="pill warn">server auth</span></td>
                <td>Long-running strategy router for native fetch, Cheerio, JSDOM, LinkeDOM, Playwright, Puppeteer, and Browserless scraping. Private cluster targets are blocked by default.</td>
              </tr>
              <tr>
                <td><span class="path-links"><a href="/telemetry/"><code>/telemetry/</code></a></span></td>
                <td>Grafana</td>
                <td><span class="pill warn">internal access</span></td>
                <td>Primary HTML dashboard for Prometheus metrics, Loki logs, Tempo traces, and NATS metrics.</td>
              </tr>
              <tr>
                <td><span class="path-links"><a href="/prometheus/"><code>/prometheus/</code></a></span></td>
                <td>Prometheus</td>
                <td><span class="pill warn">internal access</span></td>
                <td>Low-level metrics UI and query surface.</td>
              </tr>
              <tr>
                <td><span class="path-links"><a href="/nats/"><code>/nats/</code></a><a href="/nats-metrics/metrics"><code>/nats-metrics/metrics</code></a></span></td>
                <td>NATS monitor and exporter</td>
                <td><span class="pill warn">internal access</span></td>
                <td>NATS should usually be inspected through Grafana; these paths expose raw health and metrics.</td>
              </tr>
              <tr>
                <td><span class="path-links"><a href="/reaper/"><code>/reaper/</code></a><a href="/cron/"><code>/cron/</code></a></span></td>
                <td>Runtime service status</td>
                <td><span class="pill warn">internal access</span></td>
                <td>Gateway status surfaces for idle reaper and cron scheduler deployments.</td>
              </tr>
          </tbody>
        </table>
      </section>
      <section class="band">
        <h2>Security plan</h2>
        <ol>
          <li>Today: the public gateway keeps ops paths behind temporary internal access while bootstrap work is still in flight.</li>
          <li>Next: put TLS and identity-aware auth in front of the gateway using <code>auth_request</code>, oauth2-proxy, Cloudflare Access, or Tailscale.</li>
          <li>Keep worker, NATS client, and Kubernetes control services internal; only expose explicit web surfaces.</li>
          <li>Add Kubernetes NetworkPolicies and least-privilege service accounts so services can only talk to the namespaces they need.</li>
          <li>Replace the static header with signed JWT/HMAC service tokens for backend calls and SSO sessions for browser use.</li>
        </ol>
      </section>
    </main>
  </body>
</html>
"#,
        state.server_label,
        state.control_plane_label,
        state.workers_label,
        state.queue_consumer_label,
    );
    Html(body)
}

async fn agents_tasks_page() -> impl IntoResponse {
    record_request("GET", "/agents/tasks", StatusCode::OK);
    ui_document(
        "dd agents tasks",
        "#101417",
        "/assets/web-home/agents-tasks.css",
        "/assets/web-home/agents-tasks.js",
        agents_tasks_body(),
    )
}

async fn agents_threads_page() -> impl IntoResponse {
    record_request("GET", "/agents/threads", StatusCode::OK);
    ui_document(
        "dd agent threads",
        "#101417",
        "/assets/web-home/agents-threads.css",
        "/assets/web-home/agents-threads.js",
        agents_threads_body(),
    )
}

async fn agents_tasks_css() -> impl IntoResponse {
    text_asset(
        "/assets/web-home/agents-tasks.css",
        "text/css; charset=utf-8",
        AGENTS_TASKS_CSS,
    )
}

async fn agents_tasks_js() -> impl IntoResponse {
    text_asset(
        "/assets/web-home/agents-tasks.js",
        "text/javascript; charset=utf-8",
        AGENTS_TASKS_JS,
    )
}

async fn agents_tasks_html_fragment() -> impl IntoResponse {
    html_asset("/assets/web-home/agents-tasks.html", agents_tasks_body())
}

async fn agents_threads_css() -> impl IntoResponse {
    text_asset(
        "/assets/web-home/agents-threads.css",
        "text/css; charset=utf-8",
        AGENTS_THREADS_CSS,
    )
}

async fn agents_threads_js() -> impl IntoResponse {
    text_asset(
        "/assets/web-home/agents-threads.js",
        "text/javascript; charset=utf-8",
        AGENTS_THREADS_JS,
    )
}

async fn agents_threads_html_fragment() -> impl IntoResponse {
    html_asset(
        "/assets/web-home/agents-threads.html",
        agents_threads_body(),
    )
}

async fn lambda_functions_page() -> impl IntoResponse {
    record_request("GET", "/lambdas/functions", StatusCode::OK);
    Html(LAMBDA_FUNCTIONS_HTML)
}

fn ui_document(
    title: &str,
    theme_color: &str,
    stylesheet_path: &str,
    script_path: &str,
    body: Markup,
) -> Html<String> {
    Html(
        html! {
            (DOCTYPE)
            html lang="en" {
                head {
                    meta charset="utf-8";
                    meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover";
                    meta name="theme-color" content=(theme_color);
                    title { (title) }
                    link rel="stylesheet" href=(stylesheet_path);
                    script defer="defer" src=(script_path) {}
                }
                body {
                    (body)
                }
            }
        }
        .into_string(),
    )
}

fn text_asset(path: &'static str, content_type: &'static str, body: &'static str) -> Response {
    record_request("GET", path, StatusCode::OK);
    (
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, "public, max-age=60"),
        ],
        body,
    )
        .into_response()
}

fn html_asset(path: &'static str, body: Markup) -> Response {
    record_request("GET", path, StatusCode::OK);
    (
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CACHE_CONTROL, "public, max-age=60"),
        ],
        body.into_string(),
    )
        .into_response()
}

fn agents_threads_body() -> Markup {
    html! {
        div class="app" data-spa-root="agents-threads" {
            aside class="sidebar" {
                div class="topbar" {
                    div {
                        h1 { "Agent threads" }
                        p id="snapshot-meta" { "loading threads" }
                    }
                    button id="refresh" type="button" title="Refresh" { "Refresh" }
                }
                button id="new-thread" class="primary" type="button" { "New thread" }
                div id="thread-list" class="thread-list" aria-live="polite" {}
            }
            main class="main" {
                div class="topbar" {
                    div {
                        h1 id="selected-title" { "Select a thread" }
                        p id="selected-subtitle" { "Pick a thread from the sidebar or start a new one." }
                    }
                    div class="row" {
                        a href="/agents/tasks" { "Diagnostics table" }
                        a href="/home" { "Service directory" }
                    }
                }

                section class="panel prompt-panel" aria-label="Thread prompt" {
                    div class="form-grid" {
                        label {
                            span { "Thread UUID" }
                            input id="thread-id" autocomplete="off" spellcheck="false";
                        }
                        label {
                            span { "Task UUID" }
                            input id="task-id" autocomplete="off" spellcheck="false";
                        }
                        label {
                            span { "Provider" }
                            select id="provider" {
                                option value="claude-sdk" { "claude-sdk" }
                                option value="openai-sdk" { "openai-sdk" }
                                option value="gemini-sdk" { "gemini-sdk" }
                                option value="echo" { "echo" }
                            }
                        }
                        label class="field-wide" {
                            span { "Prompt" }
                            textarea id="prompt" placeholder="Ask this thread worker to do something" {}
                        }
                    }
                    div class="actions prompt-actions" {
                        button id="new-task" type="button" { "New task" }
                        button id="sleep-thread" type="button" { "Pause/Sleep (Reduce resources to container)" }
                        button id="archive-thread" class="warn" type="button" { "Archive (Deep Sleep - Suspend container?)" }
                        button id="delete-thread" class="danger" type="button" { "Delete (Delete Container)" }
                        button id="merge-thread" type="button" { "Merge with upstream" }
                        button id="open-pr-thread" type="button" { "Open draft PR" }
                        button id="send" class="primary" type="button" { "Send" }
                    }
                    p id="status-line" class="muted status-line" { "idle" }
                }

                div class="grid task-stream-grid" {
                    section class="panel" {
                        div class="topbar" {
                            h2 { "Previous tasks" }
                            span id="task-count" class="pill" { "0 tasks" }
                        }
                        div id="task-list" class="task-list" {}
                    }
                    section class="panel" {
                        div class="topbar" {
                            h2 { "Response stream" }
                            span id="stream-state" class="pill warn" { "no task selected" }
                        }
                        div id="stream" class="stream" aria-live="polite" {}
                    }
                }
            }
        }
    }
}

fn agents_tasks_body() -> Markup {
    html! {
        main class="shell" data-spa-root="agents-tasks" {
            div class="topbar" {
                div {
                    h1 { "Agent tasks" }
                    p { "Cluster-served view of remote-dev threads, tasks, PRs, and recent event output." }
                    div class="meta" {
                        a href="/home" { "Service directory" }
                        span id="source" class="pill" { "loading" }
                        span id="updated" { "waiting for first snapshot" }
                    }
                }
                div class="actions" {
                    select id="limit" {
                        option value="25" { "25 rows" }
                        option value="50" selected="selected" { "50 rows" }
                        option value="100" { "100 rows" }
                        option value="200" { "200 rows" }
                    }
                    button id="refresh" type="button" { "Refresh" }
                }
            }

            section class="grid" {
                div class="stat" { span { "Threads" } strong id="thread-count" { "0" } }
                div class="stat" { span { "Tasks" } strong id="task-count" { "0" } }
                div class="stat" { span { "Running" } strong id="running-count" { "0" } }
                div class="stat" { span { "Done" } strong id="done-count" { "0" } }
                div class="stat" { span { "Failed" } strong id="failed-count" { "0" } }
                div class="stat" { span { "PRs" } strong id="pr-count" { "0" } }
            }

            section class="band" {
                h2 { "Thread chat" }
                div class="chat-grid" {
                    label class="field" {
                        span { "Thread UUID" }
                        input id="chat-thread-id" autocomplete="off";
                    }
                    label class="field" {
                        span { "Task UUID" }
                        input id="chat-task-id" autocomplete="off";
                    }
                    label class="field" {
                        span { "Provider" }
                        select id="chat-provider" {
                            option value="claude-sdk" selected="selected" { "claude-sdk" }
                            option value="claude-cli" { "claude-cli" }
                            option value="echo" { "echo" }
                            option value="gemini-sdk" { "gemini-sdk" }
                            option value="openai-codex-cli" { "openai-codex-cli" }
                            option value="openai-sdk" { "openai-sdk" }
                        }
                    }
                    label class="field field-wide" {
                        span { "Prompt" }
                        textarea id="chat-prompt" {}
                    }
                }
                div class="actions" {
                    span id="chat-route" class="muted" {}
                    button id="new-thread" type="button" { "New thread" }
                    button id="new-task" type="button" { "New task" }
                    button id="thread-sleep" type="button" { "Pause/Sleep (Reduce resources to container)" }
                    button id="thread-archive" class="warn" type="button" { "Archive (Deep Sleep - Suspend container?)" }
                    button id="thread-delete" class="danger" type="button" { "Delete (Delete Container)" }
                    button id="thread-merge" type="button" { "Merge with upstream" }
                    button id="thread-open-pr" type="button" { "Open draft PR" }
                    button id="send-chat" type="button" { "Send" }
                }
                pre id="chat-stream" class="stream-box" { "No active stream." }
            }

            section id="errors" class="error-box" hidden="hidden" {}

            section class="band" {
                h2 { "Recent tasks" }
                table {
                    thead {
                        tr {
                            th style="width: 18%" { "Task" }
                            th style="width: 22%" { "Thread" }
                            th style="width: 23%" { "Prompt" }
                            th style="width: 11%" { "Status" }
                            th style="width: 10%" { "Events" }
                            th style="width: 16%" { "Branch / PR" }
                        }
                    }
                    tbody id="tasks-body" {
                        tr {
                            td colspan="6" class="muted" { "Loading tasks..." }
                        }
                    }
                }
            }

            section class="band" {
                h2 { "Threads" }
                table {
                    thead {
                        tr {
                            th style="width: 22%" { "Thread" }
                            th style="width: 21%" { "Title" }
                            th style="width: 18%" { "Repo" }
                            th style="width: 13%" { "Base" }
                            th style="width: 13%" { "Tasks" }
                            th style="width: 13%" { "Updated" }
                        }
                    }
                    tbody id="threads-body" {
                        tr {
                            td colspan="6" class="muted" { "Loading threads..." }
                        }
                    }
                }
            }
        }
    }
}

const AGENTS_THREADS_CSS: &str = r#"      :root {
        color-scheme: dark;
        --bg: #101417;
        --panel: #171d21;
        --panel-2: #202822;
        --panel-3: #161b24;
        --line: rgba(196, 181, 154, 0.24);
        --text: #f4f1e9;
        --muted: #b8b0a3;
        --accent: #6ee7b7;
        --accent-2: #facc15;
        --danger: #fb7185;
        --ok: #86efac;
        --warn: #fde047;
      }
      * { box-sizing: border-box; }
      html {
        height: 100%;
        overflow: hidden;
        -webkit-text-size-adjust: 100%;
      }
      body {
        margin: 0;
        height: 100%;
        min-height: 100vh;
        min-height: 100dvh;
        overflow: hidden;
        background: var(--bg);
        color: var(--text);
        font-family: Inter, ui-sans-serif, system-ui, -apple-system, Segoe UI, sans-serif;
      }
      a { color: var(--accent); text-decoration: none; }
      a:hover { text-decoration: underline; }
      button, select, input, textarea {
        border: 1px solid var(--line);
        border-radius: 7px;
        background: #121a18;
        color: var(--text);
        font: inherit;
        max-width: 100%;
      }
      button {
        min-height: 34px;
        padding: 7px 10px;
        cursor: pointer;
      }
      button:hover { border-color: rgba(110, 231, 183, 0.6); }
      button.primary {
        border-color: rgba(110, 231, 183, 0.65);
        background: rgba(20, 83, 45, 0.32);
        color: #dcfce7;
      }
      button.warn { border-color: rgba(250, 204, 21, 0.55); color: #fef9c3; }
      button.danger { border-color: rgba(251, 113, 133, 0.55); color: #ffe4e6; }
      button.icon {
        width: 34px;
        padding: 0;
        display: inline-grid;
        place-items: center;
      }
      input, select { min-height: 34px; padding: 7px 9px; width: 100%; }
      textarea {
        min-height: 112px;
        padding: 10px;
        width: 100%;
        max-height: 42dvh;
        overflow: auto;
        resize: vertical;
      }
      .app {
        height: 100vh;
        height: 100dvh;
        min-height: 0;
        display: grid;
        grid-template-columns: minmax(260px, 330px) minmax(0, 1fr);
        overflow: hidden;
      }
      .sidebar {
        border-right: 1px solid var(--line);
        background: #121715;
        padding: 18px;
        min-width: 0;
        min-height: 0;
        display: flex;
        flex-direction: column;
        overflow: hidden auto;
        overscroll-behavior: contain;
      }
      .sidebar * {
        min-width: 0;
        max-width: 100%;
      }
      .main {
        min-width: 0;
        min-height: 0;
        padding: 22px;
        display: flex;
        flex-direction: column;
        gap: 16px;
        overflow: hidden;
      }
      .topbar, .row, .actions {
        display: flex;
        align-items: center;
        gap: 10px;
        flex-wrap: wrap;
      }
      .topbar { justify-content: space-between; margin-bottom: 0; }
      .sidebar > .topbar {
        margin-bottom: 16px;
      }
      .topbar > div { min-width: 0; }
      h1 { margin: 0; font-size: 24px; }
      h2 { margin: 0 0 10px; font-size: 16px; }
      h3 { margin: 0; font-size: 14px; }
      p { margin: 0; color: var(--muted); line-height: 1.45; }
      .muted { color: var(--muted); }
      .pill {
        display: inline-flex;
        align-items: center;
        gap: 6px;
        border: 1px solid rgba(110, 231, 183, 0.35);
        border-radius: 999px;
        padding: 3px 8px;
        color: var(--accent);
        font-size: 12px;
        white-space: nowrap;
      }
      .pill.warn { border-color: rgba(250, 204, 21, 0.4); color: var(--warn); }
      .pill.bad { border-color: rgba(251, 113, 133, 0.4); color: var(--danger); }
      .thread-list {
        display: grid;
        align-content: start;
        gap: 8px;
        margin-top: 14px;
        min-height: 0;
        overflow: auto;
        overscroll-behavior: contain;
        padding-right: 3px;
      }
      .thread-item {
        width: 100%;
        min-width: 0;
        min-height: 78px;
        display: block;
        text-align: left;
        background: transparent;
        border-color: rgba(196, 181, 154, 0.18);
        overflow: hidden;
      }
      .thread-item.active {
        background: rgba(110, 231, 183, 0.08);
        border-color: rgba(110, 231, 183, 0.5);
      }
      .thread-title {
        display: block;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
      }
      .thread-meta {
        margin-top: 8px;
        display: flex;
        justify-content: space-between;
        gap: 8px;
        color: var(--muted);
        font-size: 12px;
        min-width: 0;
      }
      .thread-meta span {
        min-width: 0;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
      }
      .thread-meta > span {
        min-width: 0;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
      }
      .panel {
        border: 1px solid var(--line);
        border-radius: 8px;
        background: var(--panel);
        padding: 14px;
        min-height: 0;
      }
      .prompt-panel {
        flex: 0 0 auto;
        overflow: visible;
        position: relative;
        z-index: 1;
      }
      .prompt-panel label,
      .form-grid > label,
      .field-wide {
        min-width: 0;
      }
      .prompt-actions,
      .status-line {
        margin-top: 12px;
      }
      .grid {
        display: grid;
        grid-template-columns: minmax(0, 0.82fr) minmax(0, 1.18fr);
        gap: 14px;
        min-height: 0;
        overflow: hidden;
      }
      .task-stream-grid {
        margin-top: 6px;
      }
      .form-grid {
        display: grid;
        grid-template-columns: minmax(0, 1fr) minmax(0, 1fr) minmax(140px, 0.35fr);
        gap: 10px;
        min-width: 0;
        align-items: start;
      }
      .field-wide { grid-column: 1 / -1; }
      label span {
        display: block;
        color: var(--muted);
        font-size: 12px;
        margin-bottom: 5px;
      }
      .task-list {
        display: grid;
        align-content: start;
        gap: 8px;
        min-height: 0;
        max-height: none;
        overflow: auto;
        overscroll-behavior: contain;
      }
      .task-item {
        display: grid;
        gap: 6px;
        width: 100%;
        min-width: 0;
        text-align: left;
        background: var(--panel-3);
        overflow: hidden;
      }
      .task-item.active { border-color: rgba(250, 204, 21, 0.55); }
      .stream {
        display: grid;
        align-content: start;
        gap: 10px;
        min-height: 0;
        max-height: none;
        overflow: auto;
        overscroll-behavior: contain;
        padding-right: 3px;
      }
      .main > .topbar,
      .main > .panel {
        flex: 0 0 auto;
      }
      .main > .grid {
        flex: 1 1 auto;
      }
      .grid > .panel {
        min-height: 0;
        display: flex;
        flex-direction: column;
        overflow: hidden;
      }
      .grid > .panel > .topbar {
        flex: 0 0 auto;
        margin-bottom: 12px;
      }
      .grid > .panel > .task-list,
      .grid > .panel > .stream {
        flex: 1 1 auto;
      }
      .event {
        border: 1px solid rgba(196, 181, 154, 0.18);
        border-radius: 8px;
        background: var(--panel-3);
        padding: 12px;
      }
      .event.agent {
        background: rgba(34, 61, 49, 0.54);
        border-color: rgba(110, 231, 183, 0.34);
      }
      .event.error {
        border-color: rgba(251, 113, 133, 0.42);
      }
      .event-head {
        display: flex;
        justify-content: space-between;
        gap: 10px;
        align-items: center;
        margin-bottom: 8px;
      }
      .event-text {
        margin: 0;
        white-space: pre-wrap;
        overflow-wrap: anywhere;
        line-height: 1.45;
      }
      .vote-row {
        margin-top: 10px;
        display: flex;
        gap: 8px;
      }
      code {
        color: var(--accent-2);
        overflow-wrap: anywhere;
      }
      @media (max-width: 980px) {
        .app {
          grid-template-columns: 1fr;
          grid-template-rows: minmax(150px, 28dvh) minmax(0, 1fr);
        }
        .sidebar { border-right: 0; border-bottom: 1px solid var(--line); }
        .main {
          overflow: hidden auto;
          overscroll-behavior: contain;
        }
        .main > .grid {
          flex: 0 0 auto;
          min-height: min(360px, 52dvh);
        }
        .grid, .form-grid { grid-template-columns: 1fr; }
      }

      @media (max-width: 640px) {
        button, select, input, textarea { font-size: 16px; }
        .sidebar, .main { padding: 14px; }
        .topbar { align-items: stretch; }
        .topbar > div { min-width: 0; }
        .row, .actions { width: 100%; align-items: stretch; }
        .actions > *, .row a, .topbar button, #new-thread { width: 100%; }
        .thread-list, .task-list, .stream { max-height: none; }
        h1 { font-size: 22px; }
        h2 { font-size: 17px; }
      }
"#;

const AGENTS_THREADS_JS: &str = r#"      const $ = (id) => document.getElementById(id);
      const state = {
        snapshot: null,
        threads: [],
        tasks: [],
        selectedThreadId: null,
        selectedTaskId: null,
        liveSource: null,
        renderedEvents: new Set(),
        runtimePoll: null,
        lastRuntimeSummary: "",
      };

      function makeUuid() {
        if (crypto.randomUUID) return crypto.randomUUID();
        return "xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx".replace(/[xy]/g, (char) => {
          const value = Math.random() * 16 | 0;
          return (char === "x" ? value : (value & 0x3) | 0x8).toString(16);
        });
      }

      function shortId(id) {
        return String(id || "").replace(/-/g, "").slice(0, 12) || "new-thread";
      }

      function fmt(value) {
        if (!value) return "unknown";
        const date = new Date(value);
        return Number.isNaN(date.getTime()) ? String(value) : date.toLocaleString();
      }

      function textNode(value) {
        return document.createTextNode(String(value ?? ""));
      }

      function setStatus(message, bad = false) {
        $("status-line").textContent = message;
        $("status-line").style.color = bad ? "var(--danger)" : "var(--muted)";
      }

      function setStreamState(message, kind = "warn") {
        const node = $("stream-state");
        node.textContent = message;
        node.className = kind === "bad" ? "pill bad" : kind === "ok" ? "pill" : "pill warn";
      }

      function threadTasks(threadId) {
        return state.tasks
          .filter((task) => task.threadId === threadId)
          .sort((a, b) => String(b.createdAt || "").localeCompare(String(a.createdAt || "")));
      }

      function renderThreads() {
        const list = $("thread-list");
        list.textContent = "";
        if (!state.threads.length) {
          const empty = document.createElement("p");
          empty.className = "muted";
          empty.textContent = "No threads found yet.";
          list.appendChild(empty);
          return;
        }
        for (const thread of state.threads) {
          const button = document.createElement("button");
          button.type = "button";
          button.className = thread.id === state.selectedThreadId ? "thread-item active" : "thread-item";
          const title = document.createElement("span");
          title.className = "thread-title";
          title.textContent = thread.title || "Remote thread";
          const meta = document.createElement("span");
          meta.className = "thread-meta";
          const left = document.createElement("span");
          left.textContent = shortId(thread.id);
          const right = document.createElement("span");
          right.textContent = `${thread.taskCount || threadTasks(thread.id).length || 0} tasks`;
          meta.append(left, right);
          button.append(title, meta);
          button.addEventListener("click", () => selectThread(thread.id));
          list.appendChild(button);
        }
      }

      function renderTaskList() {
        const tasks = state.selectedThreadId ? threadTasks(state.selectedThreadId) : [];
        $("task-count").textContent = `${tasks.length} tasks`;
        const list = $("task-list");
        list.textContent = "";
        if (!tasks.length) {
          const empty = document.createElement("p");
          empty.className = "muted";
          empty.textContent = "No tasks for this thread yet.";
          list.appendChild(empty);
          return;
        }
        for (const task of tasks) {
          const button = document.createElement("button");
          button.type = "button";
          button.className = task.id === state.selectedTaskId ? "task-item active" : "task-item";
          const head = document.createElement("span");
          head.className = "row";
          const id = document.createElement("code");
          id.textContent = shortId(task.id);
          const pill = document.createElement("span");
          pill.className = task.status === "failed" ? "pill bad" : task.status === "pr_open" || task.status === "done" ? "pill" : "pill warn";
          pill.textContent = task.status || "unknown";
          head.append(id, pill);
          const prompt = document.createElement("span");
          prompt.className = "muted";
          prompt.textContent = task.prompt || "No prompt";
          const meta = document.createElement("span");
          meta.className = "muted";
          meta.textContent = `${task.eventCount || 0} events · ${fmt(task.createdAt)}`;
          button.append(head, prompt, meta);
          button.addEventListener("click", () => selectTask(task.id));
          list.appendChild(button);
        }
      }

      function updateSelectionHeader() {
        const thread = state.threads.find((item) => item.id === state.selectedThreadId);
        $("selected-title").textContent = thread?.title || "Remote thread";
        $("selected-subtitle").textContent = state.selectedThreadId
          ? `${state.selectedThreadId} · ${threadTasks(state.selectedThreadId).length} tasks`
          : "Pick a thread from the sidebar or start a new one.";
        $("thread-id").value = state.selectedThreadId || "";
        if (!state.selectedTaskId) $("task-id").value = makeUuid();
      }

      function selectThread(threadId) {
        state.selectedThreadId = threadId;
        const tasks = threadTasks(threadId);
        state.selectedTaskId = tasks[0]?.id || null;
        const url = new URL(window.location.href);
        url.searchParams.set("thread", threadId);
        if (state.selectedTaskId) url.searchParams.set("task", state.selectedTaskId);
        window.history.replaceState(null, "", url);
        renderThreads();
        updateSelectionHeader();
        renderTaskList();
        if (state.selectedTaskId) {
          $("task-id").value = state.selectedTaskId;
          loadTaskEvents(state.selectedTaskId).catch((error) => renderError(`events load failed: ${String(error)}`));
        } else {
          clearStream("No task selected.");
        }
      }

      function selectTask(taskId) {
        state.selectedTaskId = taskId;
        $("task-id").value = taskId;
        const url = new URL(window.location.href);
        url.searchParams.set("task", taskId);
        window.history.replaceState(null, "", url);
        renderTaskList();
        loadTaskEvents(taskId).catch((error) => renderError(`events load failed: ${String(error)}`));
      }

      function clearStream(message) {
        state.renderedEvents.clear();
        $("stream").textContent = "";
        setStreamState(message || "waiting", "warn");
      }

      function eventPayload(row) {
        return row?.event || row?.payload?.event || row?.payload || row || {};
      }

      function eventKind(row) {
        const payload = eventPayload(row);
        return row?.eventKind || payload.kind || payload.type || "event";
      }

      function collectText(value, out = [], depth = 0) {
        if (out.length > 8 || depth > 5 || value == null) return out;
        if (typeof value === "string") {
          const trimmed = value.trim();
          if (trimmed && trimmed.length <= 4000) out.push(trimmed);
          return out;
        }
        if (Array.isArray(value)) {
          for (const item of value) collectText(item, out, depth + 1);
          return out;
        }
        if (typeof value === "object") {
          for (const key of ["text", "content", "outputText", "output_text", "delta", "message", "result", "summary", "status", "error"]) {
            if (Object.prototype.hasOwnProperty.call(value, key)) collectText(value[key], out, depth + 1);
          }
          if (!out.length) {
            for (const item of Object.values(value).slice(0, 10)) collectText(item, out, depth + 1);
          }
        }
        return out;
      }

      function eventText(row) {
        const payload = eventPayload(row);
        if (payload.kind === "status") return [payload.status, payload.message].filter(Boolean).join("\n") || "status";
        if (payload.kind === "stderr") return payload.text || "stderr";
        if (payload.kind === "error") return payload.message || "agent error";
        if (payload.kind === "done") return payload.errorMessage || payload.exitReason || "done";
        if (payload.kind === "pr_open") return [payload.prUrl, payload.draft ? "draft" : ""].filter(Boolean).join("\n") || "PR opened";
        if (payload.kind === "feedback") return `feedback: ${payload.vote || "unknown"}`;
        const raw = payload.raw || payload;
        const text = collectText(raw).filter(Boolean);
        if (text.length) return [...new Set(text)].join("\n");
        try {
          return JSON.stringify(payload, null, 2);
        } catch (_error) {
          return String(payload);
        }
      }

      function renderError(message) {
        renderEventRow({
          seq: `error-${Date.now()}`,
          eventKind: "error",
          payload: { kind: "error", message },
          createdAt: new Date().toISOString(),
        });
      }

      function renderEventRow(row) {
        const seq = row.seq ?? row.payload?.seq ?? Date.now();
        const key = `${state.selectedTaskId || "task"}:${seq}:${eventKind(row)}`;
        if (state.renderedEvents.has(key)) return;
        state.renderedEvents.add(key);
        const kind = eventKind(row);
        const text = eventText(row);
        const item = document.createElement("article");
        item.className = `event ${kind === "claude" ? "agent" : kind === "error" ? "error" : ""}`;
        const head = document.createElement("div");
        head.className = "event-head";
        const left = document.createElement("span");
        left.className = kind === "error" ? "pill bad" : kind === "done" || kind === "claude" ? "pill" : "pill warn";
        left.textContent = `${kind} · seq ${seq}`;
        const right = document.createElement("span");
        right.className = "muted";
        right.textContent = fmt(row.createdAt);
        head.append(left, right);
        const body = document.createElement("pre");
        body.className = "event-text";
        body.appendChild(textNode(text));
        item.append(head, body);
        if (kind === "claude" || kind === "error" || kind === "stderr") {
          const votes = document.createElement("div");
          votes.className = "vote-row";
          for (const vote of ["up", "down"]) {
            const button = document.createElement("button");
            button.type = "button";
            button.className = "icon";
            button.title = vote === "up" ? "Upvote this response" : "Downvote this response";
            button.textContent = vote === "up" ? "+" : "-";
            button.addEventListener("click", () => sendFeedback(seq, vote, button));
            votes.appendChild(button);
          }
          item.appendChild(votes);
        }
        $("stream").appendChild(item);
        $("stream").scrollTop = $("stream").scrollHeight;
        setStreamState("showing events", "ok");
      }

      function workerRuntimeSummary(data) {
        const summary = data?.summary || {};
        const deployment = data?.deployment || {};
        const pods = data?.pods || [];
        if (data?.errors?.length) return `worker state unavailable: ${data.errors[0]}`;
        if (!deployment.name) return "worker deployment not created yet";
        if (summary.desiredReplicas === 0) return "worker sleeping: desired replicas 0";
        const waiting = pods.flatMap((pod) => (pod.containers || []).map((container) => ({
          pod: pod.name,
          name: container.name,
          waiting: container.state?.waiting,
          running: container.state?.running,
          ready: container.ready,
          restarts: container.restartCount || 0,
        }))).find((container) => container.waiting);
        if (waiting) {
          return `worker starting: ${waiting.pod}/${waiting.name} waiting ${waiting.waiting.reason || "unknown"}`;
        }
        if (summary.phase === "ready") {
          return `worker ready: ${summary.availableReplicas}/${summary.desiredReplicas} replicas available, ${summary.readyPodCount}/${summary.podCount} pods ready`;
        }
        if (pods.length) {
          const phases = pods.map((pod) => `${pod.name || "pod"} ${pod.phase || "unknown"}`).join(", ");
          return `worker ${summary.phase || "starting"}: ${phases}`;
        }
        return `worker ${summary.phase || "creating"}: desired ${summary.desiredReplicas}, ready ${summary.readyReplicas || 0}`;
      }

      async function loadRuntimeState(threadId, render = true) {
        if (!threadId) return null;
        const response = await fetch(`/api/agents/threads/${encodeURIComponent(threadId)}/runtime`, { cache: "no-store" });
        if (!response.ok) throw new Error(`runtime request failed ${response.status}`);
        const data = await response.json();
        const summary = workerRuntimeSummary(data);
        setStatus(summary, Boolean(data.errors?.length));
        if (render && summary !== state.lastRuntimeSummary) {
          state.lastRuntimeSummary = summary;
          renderEventRow({
            seq: `runtime-${Date.now()}`,
            eventKind: "status",
            payload: { kind: "status", status: "worker runtime", message: summary },
            createdAt: new Date().toISOString(),
          });
        }
        return data;
      }

      function stopRuntimePolling() {
        if (state.runtimePoll) clearInterval(state.runtimePoll);
        state.runtimePoll = null;
      }

      function startRuntimePolling(threadId) {
        stopRuntimePolling();
        state.lastRuntimeSummary = "";
        loadRuntimeState(threadId).catch((error) => setStatus(String(error), true));
        state.runtimePoll = setInterval(() => {
          loadRuntimeState(threadId).catch((error) => setStatus(String(error), true));
        }, 5000);
      }

      async function sendFeedback(seq, vote, button) {
        if (!state.selectedTaskId) return;
        button.disabled = true;
        const response = await fetch(`/api/agents/tasks/${encodeURIComponent(state.selectedTaskId)}/feedback`, {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({ targetSeq: Number(seq), vote }),
        });
        if (!response.ok) {
          button.disabled = false;
          renderError(`feedback failed ${response.status}`);
          return;
        }
        button.textContent = vote === "up" ? "ok" : "noted";
        const data = await response.json().catch(() => null);
        if (data?.event) renderEventRow(data.event);
      }

      async function loadTaskEvents(taskId) {
        clearStream("loading events");
        const response = await fetch(`/api/agents/tasks/${encodeURIComponent(taskId)}/events?limit=250`, { cache: "no-store" });
        if (!response.ok) throw new Error(`events request failed ${response.status}`);
        const data = await response.json();
        if (data.errors?.length) renderError(data.errors.join("\n"));
        if (!data.events?.length) {
          setStreamState("no stored events yet", "warn");
          const empty = document.createElement("p");
          empty.className = "muted";
          empty.textContent = "No stored response events for this task yet.";
          $("stream").appendChild(empty);
          return;
        }
        for (const event of data.events) renderEventRow(event);
      }

      function openLiveStream(threadId, taskId) {
        if (state.liveSource) state.liveSource.close();
        const source = new EventSource(`/api/agents/threads/${encodeURIComponent(threadId)}/stream/${encodeURIComponent(taskId)}`);
        state.liveSource = source;
        setStreamState("live stream connecting", "warn");
        source.onmessage = (message) => {
          if (!message.data) return;
          try {
            const parsed = JSON.parse(message.data);
            renderEventRow(parsed);
          } catch (_error) {
            renderEventRow({
              seq: `sse-${Date.now()}`,
              eventKind: "message",
              payload: { kind: "message", text: message.data },
              createdAt: new Date().toISOString(),
            });
          }
        };
        source.onerror = () => {
          setStreamState("live stream disconnected", "bad");
        };
      }

      async function loadSnapshot() {
        const response = await fetch("/api/agents/tasks?limit=200", { cache: "no-store" });
        if (!response.ok) throw new Error(`snapshot failed ${response.status}`);
        const data = await response.json();
        state.snapshot = data;
        state.threads = data.threads || [];
        state.tasks = data.tasks || [];
        $("snapshot-meta").textContent = `${state.threads.length} threads · ${state.tasks.length} tasks · ${data.source || "unknown"}`;
        const params = new URLSearchParams(window.location.search);
        const requestedThread = params.get("thread");
        const requestedTask = params.get("task");
        if (requestedThread && state.threads.some((thread) => thread.id === requestedThread)) {
          state.selectedThreadId = requestedThread;
        }
        if (!state.selectedThreadId && state.threads.length) state.selectedThreadId = state.threads[0].id;
        if (requestedTask && state.tasks.some((task) => task.id === requestedTask)) state.selectedTaskId = requestedTask;
        if (!state.selectedTaskId && state.selectedThreadId) state.selectedTaskId = threadTasks(state.selectedThreadId)[0]?.id || null;
        renderThreads();
        updateSelectionHeader();
        renderTaskList();
        if (state.selectedTaskId) {
          $("task-id").value = state.selectedTaskId;
          await loadTaskEvents(state.selectedTaskId);
        }
      }

      async function dispatchPrompt() {
        const threadId = $("thread-id").value.trim() || makeUuid();
        const taskId = $("task-id").value.trim() || makeUuid();
        const prompt = $("prompt").value.trim();
        const provider = $("provider").value;
        if (!prompt) {
          setStatus("prompt is required", true);
          return;
        }
        state.selectedThreadId = threadId;
        state.selectedTaskId = taskId;
        clearStream("waking worker");
        startRuntimePolling(threadId);
        renderEventRow({
          seq: `dispatch-start-${Date.now()}`,
          eventKind: "status",
          payload: {
            kind: "status",
            status: "waking worker",
            message: "Creating or waking the UUID-bound worker. Cold starts can take 30-90 seconds while the container installs dependencies, refreshes git, and starts Node.",
          },
          createdAt: new Date().toISOString(),
        });
        setStatus(`POST /api/agents/threads/${threadId}/tasks`);
        const startedAt = Date.now();
        const waitTicker = setInterval(() => {
          const elapsed = Math.round((Date.now() - startedAt) / 1000);
          setStatus(`dispatch still waiting after ${elapsed}s; worker cold-start may still be running`);
          renderEventRow({
            seq: `dispatch-wait-${elapsed}`,
            eventKind: "status",
            payload: {
              kind: "status",
              status: `still waiting (${elapsed}s)`,
              message: "The REST API is waiting for the thread worker readiness check before it forwards the task.",
            },
            createdAt: new Date().toISOString(),
          });
        }, 15000);
        let response;
        try {
          response = await fetch(`/api/agents/threads/${encodeURIComponent(threadId)}/tasks`, {
            method: "POST",
            headers: { "content-type": "application/json" },
            body: JSON.stringify({
              threadId,
              taskId,
              prompt,
              provider,
              threadTitle: prompt.slice(0, 80),
            }),
          });
        } finally {
          clearInterval(waitTicker);
          stopRuntimePolling();
        }
        const body = await response.text();
        if (!response.ok) {
          renderError(`dispatch failed ${response.status}: ${body.slice(0, 700)}`);
          setStatus("dispatch failed", true);
          return;
        }
        setStatus("dispatch accepted");
        renderEventRow({
          seq: `dispatch-accepted-${Date.now()}`,
          eventKind: "status",
          payload: {
            kind: "status",
            status: "dispatch accepted",
            message: body.slice(0, 700),
          },
          createdAt: new Date().toISOString(),
        });
        await loadRuntimeState(threadId).catch((error) => setStatus(String(error), true));
        openLiveStream(threadId, taskId);
        await loadSnapshot().catch((error) => renderError(`snapshot refresh failed: ${String(error)}`));
      }

      async function threadControl(action) {
        const threadId = $("thread-id").value.trim();
        if (!threadId) {
          setStatus("thread id is required", true);
          return;
        }
        const taskId = $("task-id").value.trim() || makeUuid();
        const routeAction = action === "delete" ? "hard-delete" : action === "merge" ? "merge-upstream" : action === "open-pr" ? "open-pr" : action;
        const response = await fetch(`/api/agents/threads/${encodeURIComponent(threadId)}/${routeAction}`, {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({
            kind: "thread-control",
            action: routeAction,
            threadId,
            taskId,
            requestedBy: "agents-threads-ui",
          }),
        });
        const body = await response.text();
        renderEventRow({
          seq: `control-${Date.now()}`,
          eventKind: response.ok ? "status" : "error",
          payload: {
            kind: response.ok ? "status" : "error",
            status: `${routeAction} ${response.status}`,
            message: body.slice(0, 700),
          },
          createdAt: new Date().toISOString(),
        });
        if (!response.ok) setStatus(`${routeAction} failed`, true);
        else {
          setStatus(`${routeAction} accepted`);
          await loadSnapshot().catch((error) => renderError(`snapshot refresh failed: ${String(error)}`));
        }
      }

      $("refresh").addEventListener("click", () => loadSnapshot().catch((error) => setStatus(String(error), true)));
      $("new-thread").addEventListener("click", () => {
        state.selectedThreadId = makeUuid();
        state.selectedTaskId = null;
        $("thread-id").value = state.selectedThreadId;
        $("task-id").value = makeUuid();
        $("prompt").focus();
        updateSelectionHeader();
        renderTaskList();
        clearStream("new thread ready");
      });
      $("new-task").addEventListener("click", () => {
        state.selectedTaskId = null;
        $("task-id").value = makeUuid();
        clearStream("new task ready");
      });
      $("thread-id").addEventListener("change", () => {
        state.selectedThreadId = $("thread-id").value.trim();
        updateSelectionHeader();
        renderThreads();
        renderTaskList();
      });
      $("task-id").addEventListener("change", () => {
        state.selectedTaskId = $("task-id").value.trim();
      });
      $("send").addEventListener("click", () => dispatchPrompt().catch((error) => renderError(`dispatch error: ${String(error)}`)));
      $("sleep-thread").addEventListener("click", () => threadControl("sleep").catch((error) => renderError(String(error))));
      $("archive-thread").addEventListener("click", () => threadControl("archive").catch((error) => renderError(String(error))));
      $("delete-thread").addEventListener("click", () => threadControl("delete").catch((error) => renderError(String(error))));
      $("merge-thread").addEventListener("click", () => threadControl("merge").catch((error) => renderError(String(error))));
      $("open-pr-thread").addEventListener("click", () => threadControl("open-pr").catch((error) => renderError(String(error))));

      loadSnapshot().catch((error) => {
        $("snapshot-meta").textContent = "snapshot failed";
        setStatus(String(error), true);
      });
"#;

const AGENTS_TASKS_CSS: &str = r#"      :root {
        color-scheme: dark;
        --bg: #0b1117;
        --panel: #111923;
        --panel-2: #0f1720;
        --line: rgba(148, 163, 184, 0.24);
        --text: #eef2f6;
        --muted: #a8b3c1;
        --accent: #5eead4;
        --danger: #f87171;
        --ok: #86efac;
        --warn: #fbbf24;
      }
      * { box-sizing: border-box; }
      body {
        margin: 0;
        min-height: 100vh;
        background: var(--bg);
        color: var(--text);
        font-family: Inter, ui-sans-serif, system-ui, -apple-system, Segoe UI, sans-serif;
        padding: 24px;
      }
      .shell { max-width: 1320px; margin: 0 auto; }
      .topbar {
        display: flex;
        align-items: flex-start;
        justify-content: space-between;
        gap: 16px;
        margin-bottom: 18px;
      }
      h1 { margin: 0 0 8px; font-size: 30px; }
      h2 { margin: 0 0 12px; font-size: 17px; }
      p { margin: 0; color: var(--muted); line-height: 1.5; }
      a { color: var(--accent); text-decoration: none; }
      a:hover { text-decoration: underline; }
      button, select, input, textarea {
        min-height: 34px;
        border: 1px solid var(--line);
        border-radius: 7px;
        background: #121c27;
        color: var(--text);
        padding: 7px 10px;
        font: inherit;
      }
      textarea {
        width: 100%;
        min-height: 116px;
        resize: vertical;
      }
      button { cursor: pointer; }
      button.danger { border-color: rgba(248, 113, 113, 0.45); color: #fecaca; }
      button.warn { border-color: rgba(251, 191, 36, 0.45); color: #fde68a; }
      button.ok {
        border-color: rgba(134, 239, 172, 0.65);
        color: #dcfce7;
        background: rgba(22, 101, 52, 0.28);
      }
      .actions { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; justify-content: flex-end; }
      .chat-grid {
        display: grid;
        grid-template-columns: minmax(0, 1.35fr) minmax(0, 1.35fr) minmax(160px, 0.7fr);
        gap: 10px;
      }
      .field span {
        display: block;
        margin-bottom: 6px;
        color: var(--muted);
        font-size: 12px;
      }
      .field input, .field select { width: 100%; }
      .field-wide { grid-column: 1 / -1; }
      .grid {
        display: grid;
        grid-template-columns: repeat(6, minmax(0, 1fr));
        gap: 12px;
        margin: 18px 0;
      }
      .stat, .band {
        border: 1px solid var(--line);
        border-radius: 8px;
        background: var(--panel);
      }
      .stat { padding: 13px; min-height: 82px; }
      .stat span { display: block; color: var(--muted); font-size: 11px; text-transform: uppercase; letter-spacing: 0.08em; }
      .stat strong { display: block; margin-top: 8px; font-size: 28px; }
      .band { padding: 16px; margin-top: 16px; overflow: hidden; }
      .meta {
        display: flex;
        gap: 10px;
        align-items: center;
        flex-wrap: wrap;
        margin-top: 8px;
        color: var(--muted);
        font-size: 13px;
      }
      .pill {
        display: inline-flex;
        align-items: center;
        min-height: 24px;
        border: 1px solid rgba(94, 234, 212, 0.35);
        border-radius: 999px;
        padding: 2px 8px;
        color: var(--accent);
        background: rgba(94, 234, 212, 0.08);
        font-size: 12px;
      }
      .pill.bad {
        border-color: rgba(248, 113, 113, 0.35);
        color: var(--danger);
        background: rgba(248, 113, 113, 0.08);
      }
      table {
        width: 100%;
        border-collapse: collapse;
        table-layout: fixed;
        font-size: 13px;
      }
      th, td {
        border-top: 1px solid var(--line);
        padding: 11px 9px;
        text-align: left;
        vertical-align: top;
        line-height: 1.4;
      }
      th { color: var(--muted); font-weight: 600; }
      code {
        display: inline-block;
        max-width: 100%;
        overflow-wrap: anywhere;
        border: 1px solid rgba(148, 163, 184, 0.2);
        border-radius: 6px;
        padding: 2px 5px;
        background: #0a1017;
        color: #d7fbf4;
        font-size: 12px;
      }
      .prompt {
        display: -webkit-box;
        -webkit-line-clamp: 3;
        -webkit-box-orient: vertical;
        overflow: hidden;
      }
      .muted { color: var(--muted); }
      .status-running { color: var(--warn); }
      .status-failed { color: var(--danger); }
      .status-done { color: var(--ok); }
      .error-box {
        border: 1px solid rgba(248, 113, 113, 0.35);
        border-radius: 8px;
        background: rgba(248, 113, 113, 0.08);
        color: #fecaca;
        padding: 12px;
        margin-top: 16px;
        white-space: pre-wrap;
      }
      .stream-box {
        min-height: 160px;
        max-height: 360px;
        overflow: auto;
        margin: 12px 0 0;
        border: 1px solid var(--line);
        border-radius: 8px;
        background: #080d13;
        color: #d7fbf4;
        padding: 12px;
        white-space: pre-wrap;
        font: 12px/1.5 ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
      }
      @media (max-width: 1000px) {
        .topbar { display: block; }
        .actions { justify-content: flex-start; margin-top: 12px; }
        .chat-grid { grid-template-columns: 1fr; }
        .grid { grid-template-columns: repeat(2, minmax(0, 1fr)); }
        table, thead, tbody, th, td, tr { display: block; }
        th { display: none; }
        tr { border-top: 1px solid var(--line); padding: 10px 0; }
        td { border-top: 0; padding: 5px 0; }
      }

      html { -webkit-text-size-adjust: 100%; }
      .shell { min-height: 100dvh; }
      @media (max-width: 640px) {
        body { padding: 14px; overflow-x: hidden; }
        button, select, input, textarea { font-size: 16px; }
        h1 { font-size: 24px; }
        .grid, .chat-grid { grid-template-columns: 1fr; }
        .topbar { align-items: stretch; }
        .actions, .meta { width: 100%; align-items: stretch; }
        .actions > *, .meta > * { width: 100%; }
        .band { padding: 13px; }
        .stream-box { max-height: 58vh; }
      }
"#;

const AGENTS_TASKS_JS: &str = r#"      const $ = (id) => document.getElementById(id);
      const empty = (value, fallback = "none") => value === null || value === undefined || value === "" ? fallback : value;
      const fmt = (value) => {
        if (!value) return "none";
        const time = new Date(value);
        return Number.isNaN(time.getTime()) ? value : time.toLocaleString();
      };
      const newUuid = () => {
        const webCrypto = globalThis.crypto;
        if (webCrypto && typeof webCrypto.randomUUID === "function") return webCrypto.randomUUID();
        return "10000000-1000-4000-8000-100000000000".replace(/[018]/g, (c) =>
          (Number(c) ^ webCrypto.getRandomValues(new Uint8Array(1))[0] & 15 >> Number(c) / 4).toString(16)
        );
      };
      const threadShort = (threadId) => String(threadId || "").replace(/[^a-z0-9]/gi, "").slice(0, 12).toLowerCase();
      const threadIngressPrefix = (threadId) => `/dd-thread/${threadShort(threadId)}`;
      let activeStream = null;
      let activeWs = null;
      const workerSockets = new Map();
      let activeTaskKey = null;
      let seenStreamEvents = new Set();
      const threadRuntimeStates = new Map();
      const sleepingStatuses = new Set(["sleeping", "archived", "suspended"]);
      const statusClass = (status) => {
        if (["queued", "running", "streaming"].includes(status)) return "status-running";
        if (["failed", "cancelled"].includes(status)) return "status-failed";
        return "status-done";
      };
      const text = (value) => document.createTextNode(empty(value));
      const cell = (child, className) => {
        const td = document.createElement("td");
        if (className) td.className = className;
        if (typeof child === "string") td.appendChild(text(child));
        else td.appendChild(child);
        return td;
      };
      const code = (value) => {
        const el = document.createElement("code");
        el.textContent = empty(value);
        return el;
      };
      const shortId = (value) => value ? value.slice(0, 8) : "none";
      const link = (href, label) => {
        const a = document.createElement("a");
        a.href = href;
        a.textContent = label;
        a.target = "_blank";
        a.rel = "noreferrer";
        return a;
      };
      const setStat = (id, value) => { $(id).textContent = String(value || 0); };
      const setChatRoute = () => {
        const threadId = $("chat-thread-id").value.trim();
        $("chat-route").textContent = threadId ? `/api/agents/threads/${threadId}/tasks` : "";
        updateThreadRuntimeControls();
      };
      const resetTaskId = () => {
        $("chat-task-id").value = newUuid();
      };
      const resetThreadId = () => {
        $("chat-thread-id").value = newUuid();
        resetTaskId();
        setChatRoute();
      };
      const appendStreamLine = (line) => {
        const stream = $("chat-stream");
        if (stream.textContent === "No active stream.") stream.textContent = "";
        stream.textContent += `${line}\n`;
        stream.scrollTop = stream.scrollHeight;
      };
      const setThreadRuntimeState = (threadId, status, detail = {}) => {
        if (!threadId || !status) return;
        threadRuntimeStates.set(threadId, {
          status,
          action: detail.action || "",
          message: detail.message || "",
          at: Date.now()
        });
        updateThreadRuntimeControls();
      };
      const runtimeSummary = (data) => {
        const summary = data?.summary || {};
        const deployment = data?.deployment || {};
        const pods = data?.pods || [];
        if (data?.errors?.length) return `worker state unavailable: ${data.errors[0]}`;
        if (!deployment.name) return "worker deployment not created yet";
        if (summary.desiredReplicas === 0) return "worker sleeping: desired replicas 0";
        const waiting = pods.flatMap((pod) => (pod.containers || []).map((container) => ({
          pod: pod.name,
          name: container.name,
          waiting: container.state?.waiting,
          ready: container.ready
        }))).find((container) => container.waiting);
        if (waiting) return `worker starting: ${waiting.pod}/${waiting.name} waiting ${waiting.waiting.reason || "unknown"}`;
        if (summary.phase === "ready") return `worker ready: ${summary.availableReplicas}/${summary.desiredReplicas} replicas available`;
        if (pods.length) return `worker ${summary.phase || "starting"}: ${pods.map((pod) => `${pod.name || "pod"} ${pod.phase || "unknown"}`).join(", ")}`;
        return `worker ${summary.phase || "creating"}: desired ${summary.desiredReplicas}, ready ${summary.readyReplicas || 0}`;
      };
      const fetchRuntimeSummary = async (threadId) => {
        const response = await fetch(`/api/agents/threads/${encodeURIComponent(threadId)}/runtime`, { cache: "no-store" });
        if (!response.ok) throw new Error(`runtime request failed ${response.status}`);
        const data = await response.json();
        const summary = runtimeSummary(data);
        setThreadRuntimeState(threadId, data?.summary?.phase || "unknown", { action: "runtime", message: summary });
        return summary;
      };
      const currentThreadRuntimeState = () => {
        const threadId = $("chat-thread-id").value.trim();
        return threadId ? threadRuntimeStates.get(threadId) : null;
      };
      function updateThreadRuntimeControls() {
        const merge = $("thread-merge");
        if (!merge) return;
        const state = currentThreadRuntimeState();
        const isSleeping = state && sleepingStatuses.has(state.status);
        merge.classList.toggle("ok", Boolean(isSleeping));
        merge.title = isSleeping
          ? "Thread runtime is asleep/suspended. Merge will wake the worker, merge origin/dev, and push."
          : "Merge latest origin/dev into this thread branch and push.";
      }
      const resetRealtimeState = (threadId, taskId) => {
        activeTaskKey = `${threadId}:${taskId}`;
        seenStreamEvents = new Set();
      };
      const shouldRenderEvent = (source, threadId, taskId, seq, kind) => {
        if (activeTaskKey && `${threadId || ""}:${taskId || ""}` !== activeTaskKey) return false;
        const key = seq === undefined || seq === null
          ? `${source}:${taskId || "none"}:no-seq:${kind}`
          : `${taskId || "none"}:${seq}:${kind}`;
        if (seenStreamEvents.has(key)) return false;
        seenStreamEvents.add(key);
        return true;
      };
      const renderStreamEvent = (kind, raw, source = "sse", seq = undefined) => {
        let parsed = raw;
        try { parsed = JSON.parse(raw); } catch (_error) {}
        if (parsed && typeof parsed === "object" && parsed.type === "task-event") {
          const event = parsed.event || {};
          if (event && event.kind === "thread-runtime") {
            setThreadRuntimeState(parsed.threadId, event.status || event.action, event);
          }
          if (!shouldRenderEvent(source, parsed.threadId, parsed.taskId, parsed.seq, event.kind || kind)) return;
          const detail = typeof event === "string" ? event : JSON.stringify(event);
          appendStreamLine(`[${new Date().toLocaleTimeString()}] ${source}:${event.kind || kind}: ${detail}`);
          if (event.kind === "done") load();
          return;
        }
        if (parsed && typeof parsed === "object" && parsed.type === "worker-status" && parsed.status === "waiting-for-task") {
          if (!shouldRenderEvent(source, parsed.threadId, parsed.taskId, undefined, parsed.type)) return;
          appendStreamLine(`[${new Date().toLocaleTimeString()}] ${source}:worker-status: waiting for task`);
          return;
        }
        const activeParts = activeTaskKey ? activeTaskKey.split(":") : ["", ""];
        if (!shouldRenderEvent(source, activeParts[0], activeParts[1], seq, kind)) return;
        const detail = typeof parsed === "string" ? parsed : JSON.stringify(parsed);
        appendStreamLine(`[${new Date().toLocaleTimeString()}] ${source}:${kind}: ${detail}`);
      };
      const workerSocketKey = (threadId, taskId) => `${threadId}:${taskId}`;
      const shouldRetryWorkerSocket = (threadId, key) => {
        if (activeTaskKey !== key) return false;
        const state = threadRuntimeStates.get(threadId);
        return !state || !sleepingStatuses.has(state.status) || state.status === "waking";
      };
      const openWorkerWebSocket = (threadId, taskId, attempt = 0) => {
        const key = workerSocketKey(threadId, taskId);
        const existing = workerSockets.get(key);
        if (existing && [WebSocket.CONNECTING, WebSocket.OPEN].includes(existing.readyState)) return;
        const proto = location.protocol === "https:" ? "wss" : "ws";
        const wsUrl = `${proto}://${location.host}${threadIngressPrefix(threadId)}/ws?threadId=${encodeURIComponent(threadId)}&taskId=${encodeURIComponent(taskId)}`;
        const ws = new WebSocket(wsUrl);
        workerSockets.set(key, ws);
        appendStreamLine(`worker websocket ${wsUrl}`);
        ws.onopen = () => {
          appendStreamLine("worker websocket connected");
          ws.send(JSON.stringify({ type: "subscribe", threadId, taskId }));
        };
        ws.onmessage = (event) => {
          renderStreamEvent("message", event.data, "worker-ws");
        };
        ws.onerror = () => {
          appendStreamLine("worker websocket error");
        };
        ws.onclose = () => {
          if (workerSockets.get(key) === ws) workerSockets.delete(key);
          appendStreamLine("worker websocket disconnected");
          if (attempt < 6 && shouldRetryWorkerSocket(threadId, key)) {
            window.setTimeout(() => openWorkerWebSocket(threadId, taskId, attempt + 1), 1000 * (attempt + 1));
          }
        };
      };
      const openTaskWebSocket = (threadId, taskId) => {
        if (activeWs) activeWs.close();
        resetRealtimeState(threadId, taskId);
        const proto = location.protocol === "https:" ? "wss" : "ws";
        const wsUrl = `${proto}://${location.host}/gleam/ws?threadId=${encodeURIComponent(threadId)}&taskId=${encodeURIComponent(taskId)}`;
        activeWs = new WebSocket(wsUrl);
        $("chat-stream").textContent = "";
        appendStreamLine(`websocket ${wsUrl}`);
        activeWs.onopen = () => {
          appendStreamLine("websocket connected");
          activeWs.send(JSON.stringify({ type: "subscribe", threadId, taskId }));
        };
        activeWs.onmessage = (event) => {
          renderStreamEvent("message", event.data, "ws");
        };
        activeWs.onclose = () => {
          appendStreamLine("websocket disconnected");
        };
      };
      const openTaskStream = (threadId, taskId) => {
        if (activeStream) activeStream.close();
        const streamUrl = `/api/agents/threads/${encodeURIComponent(threadId)}/stream/${encodeURIComponent(taskId)}`;
        activeStream = new EventSource(streamUrl);
        appendStreamLine(`sse ${streamUrl}`);
        for (const kind of ["status", "claude", "stderr", "error", "artifact", "done"]) {
          activeStream.addEventListener(kind, (event) => {
            renderStreamEvent(kind, event.data, "sse", event.lastEventId);
            if (kind === "done" && activeStream) {
              activeStream.close();
              activeStream = null;
              load();
            }
          });
        }
        activeStream.onerror = () => {
          appendStreamLine("stream disconnected");
        };
      };
      const dispatchChat = async () => {
        const threadId = $("chat-thread-id").value.trim();
        const taskId = $("chat-task-id").value.trim();
        const prompt = $("chat-prompt").value.trim();
        if (!threadId || !taskId || !prompt) {
          appendStreamLine("thread UUID, task UUID, and prompt are required");
          return;
        }
        const route = `/api/agents/threads/${encodeURIComponent(threadId)}/tasks`;
        openTaskWebSocket(threadId, taskId);
        appendStreamLine(`POST ${route}`);
        let lastRuntimeSummary = "";
        const runtimePoll = window.setInterval(async () => {
          try {
            const summary = await fetchRuntimeSummary(threadId);
            if (summary !== lastRuntimeSummary) {
              lastRuntimeSummary = summary;
              appendStreamLine(`runtime ${summary}`);
            }
          } catch (error) {
            appendStreamLine(`runtime ${String(error)}`);
          }
        }, 5000);
        fetchRuntimeSummary(threadId).then((summary) => {
          lastRuntimeSummary = summary;
          appendStreamLine(`runtime ${summary}`);
        }).catch((error) => appendStreamLine(`runtime ${String(error)}`));
        let response;
        try {
          response = await fetch(route, {
            method: "POST",
            headers: { "content-type": "application/json" },
            body: JSON.stringify({
              taskId,
              threadId,
              prompt,
              provider: $("chat-provider").value,
              threadTitle: prompt.slice(0, 120)
            })
          });
        } finally {
          window.clearInterval(runtimePoll);
        }
        const textBody = await response.text();
        if (!response.ok) {
          appendStreamLine(`dispatch failed ${response.status}: ${textBody.slice(0, 500)}`);
          return;
        }
        appendStreamLine(`dispatch accepted ${textBody.slice(0, 500)}`);
        fetchRuntimeSummary(threadId).then((summary) => appendStreamLine(`runtime ${summary}`)).catch(() => {});
        openTaskStream(threadId, taskId);
        openWorkerWebSocket(threadId, taskId);
        resetTaskId();
        load();
      };
      const runThreadControl = async (action) => {
        const threadId = $("chat-thread-id").value.trim();
        if (!threadId) {
          appendStreamLine("thread UUID is required");
          return;
        }
        const config = {
          sleep: {
            label: "Pause/Sleep (Reduce resources to container)",
            action: "sleep",
            route: `/api/agents/threads/${encodeURIComponent(threadId)}/sleep`,
            confirm: "Scale this thread runtime to zero replicas?"
          },
          archive: {
            label: "Archive (Deep Sleep - Suspend container?)",
            action: "archive",
            route: `/api/agents/threads/${encodeURIComponent(threadId)}/archive`,
            confirm: "Archive/deep-sleep this thread runtime?"
          },
          delete: {
            label: "Delete (Delete Container)",
            action: "hard-delete",
            route: `/api/agents/threads/${encodeURIComponent(threadId)}/hard-delete`,
            confirm: "Delete the Kubernetes runtime resources for this thread? GitHub PRs are not deleted."
          },
          merge: {
            label: "Merge with upstream",
            action: "merge-upstream",
            route: `/api/agents/threads/${encodeURIComponent(threadId)}/merge-upstream`,
            confirm: "Merge latest origin/dev into this thread branch and push?"
          },
          openPr: {
            label: "Open draft PR",
            action: "open-pr",
            route: `/api/agents/threads/${encodeURIComponent(threadId)}/open-pr`,
            confirm: "Open or reuse a draft WIP pull request for this thread branch?"
          }
        }[action];
        if (!config || !confirm(config.confirm)) return;
        const payload = {
          kind: "thread-control",
          action: config.action,
          threadId,
          taskId: $("chat-task-id").value.trim() || undefined,
          requestedBy: "rust-web-home",
          reason: config.label
        };
        const taskId = payload.taskId || newUuid();
        payload.taskId = taskId;
        $("chat-task-id").value = taskId;
        openTaskWebSocket(threadId, taskId);
        if (config.action === "merge-upstream" || config.action === "open-pr") {
          setThreadRuntimeState(threadId, "waking", { action: config.action, message: `${config.label} requested` });
        }
        appendStreamLine(`POST ${config.route}`);
        appendStreamLine(`signal ${JSON.stringify(payload)}`);
        const response = await fetch(config.route, {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify(payload)
        });
        const textBody = await response.text();
        if (!response.ok) {
          appendStreamLine(`${config.label} failed ${response.status}: ${textBody.slice(0, 500)}`);
          return;
        }
        appendStreamLine(`${config.label} accepted ${textBody.slice(0, 500)}`);
        if (config.action === "sleep") {
          setThreadRuntimeState(threadId, "sleeping", { action: config.action, message: "runtime scaled to zero" });
        } else if (config.action === "archive") {
          setThreadRuntimeState(threadId, "archived", { action: config.action, message: "runtime archived" });
        } else if (config.action === "hard-delete") {
          setThreadRuntimeState(threadId, "deleted", { action: config.action, message: "runtime deleted" });
        }
        load();
      };
      const clearSnapshot = () => {
        setStat("thread-count", 0);
        setStat("task-count", 0);
        setStat("running-count", 0);
        setStat("done-count", 0);
        setStat("failed-count", 0);
        setStat("pr-count", 0);
        renderTasks([]);
        renderThreads([]);
      };
      const publicLoadError = (error) => {
        if (error instanceof Error && /^agent tasks request failed \(\d+\)$/.test(error.message)) {
          return error.message;
        }
        return "agent tasks are temporarily unavailable; check remote web-home server logs";
      };

      function renderTasks(tasks) {
        const body = $("tasks-body");
        body.textContent = "";
        if (!tasks.length) {
          const tr = document.createElement("tr");
          tr.appendChild(cell("No tasks found.", "muted"));
          body.appendChild(tr);
          return;
        }
        for (const task of tasks) {
          const tr = document.createElement("tr");
          const taskBox = document.createElement("div");
          taskBox.appendChild(code(shortId(task.id)));
          const created = document.createElement("div");
          created.className = "muted";
          created.textContent = fmt(task.createdAt);
          taskBox.appendChild(created);
          tr.appendChild(cell(taskBox));

          const threadBox = document.createElement("div");
          threadBox.appendChild(text(task.threadTitle || "Untitled thread"));
          const idLine = document.createElement("div");
          idLine.className = "muted";
          idLine.textContent = task.threadId || "";
          threadBox.appendChild(idLine);
          tr.appendChild(cell(threadBox));

          const prompt = document.createElement("div");
          prompt.className = "prompt";
          prompt.textContent = empty(task.prompt, "");
          tr.appendChild(cell(prompt));

          const status = document.createElement("strong");
          status.className = statusClass(task.status);
          status.textContent = empty(task.status, "unknown");
          tr.appendChild(cell(status));

          const events = document.createElement("div");
          events.appendChild(text(`${task.eventCount || 0} events`));
          const latest = document.createElement("div");
          latest.className = "muted";
          latest.textContent = empty(task.latestEventKind, `seq ${task.lastEventSeq ?? -1}`);
          events.appendChild(latest);
          tr.appendChild(cell(events));

          const refs = document.createElement("div");
          refs.appendChild(code(empty(task.branch)));
          if (task.prUrl) {
            const pr = document.createElement("div");
            pr.appendChild(link(task.prUrl, task.prState ? `PR ${task.prState}` : "PR"));
            refs.appendChild(pr);
          }
          if (task.errorMessage) {
            const error = document.createElement("div");
            error.className = "status-failed";
            error.textContent = task.errorMessage;
            refs.appendChild(error);
          }
          tr.appendChild(cell(refs));
          body.appendChild(tr);
        }
      }

      function renderThreads(threads) {
        const body = $("threads-body");
        body.textContent = "";
        if (!threads.length) {
          const tr = document.createElement("tr");
          tr.appendChild(cell("No threads found.", "muted"));
          body.appendChild(tr);
          return;
        }
        for (const thread of threads) {
          if (thread.archivedAt) {
            setThreadRuntimeState(thread.id, "archived", { action: "archive", message: "thread archived" });
          }
          const tr = document.createElement("tr");
          const title = document.createElement("div");
          title.appendChild(text(thread.title || "Untitled thread"));
          const id = document.createElement("div");
          id.className = "muted";
          id.textContent = thread.id;
          title.appendChild(id);
          tr.appendChild(cell(title));
          tr.appendChild(cell(thread.repo || "none"));
          tr.appendChild(cell(code(thread.baseBranch || "dev")));
          tr.appendChild(cell(String(thread.taskCount || 0)));
          tr.appendChild(cell(String(thread.activeTaskCount || 0)));
          tr.appendChild(cell(fmt(thread.latestTaskAt || thread.updatedAt || thread.createdAt)));
          body.appendChild(tr);
        }
      }

      async function load() {
        const limit = $("limit").value;
        const errors = $("errors");
        try {
          const response = await fetch(`/api/agents/tasks?limit=${encodeURIComponent(limit)}`, { cache: "no-store" });
          if (!response.ok) {
            throw new Error(`agent tasks request failed (${response.status})`);
          }
          const data = await response.json();
          setStat("thread-count", data.summary.threadCount);
          setStat("task-count", data.summary.taskCount);
          setStat("running-count", data.summary.runningCount);
          setStat("done-count", data.summary.doneCount);
          setStat("failed-count", data.summary.failedCount);
          setStat("pr-count", data.summary.prCount);
          $("source").textContent = data.source;
          $("source").className = data.ok ? "pill" : "pill bad";
          $("updated").textContent = `updated ${new Date(Number(data.generatedAtMs)).toLocaleTimeString()}`;
          renderTasks(data.tasks || []);
          renderThreads(data.threads || []);
          if (data.errors && data.errors.length) {
            errors.hidden = false;
            errors.textContent = data.errors.join("\n");
          } else {
            errors.hidden = true;
            errors.textContent = "";
          }
        } catch (error) {
          clearSnapshot();
          errors.hidden = false;
          errors.textContent = publicLoadError(error);
          $("source").textContent = "error";
          $("source").className = "pill bad";
          $("updated").textContent = "waiting for successful snapshot";
        }
      }

      $("new-thread").addEventListener("click", resetThreadId);
      $("new-task").addEventListener("click", resetTaskId);
      $("thread-sleep").addEventListener("click", () => {
        runThreadControl("sleep").catch((error) => appendStreamLine(`sleep error: ${String(error)}`));
      });
      $("thread-archive").addEventListener("click", () => {
        runThreadControl("archive").catch((error) => appendStreamLine(`archive error: ${String(error)}`));
      });
      $("thread-delete").addEventListener("click", () => {
        runThreadControl("delete").catch((error) => appendStreamLine(`delete error: ${String(error)}`));
      });
      $("thread-merge").addEventListener("click", () => {
        runThreadControl("merge").catch((error) => appendStreamLine(`merge error: ${String(error)}`));
      });
      $("thread-open-pr").addEventListener("click", () => {
        runThreadControl("openPr").catch((error) => appendStreamLine(`open PR error: ${String(error)}`));
      });
      $("send-chat").addEventListener("click", () => {
        dispatchChat().catch((error) => appendStreamLine(`dispatch error: ${String(error)}`));
      });
      $("chat-thread-id").addEventListener("input", setChatRoute);
      $("refresh").addEventListener("click", load);
      $("limit").addEventListener("change", load);
      resetThreadId();
      load();
      setInterval(load, 10000);
"#;
const LAMBDA_FUNCTIONS_HTML: &str = r#"<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>dd lambda functions</title>
    <style>
      :root {
        color-scheme: dark;
        --bg: #0d1117;
        --panel: #151b23;
        --panel-2: #101722;
        --field: #0f1620;
        --line: rgba(148, 163, 184, 0.28);
        --text: #eef2f6;
        --muted: #a8b3c1;
        --accent: #5eead4;
        --accent-2: #facc15;
        --danger: #fb7185;
        --ok: #86efac;
      }
      * { box-sizing: border-box; }
      body {
        margin: 0;
        min-height: 100vh;
        background: var(--bg);
        color: var(--text);
        font-family: Inter, ui-sans-serif, system-ui, -apple-system, Segoe UI, sans-serif;
      }
      a { color: var(--accent); text-decoration: none; }
      a:hover { text-decoration: underline; }
      button, input, select, textarea {
        border: 1px solid var(--line);
        border-radius: 7px;
        background: var(--field);
        color: var(--text);
        font: inherit;
      }
      button {
        min-height: 34px;
        padding: 7px 11px;
        cursor: pointer;
      }
      button:hover { border-color: rgba(94, 234, 212, 0.62); }
      button.primary {
        border-color: rgba(94, 234, 212, 0.65);
        background: rgba(20, 83, 45, 0.32);
        color: #dcfce7;
      }
      button.warn { border-color: rgba(250, 204, 21, 0.55); color: #fef9c3; }
      input, select {
        min-height: 34px;
        width: 100%;
        padding: 7px 9px;
      }
      textarea {
        width: 100%;
        min-height: 120px;
        padding: 10px;
        resize: vertical;
        font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
        font-size: 13px;
        line-height: 1.45;
      }
      .app {
        min-height: 100vh;
        display: grid;
        grid-template-columns: minmax(280px, 360px) minmax(0, 1fr);
      }
      .sidebar {
        border-right: 1px solid var(--line);
        background: #111821;
        padding: 18px;
        min-width: 0;
      }
      .main {
        min-width: 0;
        padding: 22px;
      }
      .topbar, .row, .actions {
        display: flex;
        align-items: center;
        gap: 10px;
        flex-wrap: wrap;
      }
      .topbar { justify-content: space-between; margin-bottom: 16px; }
      h1 { margin: 0; font-size: 24px; }
      h2 { margin: 0; font-size: 16px; }
      h3 { margin: 0; font-size: 14px; }
      p { margin: 0; color: var(--muted); line-height: 1.45; }
      .muted { color: var(--muted); }
      .panel {
        border: 1px solid var(--line);
        border-radius: 8px;
        background: var(--panel);
        padding: 14px;
      }
      .grid {
        display: grid;
        grid-template-columns: repeat(2, minmax(0, 1fr));
        gap: 10px;
      }
      .wide { grid-column: 1 / -1; }
      label span {
        display: block;
        color: var(--muted);
        font-size: 12px;
        margin-bottom: 5px;
      }
      .pill {
        display: inline-flex;
        align-items: center;
        border: 1px solid rgba(94, 234, 212, 0.35);
        border-radius: 999px;
        padding: 3px 8px;
        color: var(--accent);
        font-size: 12px;
        white-space: nowrap;
      }
      .pill.warn { border-color: rgba(250, 204, 21, 0.4); color: var(--accent-2); }
      .pill.bad { border-color: rgba(251, 113, 133, 0.42); color: var(--danger); }
      .function-list {
        display: grid;
        gap: 8px;
        margin-top: 14px;
      }
      details {
        border: 1px solid rgba(148, 163, 184, 0.2);
        border-radius: 8px;
        background: var(--panel-2);
        overflow: hidden;
      }
      details[open] { border-color: rgba(94, 234, 212, 0.46); }
      summary {
        min-height: 52px;
        display: grid;
        grid-template-columns: minmax(0, 1fr) auto;
        align-items: center;
        gap: 10px;
        padding: 12px;
        cursor: pointer;
      }
      summary::marker { color: var(--accent); }
      .summary-title {
        display: block;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
        font-weight: 600;
      }
      .summary-meta {
        display: flex;
        gap: 8px;
        color: var(--muted);
        font-size: 12px;
        flex-wrap: wrap;
        margin-top: 5px;
      }
      .details-body {
        border-top: 1px solid rgba(148, 163, 184, 0.18);
        padding: 12px;
        display: grid;
        gap: 10px;
      }
      .output {
        min-height: 170px;
        max-height: 420px;
        overflow: auto;
        white-space: pre-wrap;
        overflow-wrap: anywhere;
        border: 1px solid rgba(148, 163, 184, 0.2);
        border-radius: 8px;
        background: #090f16;
        padding: 12px;
        color: #d7fbf4;
        font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
        font-size: 13px;
        line-height: 1.45;
      }
      @media (max-width: 980px) {
        .app { grid-template-columns: 1fr; }
        .sidebar { border-right: 0; border-bottom: 1px solid var(--line); }
        .grid { grid-template-columns: 1fr; }
      }
    </style>
  </head>
  <body>
    <div class="app">
      <aside class="sidebar">
        <div class="topbar">
          <div>
            <h1>Lambda functions</h1>
            <p id="snapshot-meta">loading functions</p>
          </div>
          <button id="refresh" type="button" title="Refresh">Refresh</button>
        </div>
        <input id="search" autocomplete="off" placeholder="Search functions" />
        <div class="actions" style="margin-top: 10px">
          <button id="new-function" class="primary" type="button">New</button>
        </div>
        <div id="function-list" class="function-list" aria-live="polite"></div>
      </aside>
      <main class="main">
        <div class="topbar">
          <div>
            <h1 id="editor-title">New function</h1>
            <p id="editor-subtitle">draft</p>
          </div>
          <div class="row">
            <a href="/agents/threads">Agent threads</a>
            <a href="/home">Service directory</a>
          </div>
        </div>

        <section class="panel">
          <div class="grid">
            <label>
              <span>Slug</span>
              <input id="slug" autocomplete="off" spellcheck="false" />
            </label>
            <label>
              <span>Name</span>
              <input id="display-name" autocomplete="off" />
            </label>
            <label>
              <span>Status</span>
              <select id="status">
                <option value="draft">draft</option>
                <option value="active">active</option>
                <option value="paused">paused</option>
                <option value="archived">archived</option>
              </select>
            </label>
            <label>
              <span>Runtime</span>
              <select id="runtime">
                <option value="javascript">javascript</option>
                <option value="typescript">typescript</option>
                <option value="python">python</option>
                <option value="shell">shell</option>
                <option value="gleam">gleam</option>
              </select>
            </label>
            <label>
              <span>Reuse key</span>
              <input id="reuse-key" autocomplete="off" spellcheck="false" />
            </label>
            <label>
              <span>Idle timeout seconds</span>
              <input id="idle-timeout" type="number" min="1" max="3600" />
            </label>
            <label>
              <span>Max run ms</span>
              <input id="max-run" type="number" min="1000" max="300000" step="500" />
            </label>
            <label>
              <span>Entry command</span>
              <input id="entry-command" autocomplete="off" readonly spellcheck="false" />
            </label>
            <label class="wide">
              <span>Description</span>
              <textarea id="description" style="min-height: 74px; font-family: inherit"></textarea>
            </label>
            <label class="wide">
              <span>Function body</span>
              <textarea id="function-body" spellcheck="false"></textarea>
            </label>
            <label>
              <span>Labels JSON</span>
              <textarea id="labels-json" spellcheck="false"></textarea>
            </label>
            <label>
              <span>Meta JSON</span>
              <textarea id="meta-json" spellcheck="false"></textarea>
            </label>
          </div>
          <div class="actions" style="margin-top: 10px">
            <button id="save" class="primary" type="button">Save</button>
            <button id="reset" type="button">Reset</button>
            <span id="save-state" class="pill warn">idle</span>
          </div>
        </section>

        <section class="panel" style="margin-top: 14px">
          <div class="topbar">
            <h2>Run</h2>
            <span id="run-state" class="pill warn">idle</span>
          </div>
          <label>
            <span>Request JSON</span>
            <textarea id="request-json" spellcheck="false"></textarea>
          </label>
          <div class="actions" style="margin-top: 10px">
            <button id="run" class="primary" type="button">Run</button>
            <code id="invoke-route">/lambdas/invoke/:function-id</code>
          </div>
          <pre id="output" class="output"></pre>
        </section>
      </main>
    </div>

    <script>
      const $ = (id) => document.getElementById(id);
      const defaultCommand = "env -i PATH=\"$PATH\" NODE_ENV=production node --permission --allow-net child-runtimes/js-function-runner.mjs";
      const state = {
        functions: [],
        selectedId: null,
      };

      function normalizeSlug(value) {
        return String(value || "")
          .trim()
          .toLowerCase()
          .replace(/[^a-z0-9]+/g, "-")
          .replace(/^-+|-+$/g, "")
          .slice(0, 120);
      }

      function fmt(value) {
        if (!value) return "never";
        const date = new Date(value);
        return Number.isNaN(date.getTime()) ? String(value) : date.toLocaleString();
      }

      function parseJsonField(id, fallback) {
        const value = $(id).value.trim();
        if (!value) return fallback;
        return JSON.parse(value);
      }

      function selectedFunction() {
        return state.functions.find((fn) => fn.id === state.selectedId) || null;
      }

      function functionPayload() {
        return {
          slug: normalizeSlug($("slug").value),
          displayName: $("display-name").value.trim(),
          description: $("description").value.trim(),
          runtime: $("runtime").value,
          entryCommand: defaultCommand,
          functionBody: $("function-body").value,
          reuseKey: $("reuse-key").value.trim() || null,
          idleTimeoutSeconds: Number($("idle-timeout").value || 300),
          maxRunMs: Number($("max-run").value || 30000),
          status: $("status").value,
          labels: parseJsonField("labels-json", []),
          metaData: parseJsonField("meta-json", {}),
        };
      }

      function setSaveState(message, kind = "warn") {
        const node = $("save-state");
        node.textContent = message;
        node.className = kind === "bad" ? "pill bad" : kind === "ok" ? "pill" : "pill warn";
      }

      function setRunState(message, kind = "warn") {
        const node = $("run-state");
        node.textContent = message;
        node.className = kind === "bad" ? "pill bad" : kind === "ok" ? "pill" : "pill warn";
      }

      function fillEditor(fn) {
        state.selectedId = fn?.id || null;
        $("editor-title").textContent = fn?.displayName || "New function";
        $("editor-subtitle").textContent = fn?.slug || "draft";
        $("slug").value = fn?.slug || "";
        $("display-name").value = fn?.displayName || "";
        $("status").value = fn?.status || "draft";
        $("runtime").value = fn?.runtime || "javascript";
        $("reuse-key").value = fn?.reuseKey || "";
        $("idle-timeout").value = fn?.idleTimeoutSeconds || 300;
        $("max-run").value = fn?.maxRunMs || 30000;
        $("entry-command").value = fn?.entryCommand || defaultCommand;
        $("description").value = fn?.description || "";
        $("function-body").value = fn?.functionBody || "return { status: 200, body: { ok: true, echo: request.body ?? null } };";
        $("labels-json").value = JSON.stringify(fn?.labels ?? [], null, 2);
        $("meta-json").value = JSON.stringify(fn?.metaData ?? {}, null, 2);
        $("request-json").value = JSON.stringify({ body: { ping: "pong" } }, null, 2);
        $("invoke-route").textContent = `/lambdas/invoke/${fn?.id || ":function-id"}`;
        $("output").textContent = "";
        setSaveState("idle");
        setRunState("idle");
        renderFunctions();
      }

      function renderFunctions() {
        const list = $("function-list");
        list.textContent = "";
        const search = $("search").value.trim().toLowerCase();
        const functions = state.functions.filter((fn) => {
          const haystack = `${fn.id} ${fn.slug} ${fn.displayName} ${fn.description}`.toLowerCase();
          return !search || haystack.includes(search);
        });
        $("snapshot-meta").textContent = `${functions.length} of ${state.functions.length} functions`;
        if (!functions.length) {
          const empty = document.createElement("p");
          empty.className = "muted";
          empty.textContent = "No functions found.";
          list.appendChild(empty);
          return;
        }
        for (const fn of functions) {
          const details = document.createElement("details");
          details.open = fn.id === state.selectedId;
          const summary = document.createElement("summary");
          const left = document.createElement("span");
          const title = document.createElement("span");
          title.className = "summary-title";
          title.textContent = fn.displayName || fn.slug;
          const meta = document.createElement("span");
          meta.className = "summary-meta";
          meta.textContent = `${fn.slug} - ${fn.id.slice(0, 8)} - ${fn.runtime} - updated ${fmt(fn.updatedAt)}`;
          left.append(title, meta);
          const status = document.createElement("span");
          status.className = fn.status === "active" ? "pill" : fn.status === "paused" ? "pill warn" : "pill bad";
          status.textContent = fn.status;
          summary.append(left, status);
          summary.addEventListener("click", () => fillEditor(fn));
          const body = document.createElement("div");
          body.className = "details-body";
          const description = document.createElement("p");
          description.textContent = fn.description || "No description";
          const actions = document.createElement("div");
          actions.className = "actions";
          const edit = document.createElement("button");
          edit.type = "button";
          edit.textContent = "Edit";
          edit.addEventListener("click", () => fillEditor(fn));
          const run = document.createElement("button");
          run.type = "button";
          run.className = "primary";
          run.textContent = "Run";
          run.addEventListener("click", () => {
            fillEditor(fn);
            invokeSelected().catch((error) => {
              setRunState("failed", "bad");
              $("output").textContent = String(error);
            });
          });
          actions.append(edit, run);
          body.append(description, actions);
          details.append(summary, body);
          list.appendChild(details);
        }
      }

      async function load() {
        const response = await fetch("/api/lambdas/functions?limit=250", { cache: "no-store" });
        const data = await response.json();
        state.functions = Array.isArray(data.functions) ? data.functions : [];
        if (state.selectedId) {
          const stillSelected = selectedFunction();
          if (stillSelected) fillEditor(stillSelected);
        } else if (state.functions.length) {
          fillEditor(state.functions[0]);
        } else {
          fillEditor(null);
        }
        renderFunctions();
      }

      async function save() {
        setSaveState("saving");
        const payload = functionPayload();
        if (!payload.slug || !payload.displayName || !payload.functionBody.trim()) {
          setSaveState("missing fields", "bad");
          return;
        }
        const current = selectedFunction();
        const route = current ? `/api/lambdas/functions/${encodeURIComponent(current.id)}` : "/api/lambdas/functions";
        const response = await fetch(route, {
          method: current ? "PATCH" : "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify(payload),
        });
        const data = await response.json().catch(() => ({}));
        if (!response.ok || !data.ok) {
          setSaveState("failed", "bad");
          $("output").textContent = JSON.stringify(data, null, 2);
          return;
        }
        setSaveState("saved", "ok");
        await load();
        const saved = state.functions.find((fn) => fn.id === data.function?.id);
        if (saved) fillEditor(saved);
      }

      async function invokeSelected() {
        const current = selectedFunction();
        const functionId = current?.id;
        if (!functionId) {
          setRunState("save first", "bad");
          return;
        }
        const request = parseJsonField("request-json", {});
        $("invoke-route").textContent = `/lambdas/invoke/${functionId}`;
        setRunState("running");
        const response = await fetch(`/lambdas/invoke/${encodeURIComponent(functionId)}`, {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify(request),
        });
        const text = await response.text();
        setRunState(response.ok ? "complete" : "failed", response.ok ? "ok" : "bad");
        try {
          $("output").textContent = JSON.stringify(JSON.parse(text), null, 2);
        } catch {
          $("output").textContent = text;
        }
      }

      $("refresh").addEventListener("click", () => load().catch((error) => setSaveState(String(error), "bad")));
      $("new-function").addEventListener("click", () => fillEditor(null));
      $("search").addEventListener("input", renderFunctions);
      $("slug").addEventListener("input", () => {
        $("slug").value = normalizeSlug($("slug").value);
        $("invoke-route").textContent = `/lambdas/invoke/${selectedFunction()?.id || ":function-id"}`;
      });
      $("reset").addEventListener("click", () => fillEditor(selectedFunction()));
      $("save").addEventListener("click", () => save().catch((error) => {
        setSaveState("failed", "bad");
        $("output").textContent = String(error);
      }));
      $("run").addEventListener("click", () => invokeSelected().catch((error) => {
        setRunState("failed", "bad");
        $("output").textContent = String(error);
      }));

      load().catch((error) => {
        setSaveState("load failed", "bad");
        $("snapshot-meta").textContent = String(error);
      });
      setInterval(load, 15000);
    </script>
  </body>
</html>
"#;

async fn favicon() -> impl IntoResponse {
    record_request("GET", "/favicon.ico", StatusCode::NO_CONTENT);
    StatusCode::NO_CONTENT
}

async fn healthz() -> impl IntoResponse {
    record_request("GET", "/healthz", StatusCode::OK);
    Json(HealthResponse {
        ok: true,
        service: "dd-remote-web-home".to_string(),
        mode: "public-web".to_string(),
    })
}

async fn metrics() -> impl IntoResponse {
    record_request("GET", "/metrics", StatusCode::OK);
    UPTIME_SECONDS.set(STARTED_AT.elapsed().as_secs() as i64);

    let encoder = TextEncoder::new();
    let metric_families = prometheus::gather();
    let mut buffer = Vec::new();
    encoder
        .encode(&metric_families, &mut buffer)
        .expect("failed to encode prometheus metrics");

    (
        [(header::CONTENT_TYPE, encoder.format_type().to_string())],
        buffer,
    )
}

#[tokio::main]
async fn main() {
    let host = env::var("HOST").unwrap_or_else(|_| "0.0.0.0".to_string());
    let port = env::var("PORT")
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(8080);

    let state = AppState {
        server_label:
            "Rust home server (/ + /home + /agents/tasks + /agents/threads + /lambdas/functions)"
                .to_string(),
        control_plane_label: "Kubernetes Ingress selects the UUID-bound worker Service".to_string(),
        workers_label: "Node.js containers pinned to one chat/thread".to_string(),
        queue_consumer_label: "Rust NATS shadow preparer (dd-remote-queue-consumer)".to_string(),
    };

    let app = Router::new()
        .route("/", get(root))
        .route("/home", get(home))
        .route("/home/", get(root))
        .route("/agents/tasks", get(agents_tasks_page))
        .route("/agents/tasks/", get(agents_tasks_page))
        .route("/agents/threads", get(agents_threads_page))
        .route("/agents/threads/", get(agents_threads_page))
        .route("/assets/web-home/agents-tasks.css", get(agents_tasks_css))
        .route("/assets/web-home/agents-tasks.js", get(agents_tasks_js))
        .route(
            "/assets/web-home/agents-tasks.html",
            get(agents_tasks_html_fragment),
        )
        .route(
            "/assets/web-home/agents-threads.css",
            get(agents_threads_css),
        )
        .route("/assets/web-home/agents-threads.js", get(agents_threads_js))
        .route(
            "/assets/web-home/agents-threads.html",
            get(agents_threads_html_fragment),
        )
        .route("/lambdas/functions", get(lambda_functions_page))
        .route("/lambdas/functions/", get(lambda_functions_page))
        .route("/healthz", get(healthz))
        .route("/metrics", get(metrics))
        .route("/favicon.ico", get(favicon))
        .with_state(state);

    let address: SocketAddr = format!("{host}:{port}")
        .parse()
        .expect("failed to parse bind address");
    println!("dd-remote-web-home listening on http://{address}");

    let listener = tokio::net::TcpListener::bind(address)
        .await
        .expect("failed to bind tcp listener");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .expect("axum server crashed");
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        use tokio::signal::unix::{signal, SignalKind};
        if let Ok(mut sigterm) = signal(SignalKind::terminate()) {
            let _ = sigterm.recv().await;
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}
