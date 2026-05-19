use std::{env, net::SocketAddr, time::Instant};

use axum::{
    extract::State,
    http::StatusCode,
    http::{header, HeaderValue},
    response::{Html, IntoResponse, Response},
    routing::get,
    Json, Router,
};
use maud::{html, Markup, PreEscaped, DOCTYPE};
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
    home_document(&state)
}

#[derive(Clone, Copy)]
struct AccessPill {
    label: &'static str,
    warn: bool,
}

#[derive(Clone, Copy)]
struct DeploymentRow {
    deployments: &'static [&'static str],
    service: &'static [&'static str],
    service_note: Option<&'static str>,
    access: AccessPill,
    notes: &'static str,
}

#[derive(Clone, Copy)]
struct PathEntry {
    label: &'static str,
    href: Option<&'static str>,
}

#[derive(Clone, Copy)]
struct PathRow {
    paths: &'static [PathEntry],
    target: &'static str,
    access: AccessPill,
    notes: &'static str,
}

const PUBLIC: AccessPill = AccessPill {
    label: "public",
    warn: false,
};
const SERVER_AUTH: AccessPill = AccessPill {
    label: "server auth",
    warn: true,
};
const INTERNAL: AccessPill = AccessPill {
    label: "internal",
    warn: true,
};
const INTERNAL_ACCESS: AccessPill = AccessPill {
    label: "internal access",
    warn: true,
};
const CLUSTER_LOCAL: AccessPill = AccessPill {
    label: "cluster local",
    warn: true,
};
const VPN_PRIVATE: AccessPill = AccessPill {
    label: "vpn/private",
    warn: true,
};

fn home_document(state: &AppState) -> Html<String> {
    Html(
        html! {
            (DOCTYPE)
            html lang="en" {
                head {
                    meta charset="utf-8";
                    meta name="viewport" content="width=device-width, initial-scale=1";
                    title { "dd-remote-web" }
                    style { (PreEscaped(HOME_CSS)) }
                }
                body {
                    main class="shell" {
                        h1 { "dd remote service directory" }
                        (home_summary())
                        (status_grid(state))
                        (deployments_section())
                        (live_containers_section())
                        (paths_section())
                        (security_plan_section())
                    }
                    script { (PreEscaped(HOME_LIVE_CONTAINERS_JS)) }
                }
            }
        }
        .into_string(),
    )
}

fn home_summary() -> Markup {
    html! {
        p {
            "Public entrypoint for the EC2 Kubernetes runtime. Open paths: "
            code { "/" } ", " code { "/home" } ", " code { "/auth" } ", "
            code { "/agents/tasks" } ", " code { "/agents/threads" } ", "
            code { "/api/agents/tasks" } ", " code { "/presence-test" } ", "
            code { "/wss-test" } ", " code { "/webrtc/" } ", " code { "/fsws/" } ", "
            code { "/mdp/" } ", and " code { "/des/" } ". Server-auth paths: "
            code { "/lambdas/functions" } ", " code { "/lambdas/invoke/<function-id>" } ", "
            code { "/api/lambdas/" } ", " code { "/api/agent-worker/" } ", "
            code { "/container-pools" } ", " code { "/bastion/" } ", " code { "/scrape" } ", "
            code { "/trading/" } ", " code { "/contracts/" } ", " code { "/ml/" } ", "
            code { "/builds" } ", " code { "/gleam/" } ", " code { "/mcp" } ", and "
            code { "/gcs/" } ". Internal-access ops: " code { "/headlamp/" } ", "
            code { "/telemetry/" } ", "
            code { "/prometheus/" } ", " code { "/nats/" } ", " code { "/nats-metrics/" } ", "
            code { "/reaper/" } ", " code { "/cron/" } ", plus the new "
            code { "dd-billing-server" } " ledger API and the " code { "dd-wal-gateway" }
            " Postgres -> NATS CDC pump."
        }
    }
}

fn status_grid(state: &AppState) -> Markup {
    html! {
        div class="grid" {
            (status_panel("Web Deployment", &state.server_label))
            (status_panel("K8s Routing", &state.control_plane_label))
            (status_panel("Workers", &state.workers_label))
            (status_panel("Queue Consumer", &state.queue_consumer_label))
        }
    }
}

fn status_panel(label: &str, value: &str) -> Markup {
    html! {
        section class="panel" {
            span class="label" { (label) }
            div class="value" { (value) }
        }
    }
}

fn deployments_section() -> Markup {
    html! {
        section class="band" {
            h2 { "Deployments" }
            table {
                thead {
                    tr {
                        th style="width: 25%" { "Deployment" }
                        th style="width: 22%" { "Service" }
                        th style="width: 16%" { "Access" }
                        th { "Notes" }
                    }
                }
                tbody {
                    @for row in DEPLOYMENT_ROWS {
                        (deployment_row(row))
                    }
                }
            }
        }
    }
}

fn deployment_row(row: &DeploymentRow) -> Markup {
    html! {
        tr {
            td { (code_list(row.deployments)) }
            td {
                (code_list(row.service))
                @if let Some(note) = row.service_note {
                    " " (note)
                }
            }
            td { (access_badge(row.access)) }
            td { (row.notes) }
        }
    }
}

fn live_containers_section() -> Markup {
    html! {
        section class="band" {
            div class="live-toolbar" {
                div {
                    h2 { "Live containers" }
                    p id="live-containers-status" { "loading managed deployment pods" }
                }
                button id="live-containers-refresh" type="button" { "Refresh" }
            }
            table {
                thead {
                    tr {
                        th style="width: 20%" { "Deployment" }
                        th style="width: 14%" { "Namespace" }
                        th style="width: 23%" { "Pod" }
                        th { "Containers" }
                        th style="width: 13%" { "Terminal" }
                    }
                }
                tbody id="live-containers-body" {
                    tr {
                        td colspan="5" class="muted" {
                            "Loading live container inventory from " code { "/bastion/runtime/deployments" } "."
                        }
                    }
                }
            }
            div id="home-terminal" class="terminal-dock" hidden="hidden" {
                div class="terminal-head" {
                    div {
                        h2 { "Container terminal" }
                        p id="home-terminal-caption" { "bastion exec session" }
                    }
                    button id="home-terminal-close" type="button" { "Close" }
                }
                iframe id="home-terminal-frame" class="terminal-frame" title="Bastion container terminal" {}
            }
        }
    }
}

fn paths_section() -> Markup {
    html! {
        section class="band" {
            h2 { "Paths" }
            table {
                thead {
                    tr {
                        th style="width: 27%" { "Path" }
                        th style="width: 25%" { "Target" }
                        th style="width: 16%" { "Access" }
                        th { "Notes" }
                    }
                }
                tbody {
                    @for row in PATH_ROWS {
                        (path_row(row))
                    }
                }
            }
        }
    }
}

fn path_row(row: &PathRow) -> Markup {
    html! {
        tr {
            td {
                span class="path-links" {
                    @for path in row.paths {
                        @if let Some(href) = path.href {
                            a href=(href) { code { (path.label) } }
                        } @else {
                            code { (path.label) }
                        }
                    }
                }
            }
            td { (row.target) }
            td { (access_badge(row.access)) }
            td { (row.notes) }
        }
    }
}

fn security_plan_section() -> Markup {
    html! {
        section class="band" {
            h2 { "Security plan" }
            ol {
                li { "Today: the public gateway keeps ops paths behind temporary internal access while bootstrap work is still in flight." }
                li {
                    "Next: put TLS and identity-aware auth in front of the gateway using "
                    code { "auth_request" } ", oauth2-proxy, Cloudflare Access, or Tailscale."
                }
                li { "Keep worker, NATS client, and Kubernetes control services internal; only expose explicit web surfaces." }
                li { "Add Kubernetes NetworkPolicies and least-privilege service accounts so services can only talk to the namespaces they need." }
                li { "Replace the static header with signed JWT/HMAC service tokens for backend calls and SSO sessions for browser use." }
            }
        }
    }
}

fn access_badge(access: AccessPill) -> Markup {
    html! {
        span class=(if access.warn { "pill warn" } else { "pill" }) { (access.label) }
    }
}

fn code_list(values: &[&str]) -> Markup {
    html! {
        @for (index, value) in values.iter().enumerate() {
            @if index > 0 {
                " · "
            }
            code { (value) }
        }
    }
}

static DEPLOYMENT_ROWS: &[DeploymentRow] = &[
    DeploymentRow { deployments: &["dd-web-scraper"], service: &["dd-web-scraper:8097"], service_note: None, access: SERVER_AUTH, notes: "Long-running Fastify scraper deployment with scraper parser workers, browser strategies, DOM strategies, native fetch, Cheerio, and Browserless support." },
    DeploymentRow { deployments: &["dd-build-server"], service: &["dd-build-server:8100"], service_note: None, access: SERVER_AUTH, notes: "Rust CI/CD server that clones allowlisted repos, builds allowlisted ECR images, pushes through ECR login, and applies constrained manifests with kubectl." },
    DeploymentRow { deployments: &["dd-ai-ml-pipeline"], service: &["dd-ai-ml-pipeline.ai-ml:8099"], service_note: None, access: SERVER_AUTH, notes: "Python3 online feature pipeline for telemetry risk scoring, anomaly detection, transition hints, and MDP-ready events on dd.remote.telemetry.mdp." },
    DeploymentRow { deployments: &["dd-des-simulator"], service: &["dd-des-simulator:8099"], service_note: None, access: PUBLIC, notes: "Rust DES simulator with declared des.v1 schema, validation endpoint, async job status, and NATS result publishing." },
    DeploymentRow { deployments: &["dd-contract-service"], service: &["dd-contract-service:8101"], service_note: None, access: SERVER_AUTH, notes: "Rust Solana contract gateway for solana.contract.v1 validation, signed transaction simulation, metrics, and NATS validation results." },
    DeploymentRow { deployments: &["dd-vpn"], service: &["dd-vpn-ui.vpn:51821"], service_note: None, access: VPN_PRIVATE, notes: "WireGuard wg-easy VPN server and private admin UI for split-tunnel access to the cluster service and pod CIDRs." },
    DeploymentRow { deployments: &["dd-live-mutex"], service: &["dd-live-mutex:6970"], service_note: None, access: CLUSTER_LOCAL, notes: "Singleton live-mutex broker deployment for TCP lock coordination." },
    DeploymentRow { deployments: &["dd-bastion"], service: &["dd-bastion.vpn:8111"], service_note: None, access: SERVER_AUTH, notes: "Rust bastion/jumphost access broker for VPN profile, kubeconfig export, managed deployment inventory, and browser exec terminals." },
    DeploymentRow { deployments: &["dd-redis-cache"], service: &["dd-redis-cache:6379"], service_note: None, access: CLUSTER_LOCAL, notes: "Ephemeral Redis cache deployment with bounded memory and Redis health probes." },
    DeploymentRow { deployments: &["dd-lock-loadtest-trigger"], service: &["dd-lock-loadtest-trigger:8110"], service_note: None, access: INTERNAL, notes: "Node.js HTTP trigger for live-mutex versus Redis aggregate lock load tests." },
    DeploymentRow { deployments: &["dd-trading-server"], service: &["dd-trading-server:8103"], service_note: None, access: SERVER_AUTH, notes: "Rust trading decision service for trading.decision.v1 scoring, scraper and AI/ML signals, MDP/POMDP policy hints, risk gates, and NATS order intents." },
    DeploymentRow { deployments: &["dd-container-pool"], service: &["dd-container-pool:8102"], service_note: None, access: SERVER_AUTH, notes: "Rust warm container pool service that loads runtime pool config from Postgres and starts local containerd workers through nerdctl." },
    DeploymentRow { deployments: &["headlamp"], service: &["headlamp.headlamp:80"], service_note: Some("(pod 4466)"), access: SERVER_AUTH, notes: "Kubernetes web UI served at /headlamp/. Use the headlamp-viewer service-account token for read-only pod, container, log, workload, Argo CD, KEDA, and External Secrets inspection." },
    DeploymentRow { deployments: &["dd-gleam-lambda-runner"], service: &["dd-gleam-lambda-runner:8083"], service_note: None, access: SERVER_AUTH, notes: "Gleam child-process runner deployment for POST /lambdas/invoke/<function-id>. It uses its own Argo CD app and dd-gleam-lambda-runner-secrets." },
    DeploymentRow { deployments: &["dd-remote-gateway"], service: &["dd-remote-gateway:80/443"], service_note: None, access: PUBLIC, notes: "nginx Ingress for the EC2 single-node cluster. Owns hostPort 80/443 and proxies every documented public/auth path into its in-cluster service." },
    DeploymentRow { deployments: &["dd-remote-web-home"], service: &["dd-remote-web-home:8080"], service_note: None, access: PUBLIC, notes: "This Rust service. Renders /, /home, /agents/tasks, /agents/threads, /lambdas/functions, /presence-test, and /wss-test; also exposes /healthz and /metrics." },
    DeploymentRow { deployments: &["dd-remote-auth"], service: &["dd-remote-auth:8083"], service_note: None, access: PUBLIC, notes: "Rust PIN auth service. Issues the short-lived dd_auth cookie that the gateway accepts in place of the legacy Auth header for browser sessions." },
    DeploymentRow { deployments: &["dd-remote-rest-api"], service: &["dd-remote-rest-api:8082"], service_note: None, access: PUBLIC, notes: "Rust REST API boundary for RDS/Postgres-backed agent task data. Serves /api/agents/* and /api/lambdas/* JSON." },
    DeploymentRow { deployments: &["dd-agent-worker-broker"], service: &["dd-agent-worker-broker:8098"], service_note: None, access: SERVER_AUTH, notes: "Rust NATS-first worker dispatch broker behind /api/agent-worker/. Handles wakeup and direct-if-awake handoff to the UUID-pinned worker." },
    DeploymentRow { deployments: &["dd-dev-server-api"], service: &["dd-dev-server-api:8080"], service_note: None, access: SERVER_AUTH, notes: "Bootstrap Node.js coding-agent task manager. Backs /tasks, /status, /agents, /healthz, and /stream/<taskId> until per-thread Ingress is the only path." },
    DeploymentRow { deployments: &["dd-remote-queue-consumer"], service: &["dd-remote-queue-consumer"], service_note: None, access: INTERNAL, notes: "Rust NATS shadow consumer. Reads dd.remote.thread.*.tasks, pins thread affinity, and prepares the matching UUID-bound worker; it does not execute prompts." },
    DeploymentRow { deployments: &["dd-idle-reaper"], service: &["dd-idle-reaper"], service_note: Some("(no http)"), access: INTERNAL, notes: "Rust maintenance supervisor: idle sweep, 90-minute cluster doctor loop, NATS watchdog, and the 04:00 ET worker-image rebuild for dd-dev-server:dev." },
    DeploymentRow { deployments: &["dd-billing-server"], service: &["dd-billing-server:80"], service_note: Some("(pod 8087)"), access: CLUSTER_LOCAL, notes: "Rust multi-tenant AR/AP ledger. Serves /v1/tenants/* billing/payable state, ledger primitives, provider connections, OAuth, webhooks, locks, scheduled jobs, and notifications. Not yet exposed through the public gateway." },
    DeploymentRow { deployments: &["dd-wal-gateway"], service: &["dd-wal-gateway:8104"], service_note: None, access: INTERNAL, notes: "Rust Postgres -> NATS JetStream CDC gateway. Owns one logical replication slot, publishes cdc.<schema>.<table>.<op> envelopes on stream CDC, and exposes /healthz, /readyz, /metrics." },
    DeploymentRow { deployments: &["dd-gleamlang-server"], service: &["dd-gleamlang-server:8081"], service_note: None, access: SERVER_AUTH, notes: "Gleam/OTP WebSocket fan-out behind /gleam/*. Exposes /gleam/home, /gleam/healthz, /gleam/metrics, and wss://<host>/gleam/ws." },
    DeploymentRow { deployments: &["presence"], service: &["presence-svc.presence:8080"], service_note: Some("(StatefulSet)"), access: CLUSTER_LOCAL, notes: "Gleam gleamlang-presence-server. Distributed-Erlang StatefulSet that powers user-scoped and conv-scoped websockets driving the /presence-test browser harness." },
    DeploymentRow { deployments: &["dd-gleam-mcp-server"], service: &["dd-gleam-mcp-server:8090"], service_note: None, access: SERVER_AUTH, notes: "Gleam JSON-RPC MCP service behind /mcp and /mcp/*. Ships read-only runtime tools, Prometheus metrics, and Loki-collected stdout." },
    DeploymentRow { deployments: &["dd-webrtc-signaling"], service: &["dd-webrtc-signaling:8095"], service_note: None, access: PUBLIC, notes: "Rust WebRTC signaling service behind /webrtc/. Room WebSocket signaling for browser/mobile peer handshakes; media and data channels stay peer-to-peer." },
    DeploymentRow { deployments: &["dd-mdp-optimizer"], service: &["dd-mdp-optimizer:8096"], service_note: None, access: PUBLIC, notes: "Rust MDP/POMDP/RL optimizer behind /mdp/. Consumes dd.remote.mdp.optimize and dd.remote.telemetry.mdp." },
    DeploymentRow { deployments: &["dd-akka-ws-server"], service: &["dd-akka-ws-server:8086"], service_note: None, access: INTERNAL, notes: "Scala/Akka WebSocket reference server backing the akka-streams and async-java load-test targets." },
    DeploymentRow { deployments: &["dd-fsharp-ws-server"], service: &["dd-fsharp-ws-server:8087"], service_note: None, access: PUBLIC, notes: "F# + ASP.NET Core WebSocket server behind /fsws/. Exposes /fsws/healthz, /fsws/livez, /fsws/ws/rx, and /fsws/ws/async." },
    DeploymentRow { deployments: &["dd-formal-methods-server"], service: &["dd-formal-methods-server:8110"], service_note: None, access: INTERNAL, notes: "Rust formal-methods server. Runs annotation-driven proofs and exposes verification status." },
    DeploymentRow { deployments: &["dd-formal-methods-service"], service: &["dd-formal-methods-service:8111"], service_note: None, access: INTERNAL, notes: "Rust formal-methods orchestration service. Templates and dispatches verification jobs against dd-formal-methods-server." },
    DeploymentRow { deployments: &["dd-spark-pipeline-server"], service: &["dd-spark-pipeline-server:8085"], service_note: None, access: INTERNAL, notes: "Java/Spark pipeline server. Coordinates analytical batch/stream jobs against the cluster Spark workers." },
    DeploymentRow { deployments: &["dd-ws-loadtest-rs", "dd-ws-loadtest-rs-akkaws-akkastreams", "dd-ws-loadtest-rs-akkaws-asyncjava"], service: &["dd-ws-loadtest-rs"], service_note: None, access: INTERNAL, notes: "Rust WebSocket load generator (5k clients) plus akka-streams and async-java variants." },
    DeploymentRow { deployments: &["dd-gleamlang-ws-loadtest", "dd-gleamlang-ws-loadtest-akkaws-akkastreams", "dd-gleamlang-ws-loadtest-akkaws-asyncjava"], service: &["dd-gleamlang-ws-loadtest"], service_note: None, access: INTERNAL, notes: "Gleam WebSocket load generator (5k clients) that mirrors dd-ws-loadtest-rs against the Gleam fan-out path." },
    DeploymentRow { deployments: &["dd-nats"], service: &["dd-nats.messaging:4222", "dd-nats.messaging:8222", "dd-nats.messaging:7777"], service_note: None, access: INTERNAL, notes: "NATS + JetStream broker for the cluster. JetStream storage is on the EC2 host under /var/lib/dd/nats." },
    DeploymentRow { deployments: &["dd-grafana", "dd-prometheus", "dd-loki", "dd-tempo", "dd-jaeger"], service: &["*.observability"], service_note: None, access: SERVER_AUTH, notes: "Observability stack served at /telemetry/ (Grafana), /prometheus/, and the Tempo/Jaeger trace backends. Loki collects container logs through dd-promtail." },
    DeploymentRow { deployments: &["dd-otel-collector", "dd-promtail"], service: &["dd-otel-collector.observability:4317/4318/8889", "dd-promtail"], service_note: Some("(DaemonSet)"), access: INTERNAL, notes: "OpenTelemetry Collector ingests OTLP traces and scrapes Prometheus metrics from every Rust/Gleam/Node runtime. Promtail tails /var/log/containers into Loki." },
];

static PATH_ROWS: &[PathRow] = &[
    PathRow { paths: &[PathEntry { label: "/", href: Some("/") }, PathEntry { label: "/home", href: Some("/home") }, PathEntry { label: "/agents/tasks", href: Some("/agents/tasks") }, PathEntry { label: "/agents/threads", href: Some("/agents/threads") }], target: "Rust web homepage deployment", access: PUBLIC, notes: "Service directory plus cluster-served task/thread/PR UI. Browser UIs call JSON APIs for stored state while runtime invocation paths stay separate." },
    PathRow { paths: &[PathEntry { label: "/tasks", href: Some("/tasks") }, PathEntry { label: "/status", href: Some("/status") }, PathEntry { label: "/stream/<uuid>", href: Some("/stream/example-task-id") }], target: "Node.js Coding Agent Task Manager", access: SERVER_AUTH, notes: "Runs inside the already-selected worker container. It executes prompts, tracks taskIds, streams events, and rejects requests for the wrong pinned thread." },
    PathRow { paths: &[PathEntry { label: "/api/agents/tasks", href: Some("/api/agents/tasks") }, PathEntry { label: "/api/agents/threads/<uuid>/context", href: Some("/api/agents/threads/example-thread-id/context") }], target: "Rust REST API (JSON only)", access: PUBLIC, notes: "JSON-only boundary for task snapshots and thread context. The browser UI lives at /agents/tasks." },
    PathRow { paths: &[PathEntry { label: "/lambdas/functions", href: Some("/lambdas/functions") }, PathEntry { label: "/api/lambdas/functions", href: Some("/api/lambdas/functions") }, PathEntry { label: "POST /lambdas/invoke/<function-id>", href: Some("/lambdas/invoke/00000000-0000-0000-0000-000000000000") }], target: "dd-gleam-lambda-runner deployment + Rust REST API", access: SERVER_AUTH, notes: "CRUD/read models stay in the REST API. Invocation traffic is routed directly by the gateway to the Gleam child-process runner." },
    PathRow { paths: &[PathEntry { label: "/presence-test", href: Some("/presence-test?user=alice&device=d1&autoconnect=1") }], target: "gleamlang-presence-server browser harness", access: PUBLIC, notes: "Self-contained page that opens one user-scoped ws plus N conv-scoped ws connections against the presence server." },
    PathRow { paths: &[PathEntry { label: "/wss-test", href: Some("/wss-test") }, PathEntry { label: "?preset=gleam", href: Some("/wss-test?preset=gleam") }, PathEntry { label: "?preset=webrtc", href: Some("/wss-test?preset=webrtc") }, PathEntry { label: "?preset=gcs", href: Some("/wss-test?preset=gcs") }, PathEntry { label: "?preset=fsrx", href: Some("/wss-test?preset=fsrx") }], target: "Gateway WebSocket test lab", access: PUBLIC, notes: "Rust-served browser harness for the Gleam fan-out socket, Rust WebRTC signaling socket, gcs/chat.vibe router, and F# Rx burst endpoint." },
    PathRow { paths: &[PathEntry { label: "/auth", href: Some("/auth?return=/home") }, PathEntry { label: "/auth/login", href: Some("/auth/login") }, PathEntry { label: "/auth/logout", href: Some("/auth/logout") }], target: "dd-remote-auth Rust PIN auth", access: PUBLIC, notes: "Sets the temporary dd_auth cookie so the gateway can accept browser sessions without the legacy Auth header." },
    PathRow { paths: &[PathEntry { label: "/bastion/runtime/deployments", href: Some("/bastion/runtime/deployments") }, PathEntry { label: "/bastion/profile", href: Some("/bastion/profile") }, PathEntry { label: "/bastion/terminal", href: None }], target: "Rust bastion/jumphost access broker", access: SERVER_AUTH, notes: "Same-origin gateway access to bastion inventory and allowlisted browser exec terminals." },
    PathRow { paths: &[PathEntry { label: "/headlamp/", href: Some("/headlamp/") }], target: "Headlamp Kubernetes UI", access: SERVER_AUTH, notes: "Read-only cluster browser for workload, pod, container, logs, node, Argo CD, KEDA, and External Secrets state. Paste a token from `kubectl -n headlamp create token headlamp-viewer`." },
    PathRow { paths: &[PathEntry { label: "dd.remote.thread.*.tasks", href: None }, PathEntry { label: "POST /api/agents/threads/<uuid>/prepare", href: Some("/api/agents/threads/example-thread-id/prepare") }], target: "Rust NATS Queue Consumer", access: INTERNAL_ACCESS, notes: "Shadow consumer reads task messages, keeps thread affinity, and prepares the matching UUID-bound worker. It does not execute prompts." },
    PathRow { paths: &[PathEntry { label: "/dd-thread/<short>", href: Some("/dd-thread/example") }, PathEntry { label: "/dd-thread/<short>/tasks", href: Some("/dd-thread/example/tasks") }, PathEntry { label: "/dd-thread/<short>/stream/<taskId>", href: Some("/dd-thread/example/stream/example-task-id") }, PathEntry { label: "/dd-thread/<short>/ws", href: Some("/dd-thread/example/ws") }], target: "Kubernetes per-thread Ingress", access: SERVER_AUTH, notes: "Ingress selects the UUID-bound worker Service; Node.js handles only the task inside that selected container." },
    PathRow { paths: &[PathEntry { label: "/gleam/home", href: Some("/gleam/home") }, PathEntry { label: "/gleam/healthz", href: Some("/gleam/healthz") }, PathEntry { label: "/gleam/metrics", href: Some("/gleam/metrics") }, PathEntry { label: "/gleam/ws", href: None }], target: "Gleam WebSocket service", access: INTERNAL_ACCESS, notes: "Gleam/OTP fan-out socket behind the gateway; WebSocket endpoint is wss://<host>/gleam/ws." },
    PathRow { paths: &[PathEntry { label: "/mcp", href: Some("/mcp") }, PathEntry { label: "/mcp/home", href: Some("/mcp/home") }, PathEntry { label: "/mcp/healthz", href: Some("/mcp/healthz") }, PathEntry { label: "/mcp/metrics", href: Some("/mcp/metrics") }], target: "Gleam MCP service", access: INTERNAL_ACCESS, notes: "Dedicated MCP deployment with read-only runtime tools, Prometheus metrics, and Loki-collected stdout logs." },
    PathRow { paths: &[PathEntry { label: "/webrtc/", href: Some("/webrtc/") }, PathEntry { label: "/webrtc/healthz", href: Some("/webrtc/healthz") }, PathEntry { label: "/webrtc/metrics", href: Some("/webrtc/metrics") }, PathEntry { label: "/webrtc/signal test", href: Some("/wss-test?preset=webrtc") }], target: "Rust WebRTC signaling service", access: PUBLIC, notes: "Room WebSocket signaling for browser/mobile peer handshakes. Media and data channels stay peer-to-peer." },
    PathRow { paths: &[PathEntry { label: "/mdp/", href: Some("/mdp/") }, PathEntry { label: "/mdp/healthz", href: Some("/mdp/healthz") }, PathEntry { label: "/mdp/metrics", href: Some("/mdp/metrics") }, PathEntry { label: "POST /mdp/optimize", href: Some("/mdp/optimize") }, PathEntry { label: "POST /mdp/telemetry/learn", href: Some("/mdp/telemetry/learn") }, PathEntry { label: "dd.remote.mdp.optimize", href: None }, PathEntry { label: "dd.remote.telemetry.mdp", href: None }], target: "Rust MDP/POMDP optimizer", access: PUBLIC, notes: "Async optimizer that subscribes to NATS optimization and telemetry jobs, then publishes results/events back to the runtime queue." },
    PathRow { paths: &[PathEntry { label: "/des/", href: Some("/des/") }, PathEntry { label: "/des/healthz", href: Some("/des/healthz") }, PathEntry { label: "/des/metrics", href: Some("/des/metrics") }, PathEntry { label: "/des/model/schema", href: Some("/des/model/schema") }, PathEntry { label: "/des/model/example", href: Some("/des/model/example") }, PathEntry { label: "POST /des/validate", href: Some("/des/validate") }, PathEntry { label: "POST /des/simulate", href: Some("/des/simulate") }, PathEntry { label: "dd.remote.des.simulate", href: None }], target: "Rust discrete event simulator", access: PUBLIC, notes: "Async DES job runner with declared des.v1 model schema, strict validation, in-memory job status, metrics, and NATS result publishing." },
    PathRow { paths: &[PathEntry { label: "/contracts/", href: Some("/contracts/") }, PathEntry { label: "/contracts/healthz", href: Some("/contracts/healthz") }, PathEntry { label: "/contracts/metrics", href: Some("/contracts/metrics") }, PathEntry { label: "/contracts/schema", href: Some("/contracts/schema") }, PathEntry { label: "/contracts/example", href: Some("/contracts/example") }, PathEntry { label: "POST /contracts/validate", href: Some("/contracts/validate") }, PathEntry { label: "POST /contracts/simulate", href: Some("/contracts/simulate") }, PathEntry { label: "dd.remote.contracts.solana.validate", href: None }], target: "Rust Solana contract service", access: SERVER_AUTH, notes: "Validates solana.contract.v1 instruction envelopes, proxies signed simulation through Solana JSON-RPC, and publishes NATS validation results." },
    PathRow { paths: &[PathEntry { label: "/ml/", href: Some("/ml/") }, PathEntry { label: "/ml/healthz", href: Some("/ml/healthz") }, PathEntry { label: "/ml/metrics", href: Some("/ml/metrics") }, PathEntry { label: "/ml/status", href: Some("/ml/status") }, PathEntry { label: "POST /ml/analyze", href: Some("/ml/analyze") }, PathEntry { label: "POST /ml/ingest", href: Some("/ml/ingest") }, PathEntry { label: "dd.remote.telemetry.raw", href: None }, PathEntry { label: "dd.remote.ml.features", href: None }], target: "Python AI/ML feature pipeline", access: SERVER_AUTH, notes: "Normalizes runtime telemetry into features, EWMA baselines, z-score anomalies, transition estimates, and MDP telemetry requests." },
    PathRow { paths: &[PathEntry { label: "/trading/", href: Some("/trading/") }, PathEntry { label: "/trading/healthz", href: Some("/trading/healthz") }, PathEntry { label: "/trading/metrics", href: Some("/trading/metrics") }, PathEntry { label: "/trading/schema", href: Some("/trading/schema") }, PathEntry { label: "/trading/example", href: Some("/trading/example") }, PathEntry { label: "POST /trading/decide", href: Some("/trading/decide") }, PathEntry { label: "dd.remote.trading.signals", href: None }, PathEntry { label: "dd.remote.trading.order_intents", href: None }], target: "Rust trading decision service", access: SERVER_AUTH, notes: "Combines scraped web sentiment, AI/ML features, market snapshots, and MDP/POMDP hints into risk-gated buy/sell/hold decisions." },
    PathRow { paths: &[PathEntry { label: "POST /scrape", href: Some("/scrape") }, PathEntry { label: "/scrape/strategies", href: Some("/scrape/strategies") }, PathEntry { label: "/scrape/healthz", href: Some("/scrape/healthz") }, PathEntry { label: "/scrape/metrics", href: Some("/scrape/metrics") }], target: "dd-web-scraper Fastify deployment", access: SERVER_AUTH, notes: "Long-running strategy router for native fetch, Cheerio, JSDOM, LinkeDOM, Playwright, Puppeteer, and Browserless scraping." },
    PathRow { paths: &[PathEntry { label: "POST /builds", href: Some("/builds") }, PathEntry { label: "/builds/<jobId>", href: Some("/builds/example-job") }, PathEntry { label: "/builds/<jobId>/logs", href: Some("/builds/example-job/logs") }], target: "dd-build-server Rust CI/CD deployment", access: SERVER_AUTH, notes: "Authenticated repo build queue. Jobs are build-server.v1 JSON, push only to allowlisted ECR prefixes, and deploy only allowlisted manifests/namespaces." },
    PathRow { paths: &[PathEntry { label: "/telemetry/", href: Some("/telemetry/") }], target: "Grafana", access: INTERNAL_ACCESS, notes: "Primary HTML dashboard for Prometheus metrics, Loki logs, Tempo traces, and NATS metrics." },
    PathRow { paths: &[PathEntry { label: "/prometheus/", href: Some("/prometheus/") }], target: "Prometheus", access: INTERNAL_ACCESS, notes: "Low-level metrics UI and query surface." },
    PathRow { paths: &[PathEntry { label: "/nats/", href: Some("/nats/") }, PathEntry { label: "/nats-metrics/metrics", href: Some("/nats-metrics/metrics") }], target: "NATS monitor and exporter", access: INTERNAL_ACCESS, notes: "NATS should usually be inspected through Grafana; these paths expose raw health and metrics." },
    PathRow { paths: &[PathEntry { label: "/reaper/", href: Some("/reaper/") }, PathEntry { label: "/cron/", href: Some("/cron/") }], target: "Runtime service status", access: INTERNAL_ACCESS, notes: "Gateway status surfaces for idle reaper and cron scheduler deployments." },
    PathRow { paths: &[PathEntry { label: "/fsws/", href: Some("/fsws/") }, PathEntry { label: "/fsws/healthz", href: Some("/fsws/healthz") }, PathEntry { label: "/fsws/livez", href: Some("/fsws/livez") }, PathEntry { label: "/fsws/ws/rx", href: None }, PathEntry { label: "/fsws/ws/async", href: None }, PathEntry { label: "/wss-test?preset=fsrx", href: Some("/wss-test?preset=fsrx") }], target: "dd-fsharp-ws-server", access: PUBLIC, notes: "F# + ASP.NET Core burst WebSocket server. The gateway strips the /fsws/ prefix before proxying to the upstream." },
    PathRow { paths: &[PathEntry { label: "/gcs/health", href: Some("/gcs/health") }, PathEntry { label: "/gcs/ws-health", href: Some("/gcs/ws-health") }, PathEntry { label: "/gcs/api/<...>", href: None }, PathEntry { label: "/gcs/ws/conv/<convId>", href: None }, PathEntry { label: "/gcs/ws/user/<userId>", href: None }, PathEntry { label: "/gcs/ws/device/<deviceId>", href: None }, PathEntry { label: "/wss-test?preset=gcs", href: Some("/wss-test?preset=gcs") }], target: "gcs / chat.vibe websocket router", access: SERVER_AUTH, notes: "HTTP API rewrites to /chat/* on gcs; websocket traffic is routed through gcs-router for conv/user/device pinning." },
    PathRow { paths: &[PathEntry { label: "/v1/tenants", href: None }, PathEntry { label: "/v1/tenants/<tenant_id>", href: None }, PathEntry { label: "/v1/tenants/<tenant_id>/customers/by-email/<email>/billing-state", href: None }, PathEntry { label: "/v1/tenants/<tenant_id>/vendors/by-email/<email>/payable-state", href: None }, PathEntry { label: "/v1/tenants/<tenant_id>/connections", href: None }, PathEntry { label: "POST /v1/oauth/<provider>/start", href: None }, PathEntry { label: "GET /v1/oauth/<provider>/callback", href: None }, PathEntry { label: "POST /v1/webhooks/<provider>", href: None }, PathEntry { label: "GET /v1/verify/tenants/<tenant_id>/postings/<id>", href: None }], target: "dd-billing-server Rust ledger service", access: CLUSTER_LOCAL, notes: "Multi-tenant AR/AP ledger. Public verification needs no auth; provider webhooks update ledger state in seconds." },
    PathRow { paths: &[PathEntry { label: "cdc.<schema>.<table>.<op>", href: None }, PathEntry { label: "JetStream stream CDC", href: None }, PathEntry { label: "/healthz", href: None }, PathEntry { label: "/readyz", href: None }, PathEntry { label: "/metrics", href: None }], target: "dd-wal-gateway (postgres-to-NATS CDC)", access: INTERNAL_ACCESS, notes: "One advisory-locked logical replication slot pumps wal2json rows into JetStream as cdc.row.v1 envelopes." },
];

const HOME_CSS: &str = r##"
:root {
  color-scheme: dark;
  --bg: #0b1117;
  --panel: #111923;
  --panel-2: #0f1720;
  --line: rgba(148, 163, 184, 0.24);
  --text: #eef2f6;
  --muted: #a8b3c1;
  --accent: #5eead4;
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
.shell { max-width: 1180px; margin: 0 auto; }
h1 { margin: 0 0 10px; font-size: 30px; }
h2 { margin: 0 0 12px; font-size: 17px; }
p { margin: 0 0 14px; color: var(--muted); line-height: 1.5; }
a { color: var(--accent); text-decoration: none; }
a:hover { text-decoration: underline; }
.grid {
  display: grid;
  grid-template-columns: repeat(4, minmax(0, 1fr));
  gap: 14px;
  margin: 18px 0;
}
.panel {
  border: 1px solid var(--line);
  border-radius: 8px;
  background: var(--panel);
  padding: 14px;
}
.label {
  display: block;
  font-size: 11px;
  color: var(--muted);
  margin-bottom: 7px;
  text-transform: uppercase;
  letter-spacing: 0.08em;
}
.value { font-size: 14px; line-height: 1.35; }
.band {
  border: 1px solid var(--line);
  border-radius: 8px;
  background: var(--panel-2);
  padding: 16px;
  margin-top: 16px;
}
table {
  width: 100%;
  border-collapse: collapse;
  table-layout: fixed;
  font-size: 13px;
}
th, td {
  border-top: 1px solid var(--line);
  padding: 11px 10px;
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
.path-links {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
}
.path-links a { text-decoration: none; }
.path-links a:hover code,
.path-links a:focus-visible code {
  border-color: rgba(94, 234, 212, 0.62);
  background: rgba(94, 234, 212, 0.1);
}
.pill {
  display: inline-flex;
  align-items: center;
  min-height: 24px;
  border-radius: 999px;
  border: 1px solid rgba(94, 234, 212, 0.35);
  padding: 2px 8px;
  color: var(--accent);
  background: rgba(94, 234, 212, 0.08);
  font-size: 12px;
}
.pill.warn {
  border-color: rgba(251, 191, 36, 0.35);
  color: var(--warn);
  background: rgba(251, 191, 36, 0.08);
}
.service-actions {
  display: flex;
  flex-wrap: wrap;
  gap: 7px;
  align-items: center;
}
button {
  border: 1px solid rgba(94, 234, 212, 0.34);
  border-radius: 6px;
  background: rgba(94, 234, 212, 0.09);
  color: var(--text);
  padding: 6px 9px;
  font: inherit;
  font-size: 12px;
  cursor: pointer;
}
button:hover,
button:focus-visible {
  border-color: rgba(94, 234, 212, 0.72);
  outline: none;
}
button:disabled {
  cursor: not-allowed;
  opacity: 0.55;
}
.live-toolbar {
  display: flex;
  justify-content: space-between;
  gap: 12px;
  align-items: center;
  flex-wrap: wrap;
  margin-bottom: 12px;
}
.live-toolbar h2 { margin-bottom: 0; }
.container-cell {
  display: grid;
  gap: 8px;
}
.container-item {
  display: grid;
  gap: 5px;
}
.terminal-dock {
  display: grid;
  gap: 10px;
  margin-top: 14px;
}
.terminal-dock[hidden] { display: none; }
.terminal-head {
  display: flex;
  justify-content: space-between;
  gap: 10px;
  align-items: center;
}
.terminal-frame {
  width: 100%;
  height: 460px;
  border: 1px solid var(--line);
  border-radius: 8px;
  background: #05080d;
}
ol {
  margin: 8px 0 0;
  padding-left: 22px;
  color: var(--muted);
  line-height: 1.55;
}
@media (max-width: 880px) {
  .grid { grid-template-columns: 1fr; }
  table, thead, tbody, th, td, tr { display: block; }
  th { display: none; }
  td { border-top: 0; padding: 5px 0; }
  tr { border-top: 1px solid var(--line); padding: 10px 0; }
}
"##;

const HOME_LIVE_CONTAINERS_JS: &str = r##"
(() => {
  const body = document.getElementById("live-containers-body");
  const status = document.getElementById("live-containers-status");
  const refresh = document.getElementById("live-containers-refresh");
  const dock = document.getElementById("home-terminal");
  const frame = document.getElementById("home-terminal-frame");
  const caption = document.getElementById("home-terminal-caption");
  const close = document.getElementById("home-terminal-close");
  let inventoryStatus = "loading managed deployment pods";
  let reloadTimer = 0;
  const wsStatus = { gleam: "idle", rust: "idle" };
  const renderStatus = () => {
    status.textContent = `${inventoryStatus} · gleam ws ${wsStatus.gleam} · rust ws ${wsStatus.rust}`;
  };
  const setStatus = (message) => {
    inventoryStatus = message;
    renderStatus();
  };
  const setWsStatus = (name, message) => {
    wsStatus[name] = message;
    renderStatus();
  };
  const text = (value) => document.createTextNode(value == null || value === "" ? "none" : String(value));
  const cell = (child) => {
    const td = document.createElement("td");
    if (typeof child === "string") td.appendChild(text(child));
    else td.appendChild(child);
    return td;
  };
  const code = (value) => {
    const el = document.createElement("code");
    el.textContent = value == null || value === "" ? "none" : String(value);
    return el;
  };
  const pill = (value, warn) => {
    const el = document.createElement("span");
    el.className = warn ? "pill warn" : "pill";
    el.textContent = value;
    return el;
  };
  const shortContainerId = (value) => String(value || "").replace(/^\w+:\/\//, "").slice(0, 18);
  const stateText = (container) => {
    const state = container?.state || {};
    if (state.running) return "running";
    if (state.waiting) return "waiting " + (state.waiting.reason || "unknown");
    if (state.terminated) return "terminated " + (state.terminated.reason || "unknown");
    return "unknown";
  };
  const safeBastionTerminalUrl = (value) => {
    try {
      const url = new URL(String(value || ""), window.location.origin);
      if (url.origin !== window.location.origin || url.pathname !== "/bastion/terminal") return "";
      for (const key of ["namespace", "deployment", "pod", "container"]) {
        if (!url.searchParams.get(key)) return "";
      }
      return `${url.pathname}${url.search}`;
    } catch {
      return "";
    }
  };
  const openTerminal = (url, label) => {
    const targetUrl = safeBastionTerminalUrl(url);
    if (!targetUrl) {
      setStatus("ignored unsafe bastion terminal URL");
      return;
    }
    caption.textContent = label;
    frame.src = targetUrl;
    dock.hidden = false;
    dock.scrollIntoView({ behavior: "smooth", block: "start" });
  };
  const renderEmpty = (message) => {
    body.textContent = "";
    const tr = document.createElement("tr");
    const td = document.createElement("td");
    td.colSpan = 5;
    td.className = "muted";
    td.textContent = message;
    tr.appendChild(td);
    body.appendChild(tr);
  };
  const render = (data) => {
    body.textContent = "";
    let rowCount = 0;
    for (const deployment of data.deployments || []) {
      const pods = deployment.pods || [];
      if (!pods.length) {
        const tr = document.createElement("tr");
        tr.append(
          cell(deployment.deployment),
          cell(deployment.namespace),
          cell("no pods"),
          cell((deployment.errors || []).join("; ") || "deployment has no selected pods"),
          cell("none")
        );
        body.appendChild(tr);
        rowCount += 1;
        continue;
      }
      for (const pod of pods) {
        const containers = pod.containers || [];
        const containerCell = document.createElement("div");
        containerCell.className = "container-cell";
        const actions = document.createElement("div");
        actions.className = "service-actions";
        for (const container of containers) {
          const item = document.createElement("div");
          item.className = "container-item";
          const meta = document.createElement("span");
          meta.append(code(container.name));
          meta.append(" ");
          meta.append(pill(stateText(container), !container.ready));
          meta.append(" restarts ");
          meta.append(code(container.restartCount || 0));
          item.appendChild(meta);
          if (container.containerId) {
            const idLine = document.createElement("span");
            idLine.className = "muted";
            idLine.append("id ");
            idLine.append(code(shortContainerId(container.containerId)));
            item.appendChild(idLine);
          }
          containerCell.appendChild(item);
          const safeTerminalUrl = safeBastionTerminalUrl(container.terminalUrl);
          const button = document.createElement("button");
          button.type = "button";
          button.textContent = "Terminal";
          button.disabled = !safeTerminalUrl || !data.terminalEnabled;
          button.title = button.disabled ? "terminal unavailable" : "Open bastion exec terminal";
          button.addEventListener("click", () => openTerminal(safeTerminalUrl, deployment.namespace + "/" + pod.name + "/" + container.name));
          actions.appendChild(button);
        }
        const podCell = document.createElement("div");
        podCell.className = "container-cell";
        podCell.append(code(pod.name));
        podCell.append(pill(pod.phase || "unknown", pod.phase !== "Running"));
        const tr = document.createElement("tr");
        tr.append(
          cell(deployment.deployment),
          cell(deployment.namespace),
          cell(podCell),
          cell(containerCell),
          cell(actions)
        );
        body.appendChild(tr);
        rowCount += 1;
      }
    }
    if (!rowCount) renderEmpty("No managed deployment pods returned.");
    setStatus((data.deployments || []).length + " managed deployments · " + (data.terminalEnabled ? "terminal enabled" : "terminal disabled"));
  };
  const load = async () => {
    setStatus("loading managed deployment pods");
    refresh.disabled = true;
    try {
      const response = await fetch("/bastion/runtime/deployments", { cache: "no-store", credentials: "same-origin" });
      if (response.status === 401) {
        renderEmpty("Sign in through /auth?return=/home to load live containers and bastion terminals.");
        setStatus("auth required");
        return;
      }
      if (!response.ok) throw new Error("runtime inventory failed " + response.status);
      render(await response.json());
    } catch (error) {
      renderEmpty(String(error));
      setStatus("live container inventory unavailable");
    } finally {
      refresh.disabled = false;
    }
  };
  const scheduleRuntimeReload = (label, event) => {
    const kind = event.kind || "runtime";
    const name = event.name || "resource";
    setStatus(`${label} update ${kind}/${name}; refreshing`);
    window.clearTimeout(reloadTimer);
    reloadTimer = window.setTimeout(load, 700);
  };
  const handleRuntimeMessage = (label, event) => {
    let parsed;
    try {
      parsed = JSON.parse(event.data);
    } catch {
      return;
    }
    if (parsed.type === "k8s-runtime-event") {
      scheduleRuntimeReload(label, parsed);
    }
  };
  const openRuntimeSocket = (name, path, attempt = 0) => {
    const protocol = window.location.protocol === "https:" ? "wss" : "ws";
    const ws = new WebSocket(`${protocol}://${window.location.host}${path}`);
    setWsStatus(name, "connecting");
    ws.onopen = () => {
      setWsStatus(name, "connected");
      if (name === "gleam") ws.send("ping");
      if (name === "rust") ws.send(JSON.stringify({ type: "ping" }));
    };
    ws.onmessage = (event) => handleRuntimeMessage(name, event);
    ws.onerror = () => setWsStatus(name, "error");
    ws.onclose = () => {
      setWsStatus(name, "closed");
      if (attempt < 8) {
        const delay = Math.min(30000, 1000 * (attempt + 1));
        window.setTimeout(() => openRuntimeSocket(name, path, attempt + 1), delay);
      }
    };
  };
  const connectRuntimeSockets = () => {
    const clientId = Math.random().toString(36).slice(2);
    openRuntimeSocket("gleam", `/admin/gleam/ws?channel=k8s-runtime-admin&client=home-${clientId}`);
    openRuntimeSocket("rust", `/admin/webrtc/runtime/ws?client=home-${clientId}`);
  };
  refresh.addEventListener("click", load);
  close.addEventListener("click", () => {
    frame.src = "about:blank";
    dock.hidden = true;
  });
  load();
  connectRuntimeSockets();
})();
"##;

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

async fn presence_test_page() -> impl IntoResponse {
    record_request("GET", "/presence-test", StatusCode::OK);
    Html(PRESENCE_TEST_HTML)
}

async fn wss_test_page() -> impl IntoResponse {
    record_request("GET", "/wss-test", StatusCode::OK);
    Html(WSS_TEST_HTML)
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
                    script defer="defer" src="https://cdn.jsdelivr.net/npm/rxjs@7.8.1/dist/bundles/rxjs.umd.min.js" crossorigin="anonymous" {}
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
            main id="thread-workspace" class="main mode-empty control-wide" {
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

                section id="thread-control-panel" class="panel prompt-panel" tabindex="0" aria-label="Thread control panel" {
                    div class="topbar thread-control-heading" {
                        div {
                            h2 { "Thread Control" }
                            p id="thread-control-subtitle" { "Select an existing worker thread or prepare a new one." }
                        }
                        span id="thread-mode" class="pill warn" { "select thread" }
                    }
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
                                option value="claude-sdk" selected { "claude-sdk" }
                                option value="gemini-sdk" { "gemini-sdk" }
                                option value="openai-sdk" { "openai-sdk" }
                            }
                        }
                        label {
                            span { "Dispatch mode" }
                            select id="dispatch-mode" {
                                option value="queued" selected { "queued NATS" }
                                option value="queued-pool" { "NATS container pool" }
                                option value="direct" { "direct REST" }
                            }
                        }
                        label class="field-wide" {
                            span { "Git repo URL" }
                            select id="repo-url" {}
                        }
                        label id="repo-url-new-row" class="field-wide" hidden="hidden" {
                            span { "New repo URL" }
                            input id="repo-url-new" autocomplete="off" spellcheck="false" placeholder="git@github.com:org/repo.git or org/repo";
                        }
                        label {
                            span { "Base branch" }
                            input id="base-branch" autocomplete="off" spellcheck="false" value="dev";
                        }
                        label class="field-wide" {
                            span { "Prompt" }
                            textarea id="prompt" placeholder="Ask this thread worker to do something" {}
                        }
                    }
                    div class="actions prompt-actions" {
                        button id="save-repo" type="button" title="Save this repo URL and default branch to the known repo list" { "Save repo URL" }
                        button id="new-task" type="button" { "New task" }
                        button id="sleep-thread" type="button" title="Reduce resources by scaling the thread container to zero" { "Pause/Sleep" }
                        button id="archive-thread" class="warn" type="button" title="Deep sleep: suspend the thread container" { "Archive" }
                        button id="delete-thread" class="danger" type="button" { "Delete runtime" }
                        button id="merge-thread" type="button" { "Merge with upstream" }
                        button id="commit-thread" type="button" title="Commit current worker changes and push the thread branch" { "Make commit" }
                        button id="open-pr-thread" type="button" { "Open draft PR" }
                        button id="terminal-thread" type="button" title="Open a shell in the thread's Node.js worker container" { "Terminal" }
                        button id="send" class="primary" type="button" { "Send" }
                    }
                    p id="status-line" class="muted status-line" { "idle" }
                }

                div id="task-stream-grid" class="grid task-stream-grid" {
                    section id="previous-tasks-panel" class="panel" tabindex="0" aria-label="Previous tasks panel" {
                        div class="topbar" {
                            h2 { "Previous tasks" }
                            span id="task-count" class="pill" { "0 tasks" }
                        }
                        div id="task-list" class="task-list" {}
                    }
                    section id="response-stream-panel" class="panel" tabindex="0" aria-label="Response stream panel" {
                        div class="topbar" {
                            h2 { "Response stream" }
                            span id="stream-state" class="pill warn" { "no task selected" }
                        }
                        div id="stream" class="stream" aria-live="polite" {}
                        div id="terminal-inline" class="terminal-inline" hidden="hidden" {
                            div class="terminal-head" {
                                div {
                                    h3 { "Terminal" }
                                    p id="terminal-caption" class="muted" { "worker shell" }
                                }
                                button id="terminal-close" type="button" title="Close terminal" { "Close" }
                            }
                            iframe id="terminal-frame" title="Thread worker terminal" {}
                        }
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
                            option value="gemini-sdk" { "gemini-sdk" }
                            option value="claude-cli" { "claude-cli" }
                            option value="openai-codex-cli" { "openai-codex-cli" }
                            option value="openai-sdk" { "openai-sdk" }
                        }
                    }
                    label class="field field-wide" {
                        span { "Git repo URL" }
                        select id="chat-repo-url" {}
                    }
                    label id="chat-repo-url-new-row" class="field field-wide" hidden="hidden" {
                        span { "New repo URL" }
                        input id="chat-repo-url-new" autocomplete="off" spellcheck="false" placeholder="git@github.com:org/repo.git or org/repo";
                    }
                    label class="field" {
                        span { "Base branch" }
                        input id="chat-base-branch" autocomplete="off" spellcheck="false" value="dev";
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
                    button id="save-chat-repo" type="button" title="Save this repo URL and default branch to the known repo list" { "Save repo URL" }
                    button id="thread-sleep" type="button" title="Reduce resources by scaling the thread container to zero" { "Pause/Sleep" }
                    button id="thread-archive" class="warn" type="button" title="Deep sleep: suspend the thread container" { "Archive" }
                    button id="thread-delete" class="danger" type="button" { "Delete runtime" }
                    button id="thread-merge" type="button" { "Merge with upstream" }
                    button id="thread-commit" type="button" title="Commit current worker changes and push the thread branch" { "Make commit" }
                    button id="thread-open-pr" type="button" { "Open draft PR" }
                    button id="thread-terminal" type="button" title="Open a shell in the thread's Node.js worker container" { "Terminal" }
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
      input:invalid, select:invalid {
        border-color: rgba(251, 113, 133, 0.7);
        box-shadow: 0 0 0 1px rgba(251, 113, 133, 0.18);
      }
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
        --control-share: 1;
        --lower-share: 1;
        min-width: 0;
        min-height: 0;
        padding: 22px;
        display: flex;
        flex-direction: column;
        gap: 16px;
        overflow: hidden;
      }
      .main.control-wide {
        --control-share: 1.2;
        --lower-share: 0.8;
      }
      .main.lower-wide {
        --control-share: 0.8;
        --lower-share: 1.2;
      }
      .main.mode-empty #sleep-thread,
      .main.mode-empty #archive-thread,
      .main.mode-empty #delete-thread,
      .main.mode-empty #merge-thread,
      .main.mode-empty #commit-thread,
      .main.mode-empty #open-pr-thread,
      .main.mode-empty #terminal-thread,
      .main.mode-new #sleep-thread,
      .main.mode-new #archive-thread,
      .main.mode-new #delete-thread,
      .main.mode-new #merge-thread,
      .main.mode-new #commit-thread,
      .main.mode-new #open-pr-thread,
      .main.mode-new #terminal-thread {
        display: none;
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
        flex: var(--control-share) 1 0;
        min-height: 170px;
        overflow: hidden auto;
        overscroll-behavior: contain;
        position: relative;
        z-index: 1;
        transition: flex-grow 160ms ease;
      }
      .main.control-wide .prompt-panel {
        min-height: 230px;
      }
      .main.lower-wide .prompt-panel {
        min-height: 140px;
      }
      .main.mode-existing.lower-wide textarea {
        min-height: 78px;
        max-height: 28dvh;
      }
      .thread-control-heading {
        margin-bottom: 12px;
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
        transition: grid-template-columns 160ms ease, flex-grow 160ms ease;
      }
      .task-stream-grid.tasks-wide {
        grid-template-columns: minmax(0, 1.02fr) minmax(0, 0.98fr);
      }
      .task-stream-grid.stream-wide {
        grid-template-columns: minmax(0, 0.62fr) minmax(0, 1.38fr);
      }
      #previous-tasks-panel,
      #response-stream-panel,
      #thread-control-panel {
        cursor: pointer;
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
      .terminal-inline {
        flex: 1 1 auto;
        min-height: 0;
        display: flex;
        flex-direction: column;
        gap: 10px;
      }
      .terminal-inline[hidden] {
        display: none;
      }
      .terminal-head {
        flex: 0 0 auto;
        display: flex;
        align-items: center;
        justify-content: space-between;
        gap: 10px;
      }
      .terminal-inline iframe {
        flex: 1 1 auto;
        width: 100%;
        min-height: 260px;
        border: 1px solid var(--line);
        border-radius: 8px;
        background: #050806;
      }
      #response-stream-panel.terminal-open #stream {
        display: none;
      }
      .main > .topbar {
        flex: 0 0 auto;
      }
      .main > .grid {
        flex: var(--lower-share) 1 0;
        min-height: 0;
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
      .grid > .panel > .stream,
      .grid > .panel > .terminal-inline {
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
        knownRepos: [],
        selectedThreadId: null,
        selectedTaskId: null,
        liveSource: null,
        liveWs: null,
        renderedEvents: new Set(),
        streamTaskId: null,
        runtimePoll: null,
        lastRuntimeSummary: "",
        lastRuntimeData: null,
        threadUiMode: "empty",
        snapshotFailures: 0,
        snapshotRetryTimer: null,
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

      function terminalUrl(threadId) {
        return `/dd-thread/${shortId(threadId).toLowerCase()}/terminal?threadId=${encodeURIComponent(threadId)}`;
      }

      const UUID_PATTERN = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

      function normalizeUuid(value) {
        return String(value || "").trim().toLowerCase();
      }

      function isUuid(value) {
        return UUID_PATTERN.test(normalizeUuid(value));
      }

      function readUuidInput(id, label, options = {}) {
        const input = $(id);
        let value = normalizeUuid(input.value);
        if (!value && options.generate) {
          value = makeUuid();
          input.value = value;
        }
        if (!value && options.allowEmpty) {
          input.setCustomValidity("");
          return "";
        }
        if (!isUuid(value)) {
          input.setCustomValidity(`${label} must be a UUID`);
          input.reportValidity?.();
          setStatus(`${label} must be a UUID`, true);
          return null;
        }
        input.value = value;
        input.setCustomValidity("");
        return value;
      }

      function queryUuid(params, name) {
        const value = normalizeUuid(params.get(name));
        return isUuid(value) ? value : null;
      }

      function trustedTerminalUrl(threadId, candidate) {
        const fallback = terminalUrl(threadId);
        if (!candidate) return fallback;
        try {
          const parsed = new URL(String(candidate), window.location.origin);
          const expectedPath = `/dd-thread/${shortId(threadId).toLowerCase()}/terminal`;
          const returnedThreadId = normalizeUuid(parsed.searchParams.get("threadId"));
          if (parsed.origin !== window.location.origin || parsed.pathname !== expectedPath || returnedThreadId !== normalizeUuid(threadId)) {
            throw new Error("unexpected terminal URL");
          }
          return `${parsed.pathname}${parsed.search}`;
        } catch {
          renderError("ignored unsafe terminal URL from control response");
          return fallback;
        }
      }

      function terminalUrlFromControlResponse(threadId, body) {
        try {
          const parsed = JSON.parse(body);
          return trustedTerminalUrl(threadId, parsed.terminalUrl);
        } catch {
          return terminalUrl(threadId);
        }
      }

      function setWorkspaceLayout(mode) {
        const workspace = $("thread-workspace");
        workspace.classList.remove("control-wide", "lower-wide");
        if (mode === "control") workspace.classList.add("control-wide");
        if (mode === "lower") workspace.classList.add("lower-wide");
      }

      function setThreadUiMode(modeName) {
        const workspace = $("thread-workspace");
        state.threadUiMode = modeName;
        workspace.classList.remove("mode-empty", "mode-new", "mode-existing");
        workspace.classList.add(`mode-${modeName}`);
        $("new-task").disabled = modeName === "empty";
        $("send").textContent = modeName === "new" ? "Create thread & send" : modeName === "existing" ? "Send task" : "Send";
        for (const id of ["sleep-thread", "archive-thread", "delete-thread", "merge-thread", "commit-thread", "open-pr-thread", "terminal-thread"]) {
          $(id).disabled = modeName !== "existing";
        }
      }

      function setTaskStreamLayout(mode) {
        const grid = $("task-stream-grid");
        grid.classList.remove("tasks-wide", "stream-wide");
        if (mode === "tasks") grid.classList.add("tasks-wide");
        if (mode === "stream") grid.classList.add("stream-wide");
        setWorkspaceLayout("lower");
      }

      function handlePanelKey(event, mode) {
        if (shouldIgnorePanelShortcut(event.target)) return;
        if (event.key !== "Enter" && event.key !== " ") return;
        event.preventDefault();
        setTaskStreamLayout(mode);
      }

      function shouldIgnorePanelShortcut(target) {
        return Boolean(target?.closest?.("button, input, select, textarea, a"));
      }

      function handleControlPanelClick(event) {
        if (shouldIgnorePanelShortcut(event.target)) return;
        setWorkspaceLayout("control");
      }

      function handleLowerPanelClick(event, mode) {
        if (shouldIgnorePanelShortcut(event.target)) return;
        setTaskStreamLayout(mode);
      }

      function handleControlPanelKey(event) {
        if (shouldIgnorePanelShortcut(event.target)) return;
        if (event.key !== "Enter" && event.key !== " ") return;
        event.preventDefault();
        setWorkspaceLayout("control");
      }

      function replaceSelectionUrl(threadId, taskId) {
        const url = new URL(window.location.href);
        if (threadId) url.searchParams.set("thread", threadId);
        else url.searchParams.delete("thread");
        if (taskId) url.searchParams.set("task", taskId);
        else url.searchParams.delete("task");
        window.history.replaceState(null, "", url);
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

      function adminDetailText(value) {
        if (value instanceof Error) return value.stack || `${value.name}: ${value.message}`;
        if (typeof value === "string") return value;
        try {
          return JSON.stringify(value, null, 2);
        } catch (_error) {
          return String(value);
        }
      }

      function logAdminDetail(label, value) {
        try {
          console.error(`[agents admin] ${label}`, value);
        } catch (_error) {
          console.error(`[agents admin] ${label}: ${adminDetailText(value)}`);
        }
      }

      function adminPreview(label, value, limit = 1200) {
        const text = adminDetailText(value);
        if (text.length <= limit) return text;
        logAdminDetail(label, value);
        return `${text.slice(0, limit)}\n\n[truncated in UI; see browser console for full ${label}]`;
      }

      const NEW_REPO_VALUE = "__new__";
      const REPO_URL_HELP = "repo must start with git@, ssh://, or https://; GitHub owner/repo shorthand is also accepted";
      const REPO_URL_PREFIX_PATTERN = /^(git@|ssh:\/\/|https:\/\/)/;
      const GITHUB_REPO_SHORTHAND_PATTERN = /^([A-Za-z0-9][A-Za-z0-9_.-]*)\/([A-Za-z0-9][A-Za-z0-9_.-]*?)(?:\.git)?$/;

      function normalizeRepoUrlInput(value) {
        const repo = value.trim();
        const shorthand = repo.match(GITHUB_REPO_SHORTHAND_PATTERN);
        if (!shorthand) return repo;
        return `https://github.com/${shorthand[1]}/${shorthand[2]}.git`;
      }

      function validateRepoUrlInput(value) {
        const repo = normalizeRepoUrlInput(value);
        if (!repo) return { repo: "", error: "git repo URL is required" };
        if (!REPO_URL_PREFIX_PATTERN.test(repo)) return { repo, error: REPO_URL_HELP };
        return { repo, error: "" };
      }

      const BUILTIN_GIT_REPOS = [
        { repoUrl: "https://github.com/ORESoftware/live-mutex.git", displayName: "ORESoftware/live-mutex", provider: "github", defaultBranch: "dev", status: "active" },
        { repoUrl: "https://github.com/benefactor-cc/benefactor-cc.github.io.git", displayName: "benefactor-cc/benefactor-cc.github.io", provider: "github", defaultBranch: "main", status: "active" },
        { repoUrl: "https://github.com/ORESoftware/k8s-cluster.git", displayName: "ORESoftware/k8s-cluster", provider: "github", defaultBranch: "main", status: "active" },
        { repoUrl: "https://github.com/ORESoftware/us-anti-corruption-court-project.git", displayName: "ORESoftware/us-anti-corruption-court-project", provider: "github", defaultBranch: "main", status: "active" },
        { repoUrl: "https://github.com/dancing-dragons/dd-next-1.git", displayName: "dancing-dragons/dd-next-1", provider: "github", defaultBranch: "dev", status: "active" },
      ];

      function repoMergeKey(repoUrl) {
        const normalized = normalizeRepoUrlInput(repoUrl || "").replace(/\.git$/i, "");
        const githubSsh = normalized.match(/^git@github\.com:([^/]+\/[^/]+)$/i);
        if (githubSsh) return `github:${githubSsh[1].toLowerCase()}`;
        const githubHttps = normalized.match(/^https:\/\/github\.com\/([^/]+\/[^/]+)$/i);
        if (githubHttps) return `github:${githubHttps[1].toLowerCase()}`;
        return normalized.toLowerCase();
      }

      function mergeKnownRepos(builtinRepos, storedRepos) {
        const merged = new Map();
        for (const repo of [...builtinRepos, ...(storedRepos || [])]) {
          const repoUrl = normalizeRepoUrlInput(repo.repoUrl || "");
          if (!repoUrl) continue;
          const key = repoMergeKey(repoUrl);
          const existing = merged.get(key) || {};
          merged.set(key, {
            ...existing,
            ...repo,
            repoUrl,
            displayName: repo.displayName || existing.displayName || repoUrl,
            defaultBranch: repo.defaultBranch || existing.defaultBranch || "dev",
            provider: repo.provider || existing.provider || "github",
            status: repo.status || existing.status || "active",
          });
        }
        return [...merged.values()];
      }

      async function fetchPgKnownRepos() {
        const response = await fetch("/api/agents/git-repos?limit=100", { cache: "no-store" });
        if (!response.ok) throw new Error(`known repos failed ${response.status}: ${await response.text()}`);
        const data = await response.json();
        return data.repos || [];
      }

      function loadMergedKnownRepos() {
        if (!window.rxjs) {
          return fetchPgKnownRepos()
            .catch(() => [])
            .then((storedRepos) => mergeKnownRepos(BUILTIN_GIT_REPOS, storedRepos));
        }
        const { combineLatest, from, of } = window.rxjs;
        const { catchError, map } = window.rxjs.operators || window.rxjs;
        return new Promise((resolve) => {
          combineLatest([
            of(BUILTIN_GIT_REPOS),
            from(fetchPgKnownRepos()).pipe(catchError(() => of([]))),
          ])
            .pipe(map(([builtinRepos, storedRepos]) => mergeKnownRepos(builtinRepos, storedRepos)))
            .subscribe(resolve);
        });
      }

      function currentRepoRawValue() {
        const selected = $("repo-url").value.trim();
        return selected === NEW_REPO_VALUE ? $("repo-url-new").value.trim() : selected;
      }

      function currentRepoUrl() {
        return validateRepoUrlInput(currentRepoRawValue()).repo;
      }

      function validateCurrentRepoUrl() {
        const selected = $("repo-url").value;
        const input = selected === NEW_REPO_VALUE ? $("repo-url-new") : $("repo-url");
        const rawRepo = currentRepoRawValue();
        const validation = validateRepoUrlInput(rawRepo);
        input.setCustomValidity(validation.error || "");
        if (!validation.error && selected === NEW_REPO_VALUE && rawRepo && rawRepo !== validation.repo) {
          $("repo-url-new").value = validation.repo;
        }
        return validation;
      }

      function validateRepoUrlField() {
        if ($("repo-url").value !== NEW_REPO_VALUE) return true;
        const input = $("repo-url-new");
        if (!input.value.trim()) {
          input.setCustomValidity("");
          return true;
        }
        const validation = validateCurrentRepoUrl();
        if (validation.error) setStatus(validation.error, true);
        return !validation.error;
      }

      function currentBaseBranch() {
        return $("base-branch").value.trim() || "dev";
      }

      function optionLabel(repo) {
        return `${repo.displayName || repo.repoUrl} (${repo.defaultBranch || "dev"})`;
      }

      function updateRepoUrlMode() {
        const selected = $("repo-url").value;
        const isNew = selected === NEW_REPO_VALUE;
        $("repo-url").setCustomValidity("");
        $("repo-url-new-row").hidden = !isNew;
        if (!isNew) $("repo-url-new").setCustomValidity("");
        if (!isNew) {
          const repo = state.knownRepos.find((item) => item.repoUrl === selected);
          if (repo?.defaultBranch) $("base-branch").value = repo.defaultBranch;
        }
      }

      function setRepoSelection(repoUrl) {
        if (!repoUrl) {
          $("repo-url").value = "";
          updateRepoUrlMode();
          return;
        }
        const known = state.knownRepos.some((repo) => repo.repoUrl === repoUrl);
        if (known) {
          $("repo-url").value = repoUrl;
        } else {
          $("repo-url").value = NEW_REPO_VALUE;
          $("repo-url-new").value = repoUrl;
        }
        updateRepoUrlMode();
      }

      function renderKnownRepos() {
        const select = $("repo-url");
        const selected = currentRepoUrl();
        select.textContent = "";
        const placeholder = document.createElement("option");
        placeholder.value = "";
        placeholder.textContent = "Select a repo";
        select.appendChild(placeholder);
        for (const repo of state.knownRepos) {
          const option = document.createElement("option");
          option.value = repo.repoUrl;
          option.textContent = optionLabel(repo);
          select.appendChild(option);
        }
        const newOption = document.createElement("option");
        newOption.value = NEW_REPO_VALUE;
        newOption.textContent = "New repo URL...";
        select.appendChild(newOption);
        setRepoSelection(selected);
      }

      async function loadKnownRepos() {
        state.knownRepos = await loadMergedKnownRepos();
        renderKnownRepos();
      }

      async function saveKnownRepo() {
        const repoValidation = validateCurrentRepoUrl();
        if (repoValidation.error) {
          setStatus(repoValidation.error, true);
          return;
        }
        const repoUrl = repoValidation.repo;
        const response = await fetch("/api/agents/git-repos", {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({
            repoUrl,
            defaultBranch: currentBaseBranch(),
          }),
        });
        const body = await response.text();
        if (!response.ok) {
          setStatus(`repo URL save failed ${response.status}: ${adminPreview("repo URL save response body", body, 240)}`, true);
          return;
        }
        setStatus("repo URL saved");
        await loadKnownRepos();
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

      function existingThread(threadId) {
        return state.threads.find((item) => item.id === threadId) || null;
      }

      function existingTask(taskId) {
        return state.tasks.find((item) => item.id === taskId) || null;
      }

      function updateThreadMode() {
        const threadId = $("thread-id").value.trim() || state.selectedThreadId || "";
        const mode = $("thread-mode");
        const subtitle = $("thread-control-subtitle");
        if (!threadId) {
          setThreadUiMode("empty");
          mode.textContent = "select thread";
          mode.className = "pill warn";
          subtitle.textContent = "Select an existing worker thread or prepare a new one.";
          return;
        }
        if (existingThread(threadId)) {
          setThreadUiMode("existing");
          mode.textContent = "viewing existing";
          mode.className = "pill";
          subtitle.textContent = "Viewing an existing worker. Pick a previous task below, send another task, or open the inline terminal.";
          return;
        }
        setThreadUiMode("new");
        mode.textContent = "creating new";
        mode.className = "pill warn";
        subtitle.textContent = "Creating a new worker. Repo, branch, provider, and prompt are used for the first task.";
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
        const thread = existingThread(state.selectedThreadId);
        const creating = Boolean(state.selectedThreadId && !thread);
        $("selected-title").textContent = thread?.title || (creating ? "New thread" : "Select a thread");
        $("selected-subtitle").textContent = state.selectedThreadId
          ? `${state.selectedThreadId} · ${creating ? "not created yet" : `${threadTasks(state.selectedThreadId).length} tasks`}`
          : "Pick a thread from the sidebar or start a new one.";
        $("thread-id").value = state.selectedThreadId || "";
        if (thread?.repo) setRepoSelection(thread.repo);
        if (thread?.baseBranch) $("base-branch").value = thread.baseBranch;
        if (!state.selectedTaskId) $("task-id").value = makeUuid();
        updateThreadMode();
      }

      function selectThread(threadId) {
        state.selectedThreadId = threadId;
        const tasks = threadTasks(threadId);
        state.selectedTaskId = tasks[0]?.id || null;
        closeInlineTerminal();
        setTaskStreamLayout("stream");
        replaceSelectionUrl(threadId, state.selectedTaskId);
        renderThreads();
        updateSelectionHeader();
        renderTaskList();
        if (state.selectedThreadId && existingThread(state.selectedThreadId)) setWorkspaceLayout("lower");
        if (state.selectedTaskId) {
          $("task-id").value = state.selectedTaskId;
          loadTaskEvents(state.selectedTaskId).catch((error) => renderError(`events load failed: ${adminPreview("events load error", error)}`, error, "events load error"));
        } else {
          clearStream("No task selected.");
        }
      }

      function selectTask(taskId) {
        state.selectedTaskId = taskId;
        $("task-id").value = taskId;
        closeInlineTerminal();
        setTaskStreamLayout("stream");
        replaceSelectionUrl(state.selectedThreadId, taskId);
        renderTaskList();
        loadTaskEvents(taskId).catch((error) => renderError(`events load failed: ${adminPreview("events load error", error)}`, error, "events load error"));
      }

      function terminalIsOpen() {
        return !$("terminal-inline").hidden;
      }

      function openInlineTerminal(targetUrl) {
        $("terminal-caption").textContent = targetUrl;
        $("terminal-frame").src = targetUrl;
        $("terminal-inline").hidden = false;
        $("response-stream-panel").classList.add("terminal-open");
        setTaskStreamLayout("stream");
        setStreamState("terminal open", "ok");
      }

      function closeInlineTerminal() {
        if (!terminalIsOpen()) return;
        $("terminal-frame").src = "about:blank";
        $("terminal-inline").hidden = true;
        $("response-stream-panel").classList.remove("terminal-open");
        setStreamState(state.selectedTaskId ? "showing events" : "no task selected", state.selectedTaskId ? "ok" : "warn");
      }

      function clearStream(message, taskId = state.selectedTaskId) {
        state.renderedEvents.clear();
        state.streamTaskId = taskId || null;
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

      function renderError(message, detail = null, label = "error") {
        if (detail !== null) logAdminDetail(label, detail);
        renderEventRow({
          seq: `error-${Date.now()}`,
          eventKind: "error",
          payload: { kind: "error", message: adminPreview(label, message) },
          createdAt: new Date().toISOString(),
        });
      }

      function scheduleSnapshotRetry(options = {}) {
        if (state.snapshotRetryTimer !== null) return;
        const delay = Math.min(30000, 2000 * Math.max(1, state.snapshotFailures));
        state.snapshotRetryTimer = window.setTimeout(() => {
          state.snapshotRetryTimer = null;
          loadSnapshot({ ...options, fromRetry: true }).catch((error) => handleSnapshotError(error, options));
        }, delay);
      }

      async function readableFetchError(response, label) {
        const body = await response.text();
        const contentType = response.headers.get("content-type") || "";
        if (contentType.includes("text/html") || /^\s*</.test(body)) {
          return `${label} failed ${response.status}: gateway returned HTML; retrying`;
        }
        return `${label} failed ${response.status}: ${adminPreview(label, body, 240)}`;
      }

      function handleSnapshotError(error, options = {}) {
        state.snapshotFailures += 1;
        logAdminDetail("snapshot load error", error);
        const hasSnapshot = Boolean(state.snapshot || state.threads.length || state.tasks.length);
        const summary = hasSnapshot
          ? `${state.threads.length} threads · ${state.tasks.length} tasks · snapshot retrying`
          : "snapshot unavailable · retrying";
        $("snapshot-meta").textContent = summary;
        setStatus(adminPreview("snapshot temporarily unavailable; retrying", error, 180), true);
        scheduleSnapshotRetry(options);
      }

      function renderRealtimePayload(raw, source = "ws") {
        let parsed = raw;
        try { parsed = JSON.parse(raw); } catch (_error) {}
        if (parsed && typeof parsed === "object" && parsed.type === "task-event") {
          if (parsed.threadId && parsed.threadId !== state.selectedThreadId) return;
          if (parsed.taskId && parsed.taskId !== state.selectedTaskId) return;
          renderEventRow({
            messageId: parsed.messageId || parsed.message_id || parsed.id,
            seq: parsed.seq ?? `${source}-${Date.now()}`,
            eventKind: parsed.event?.kind || "message",
            payload: parsed.event || parsed,
            createdAt: parsed.emittedAt || new Date().toISOString(),
          });
        }
      }

      function renderEventRow(row) {
        state.streamTaskId = state.selectedTaskId || state.streamTaskId;
        const seq = row.seq ?? row.payload?.seq ?? Date.now();
        const kind = eventKind(row);
        const stableSeq = row.seq ?? row.payload?.seq;
        const key = stableSeq !== undefined && stableSeq !== null
          ? `${state.selectedTaskId || row.taskId || "task"}:${stableSeq}:${kind}`
          : row.messageId || row.payload?.messageId || `${state.selectedTaskId || "task"}:${seq}:${kind}`;
        if (state.renderedEvents.has(key)) return;
        state.renderedEvents.add(key);
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
        const unscheduled = pods.map((pod) => ({
          pod: pod.name,
          condition: (pod.conditions || []).find((condition) => condition.type === "PodScheduled" && condition.status === "False"),
        })).find((item) => item.condition);
        if (unscheduled) {
          const reason = unscheduled.condition.reason || "unscheduled";
          const message = unscheduled.condition.message || "scheduler has not placed this pod yet";
          if (/too many pods/i.test(message)) {
            return `worker pending: node pod-slot limit full; ${unscheduled.pod} ${reason}: ${message}`;
          }
          if (/insufficient cpu/i.test(message)) {
            return `worker pending: node CPU capacity full; ${unscheduled.pod} ${reason}: ${message}`;
          }
          return `worker pending: ${unscheduled.pod} ${reason}: ${message}`;
        }
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

      function workerRuntimeWaitDetails(data) {
        if (!data) return "runtime snapshot pending";
        const summary = data.summary || {};
        const pods = data.pods || [];
        const lines = [
          `runtime phase=${summary.phase || "unknown"} desired=${summary.desiredReplicas ?? "?"} readyReplicas=${summary.readyReplicas ?? 0} pods=${summary.readyPodCount ?? 0}/${summary.podCount ?? pods.length}`,
        ];
        if (data.errors?.length) lines.push(`runtime error=${data.errors[0]}`);
        for (const pod of pods.slice(0, 3)) {
          const unscheduled = (pod.conditions || []).find((condition) => condition.type === "PodScheduled" && condition.status === "False");
          if (unscheduled) {
            lines.push(`${pod.name}: ${unscheduled.reason || "Unscheduled"}: ${unscheduled.message || "scheduler has not placed this pod yet"}`);
            continue;
          }
          const waiting = [...(pod.initContainers || []), ...(pod.containers || [])].find((container) => container.state?.waiting);
          if (waiting) {
            lines.push(`${pod.name}/${waiting.name}: waiting ${waiting.state.waiting.reason || "unknown"}${waiting.state.waiting.message ? `: ${waiting.state.waiting.message}` : ""}`);
            continue;
          }
          const unready = (pod.containers || []).find((container) => !container.ready);
          if (unready) {
            const running = unready.state?.running?.startedAt ? ` running since ${unready.state.running.startedAt}` : "";
            lines.push(`${pod.name}/${unready.name}: not ready${running}; restarts=${unready.restartCount || 0}`);
            continue;
          }
          if (pod.name) lines.push(`${pod.name}: ${pod.phase || "unknown"} ready`);
        }
        return lines.join("\n");
      }

      async function loadRuntimeState(threadId, render = true) {
        if (!threadId) return null;
        const response = await fetch(`/api/agents/threads/${encodeURIComponent(threadId)}/runtime`, { cache: "no-store" });
        if (!response.ok) throw new Error(`runtime request failed ${response.status}: ${await response.text()}`);
        const data = await response.json();
        const summary = workerRuntimeSummary(data);
        state.lastRuntimeData = data;
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
        loadRuntimeState(threadId).catch((error) => setStatus(adminPreview("runtime state error", error, 240), true));
        state.runtimePoll = setInterval(() => {
          loadRuntimeState(threadId).catch((error) => setStatus(adminPreview("runtime state error", error, 240), true));
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
          renderError(`feedback failed ${response.status}: ${adminPreview("feedback response body", await response.text())}`);
          return;
        }
        button.textContent = vote === "up" ? "ok" : "noted";
        const data = await response.json().catch(() => null);
        if (data?.event) renderEventRow(data.event);
      }

      async function loadTaskEvents(taskId, options = {}) {
        const response = await fetch(`/api/agents/tasks/${encodeURIComponent(taskId)}/events?limit=250`, { cache: "no-store" });
        if (!response.ok) throw new Error(`events request failed ${response.status}: ${await response.text()}`);
        const data = await response.json();
        if (data.errors?.length) renderError(data.errors.join("\n"));
        if (!data.events?.length) {
          if (options.preserveCurrentOnEmpty && state.streamTaskId === taskId && $("stream").childElementCount > 0) {
            setStreamState("showing live status", "ok");
            return;
          }
          clearStream("no stored events", taskId);
          setStreamState("no stored events yet", "warn");
          const empty = document.createElement("p");
          empty.className = "muted";
          empty.textContent = "No stored response events for this task yet.";
          $("stream").appendChild(empty);
          return;
        }
        if (options.appendOnly) {
          state.streamTaskId = taskId;
        } else {
          clearStream("loading events", taskId);
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

      function openGleamLiveSocket(threadId, taskId) {
        if (state.liveWs) state.liveWs.close();
        const proto = location.protocol === "https:" ? "wss" : "ws";
        const wsUrl = `${proto}://${location.host}/gleam/ws?threadId=${encodeURIComponent(threadId)}&taskId=${encodeURIComponent(taskId)}`;
        const ws = new WebSocket(wsUrl);
        state.liveWs = ws;
        ws.onopen = () => {
          setStreamState("websocket connected", "ok");
          ws.send(JSON.stringify({ type: "subscribe", threadId, taskId }));
        };
        ws.onmessage = (event) => renderRealtimePayload(event.data, "gleam-ws");
        ws.onerror = () => setStreamState("websocket error", "bad");
        ws.onclose = () => {
          if (state.liveWs === ws) state.liveWs = null;
        };
      }

      async function loadSnapshot(options = {}) {
        const response = await fetch("/api/agents/tasks?limit=200", { cache: "no-store" });
        if (!response.ok) throw new Error(await readableFetchError(response, "snapshot"));
        const data = await response.json();
        state.snapshotFailures = 0;
        if (state.snapshotRetryTimer !== null) {
          window.clearTimeout(state.snapshotRetryTimer);
          state.snapshotRetryTimer = null;
        }
        state.snapshot = data;
        state.threads = data.threads || [];
        state.tasks = data.tasks || [];
        $("snapshot-meta").textContent = `${state.threads.length} threads · ${state.tasks.length} tasks · ${data.source || "unknown"}`;
        const params = new URLSearchParams(window.location.search);
        const requestedThread = queryUuid(params, "thread");
        const requestedTask = queryUuid(params, "task");
        if (requestedThread) {
          state.selectedThreadId = requestedThread;
        }
        if (!state.selectedThreadId && state.threads.length) state.selectedThreadId = state.threads[0].id;
        if (requestedTask && state.tasks.some((task) => task.id === requestedTask)) state.selectedTaskId = requestedTask;
        if (!state.selectedTaskId && state.selectedThreadId) state.selectedTaskId = threadTasks(state.selectedThreadId)[0]?.id || null;
        renderThreads();
        updateSelectionHeader();
        renderTaskList();
        setWorkspaceLayout(state.selectedThreadId && existingThread(state.selectedThreadId) ? "lower" : "control");
        if (state.selectedTaskId) {
          $("task-id").value = state.selectedTaskId;
          if (options.preserveStreamForTask !== state.selectedTaskId) {
            await loadTaskEvents(state.selectedTaskId, {
              preserveCurrentOnEmpty: state.streamTaskId === state.selectedTaskId,
            });
          }
        }
      }

      async function dispatchPrompt() {
        const threadId = readUuidInput("thread-id", "thread UUID", { generate: true });
        let taskId = readUuidInput("task-id", "task UUID", { generate: true });
        const prompt = $("prompt").value.trim();
        const provider = $("provider").value;
        const dispatchMode = $("dispatch-mode").value;
        const usesContainerPool = dispatchMode === "queued-pool";
        const repoValidation = validateCurrentRepoUrl();
        const repo = repoValidation.repo;
        const baseBranch = currentBaseBranch();
        if (!threadId || !taskId) return;
        if (!prompt) {
          setStatus("prompt is required", true);
          return;
        }
        if (repoValidation.error) {
          setStatus(repoValidation.error, true);
          return;
        }
        const taskAlreadyExists = existingTask(taskId);
        if (taskAlreadyExists) {
          if (taskAlreadyExists.threadId !== threadId) {
            setStatus("task UUID already belongs to a different thread", true);
            return;
          }
          taskId = makeUuid();
          $("task-id").value = taskId;
        }
        state.selectedThreadId = threadId;
        state.selectedTaskId = taskId;
        closeInlineTerminal();
        setTaskStreamLayout("stream");
        replaceSelectionUrl(threadId, taskId);
        clearStream(usesContainerPool ? "waiting for queue" : "waking worker");
        openGleamLiveSocket(threadId, taskId);
        if (!usesContainerPool) startRuntimePolling(threadId);
        renderEventRow({
          seq: `dispatch-start-${Date.now()}`,
          eventKind: "status",
          payload: {
            kind: "status",
            status: usesContainerPool ? "queueing container-pool task" : "waking worker",
            message: usesContainerPool
              ? "Publishing the task to NATS for the queue consumer to dispatch through container-pool using this thread UUID as the affinity key."
              : "Creating or waking the UUID-bound worker. Cold starts can take 30-90 seconds while the container installs dependencies, refreshes git, and starts Node.",
          },
          createdAt: new Date().toISOString(),
        });
        setStatus(`POST /api/agents/threads/${threadId}/tasks`);
        const startedAt = Date.now();
        const keepRuntimePolling = dispatchMode === "queued";
        const waitTicker = usesContainerPool ? null : setInterval(() => {
            const elapsed = Math.round((Date.now() - startedAt) / 1000);
            const runtimeSummary = state.lastRuntimeSummary || "runtime snapshot pending";
            const runtimeDetails = workerRuntimeWaitDetails(state.lastRuntimeData);
            setStatus(`dispatch waiting ${elapsed}s · ${runtimeSummary}`);
            renderEventRow({
              seq: `dispatch-wait-${elapsed}`,
              eventKind: "status",
              payload: {
                kind: "status",
                status: `still waiting (${elapsed}s)`,
                message: [
                  "The REST API is waiting for the thread worker readiness check before it forwards the task.",
                  runtimeSummary,
                  runtimeDetails,
                ].filter(Boolean).join("\n"),
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
              repo,
              baseBranch,
              dispatchMode,
              threadTitle: prompt.slice(0, 80),
            }),
          });
        } finally {
          if (waitTicker !== null) clearInterval(waitTicker);
          if (!keepRuntimePolling) stopRuntimePolling();
        }
        const body = await response.text();
        if (!response.ok) {
          renderError(
            `dispatch failed ${response.status}: ${adminPreview("dispatch response body", body)}`,
            body,
            "dispatch response body",
          );
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
            message: adminPreview("dispatch accepted response body", body),
          },
          createdAt: new Date().toISOString(),
        });
        if (!usesContainerPool) {
          await loadRuntimeState(threadId).catch((error) => setStatus(adminPreview("runtime state error", error, 240), true));
        }
        openLiveStream(threadId, taskId);
        await loadSnapshot({ preserveStreamForTask: taskId }).catch((error) => handleSnapshotError(error, { preserveStreamForTask: taskId }));
      }

      async function threadControl(action) {
        if (!$("thread-id").value.trim()) {
          setStatus("thread id is required", true);
          return;
        }
        const threadId = readUuidInput("thread-id", "thread UUID");
        if (threadId === null) return;
        const taskId = readUuidInput("task-id", "task UUID", { generate: true });
        if (!taskId) return;
        const routeActions = {
          delete: "hard-delete",
          merge: "merge-upstream",
          commit: "make-commit",
          terminal: "terminal",
          "open-pr": "open-pr",
        };
        const routeAction = routeActions[action] || action;
        if (["hard-delete", "merge-upstream", "make-commit", "open-pr", "terminal", "sleep", "archive"].includes(routeAction) && !existingThread(threadId)) {
          setStatus(`${routeAction} is available after this thread has been created`, true);
          return;
        }
        const pollRuntime = routeAction === "terminal";
        if (pollRuntime) {
          closeInlineTerminal();
          setTaskStreamLayout("stream");
          clearStream("waking terminal");
          renderEventRow({
            seq: `terminal-start-${Date.now()}`,
            eventKind: "status",
            payload: {
              kind: "status",
              status: "waking terminal",
              message: "Waking the selected worker and opening its shell inside the response panel.",
            },
            createdAt: new Date().toISOString(),
          });
          startRuntimePolling(threadId);
        }
        let response;
        try {
          response = await fetch(`/api/agents/threads/${encodeURIComponent(threadId)}/${routeAction}`, {
            method: "POST",
            headers: { "content-type": "application/json" },
            body: JSON.stringify({
              kind: "thread-control",
              action: routeAction,
              threadId,
              taskId,
              requestedBy: "agents-threads-ui",
              reason: routeAction === "make-commit" ? "manual commit" : routeAction,
            }),
          });
        } finally {
          if (pollRuntime) stopRuntimePolling();
        }
        const body = await response.text();
        const visibleBody = adminPreview(`${routeAction} response body`, body);
        renderEventRow({
          seq: `control-${Date.now()}`,
          eventKind: response.ok ? "status" : "error",
          payload: {
            kind: response.ok ? "status" : "error",
            status: `${routeAction} ${response.status}`,
            message: visibleBody,
          },
          createdAt: new Date().toISOString(),
        });
        if (!response.ok) {
          logAdminDetail(`${routeAction} response body`, body);
          setStatus(`${routeAction} failed`, true);
        } else {
          setStatus(`${routeAction} accepted`);
          let terminalTargetUrl = null;
          if (routeAction === "terminal") {
            terminalTargetUrl = terminalUrlFromControlResponse(threadId, body);
          }
          await loadSnapshot().catch((error) => handleSnapshotError(error));
          if (terminalTargetUrl) openInlineTerminal(terminalTargetUrl);
        }
      }

      $("refresh").addEventListener("click", () => {
        loadKnownRepos().catch((error) => setStatus(adminPreview("known repos load error", error, 240), true));
        loadSnapshot().catch((error) => handleSnapshotError(error));
      });
      $("save-repo").addEventListener("click", () => saveKnownRepo().catch((error) => setStatus(adminPreview("repo save error", error, 240), true)));
      $("repo-url").addEventListener("change", updateRepoUrlMode);
      $("repo-url-new").addEventListener("blur", validateRepoUrlField);
      $("repo-url-new").addEventListener("input", () => $("repo-url-new").setCustomValidity(""));
      $("thread-control-panel").addEventListener("click", handleControlPanelClick);
      $("thread-control-panel").addEventListener("keydown", handleControlPanelKey);
      $("previous-tasks-panel").addEventListener("click", (event) => handleLowerPanelClick(event, "tasks"));
      $("previous-tasks-panel").addEventListener("keydown", (event) => handlePanelKey(event, "tasks"));
      $("response-stream-panel").addEventListener("click", (event) => handleLowerPanelClick(event, "stream"));
      $("response-stream-panel").addEventListener("keydown", (event) => handlePanelKey(event, "stream"));
      $("terminal-close").addEventListener("click", (event) => {
        event.stopPropagation();
        closeInlineTerminal();
      });
      $("new-thread").addEventListener("click", () => {
        state.selectedThreadId = makeUuid();
        state.selectedTaskId = null;
        closeInlineTerminal();
        setWorkspaceLayout("control");
        $("thread-id").value = state.selectedThreadId;
        $("task-id").value = makeUuid();
        replaceSelectionUrl(state.selectedThreadId, null);
        $("prompt").focus();
        updateSelectionHeader();
        renderTaskList();
        clearStream("new thread ready");
      });
      $("new-task").addEventListener("click", () => {
        state.selectedTaskId = null;
        closeInlineTerminal();
        setWorkspaceLayout("control");
        $("task-id").value = makeUuid();
        replaceSelectionUrl(state.selectedThreadId, null);
        clearStream("new task ready");
      });
      $("thread-id").addEventListener("input", () => {
        $("thread-id").setCustomValidity("");
        updateThreadMode();
      });
      $("thread-id").addEventListener("change", () => {
        const threadId = readUuidInput("thread-id", "thread UUID", { allowEmpty: true });
        if (threadId === null) return;
        state.selectedThreadId = threadId || null;
        replaceSelectionUrl(state.selectedThreadId, state.selectedThreadId ? state.selectedTaskId : null);
        updateSelectionHeader();
        renderThreads();
        renderTaskList();
      });
      $("task-id").addEventListener("change", () => {
        const taskId = readUuidInput("task-id", "task UUID", { allowEmpty: true });
        if (taskId === null) return;
        state.selectedTaskId = taskId || null;
        replaceSelectionUrl(state.selectedThreadId, state.selectedTaskId);
      });
      $("send").addEventListener("click", () => dispatchPrompt().catch((error) => renderError(`dispatch error: ${adminPreview("dispatch exception", error)}`, error, "dispatch exception")));
      $("sleep-thread").addEventListener("click", () => threadControl("sleep").catch((error) => renderError(adminPreview("sleep exception", error), error, "sleep exception")));
      $("archive-thread").addEventListener("click", () => threadControl("archive").catch((error) => renderError(adminPreview("archive exception", error), error, "archive exception")));
      $("delete-thread").addEventListener("click", () => threadControl("delete").catch((error) => renderError(adminPreview("delete exception", error), error, "delete exception")));
      $("merge-thread").addEventListener("click", () => threadControl("merge").catch((error) => renderError(adminPreview("merge exception", error), error, "merge exception")));
      $("commit-thread").addEventListener("click", () => threadControl("commit").catch((error) => renderError(adminPreview("commit exception", error), error, "commit exception")));
      $("open-pr-thread").addEventListener("click", () => threadControl("open-pr").catch((error) => renderError(adminPreview("open-pr exception", error), error, "open-pr exception")));
      $("terminal-thread").addEventListener("click", () => threadControl("terminal").catch((error) => renderError(adminPreview("terminal exception", error), error, "terminal exception")));

      loadKnownRepos().catch((error) => setStatus(adminPreview("known repos load error", error, 240), true));
      loadSnapshot().catch((error) => handleSnapshotError(error));
      setInterval(() => {
        if (!state.selectedTaskId) return;
        loadSnapshot({ preserveStreamForTask: state.selectedTaskId }).catch((error) => handleSnapshotError(error, { preserveStreamForTask: state.selectedTaskId }));
        loadTaskEvents(state.selectedTaskId, {
          preserveCurrentOnEmpty: true,
          appendOnly: true,
        }).catch((error) => setStatus(adminPreview("events poll error", error, 240), true));
      }, 10000);
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
      input:invalid, select:invalid {
        border-color: rgba(248, 113, 113, 0.7);
        box-shadow: 0 0 0 1px rgba(248, 113, 113, 0.18);
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
      const threadTerminalUrl = (threadId) => `${threadIngressPrefix(threadId)}/terminal?threadId=${encodeURIComponent(threadId)}`;
      const normalizeThreadId = (threadId) => String(threadId || "").trim().toLowerCase();
      const trustedThreadTerminalUrl = (threadId, candidate) => {
        const fallback = threadTerminalUrl(threadId);
        if (!candidate) return fallback;
        try {
          const parsed = new URL(String(candidate), window.location.origin);
          const expectedPath = `${threadIngressPrefix(threadId)}/terminal`;
          const returnedThreadId = normalizeThreadId(parsed.searchParams.get("threadId"));
          if (parsed.origin !== window.location.origin || parsed.pathname !== expectedPath || returnedThreadId !== normalizeThreadId(threadId)) {
            throw new Error("unexpected terminal URL");
          }
          return `${parsed.pathname}${parsed.search}`;
        } catch {
          appendStreamLine("ignored unsafe terminal URL from control response");
          return fallback;
        }
      };
      const threadTerminalUrlFromControlResponse = (threadId, body) => {
        try {
          const parsed = JSON.parse(body);
          return trustedThreadTerminalUrl(threadId, parsed.terminalUrl);
        } catch {
          return threadTerminalUrl(threadId);
        }
      };
      let activeStream = null;
      let activeWs = null;
      const workerSockets = new Map();
      let activeTaskKey = null;
      let seenStreamEvents = new Set();
      let knownRepos = [];
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
      const adminDetailText = (value) => {
        if (value instanceof Error) return value.stack || `${value.name}: ${value.message}`;
        if (typeof value === "string") return value;
        try { return JSON.stringify(value, null, 2); } catch (_error) { return String(value); }
      };
      const logAdminDetail = (label, value) => {
        try { console.error(`[agents admin] ${label}`, value); }
        catch (_error) { console.error(`[agents admin] ${label}: ${adminDetailText(value)}`); }
      };
      const adminPreview = (label, value, limit = 1200) => {
        const textValue = adminDetailText(value);
        if (textValue.length <= limit) return textValue;
        logAdminDetail(label, value);
        return `${textValue.slice(0, limit)}\n\n[truncated in UI; see browser console for full ${label}]`;
      };
      const setChatRoute = () => {
        const threadId = $("chat-thread-id").value.trim();
        $("chat-route").textContent = threadId ? `/api/agents/threads/${threadId}/tasks` : "";
        updateThreadRuntimeControls();
      };
      const NEW_REPO_VALUE = "__new__";
      const REPO_URL_HELP = "repo must start with git@, ssh://, or https://; GitHub owner/repo shorthand is also accepted";
      const REPO_URL_PREFIX_PATTERN = /^(git@|ssh:\/\/|https:\/\/)/;
      const GITHUB_REPO_SHORTHAND_PATTERN = /^([A-Za-z0-9][A-Za-z0-9_.-]*)\/([A-Za-z0-9][A-Za-z0-9_.-]*?)(?:\.git)?$/;
      const normalizeRepoUrlInput = (value) => {
        const repo = value.trim();
        const shorthand = repo.match(GITHUB_REPO_SHORTHAND_PATTERN);
        if (!shorthand) return repo;
        return `https://github.com/${shorthand[1]}/${shorthand[2]}.git`;
      };
      const validateRepoUrlInput = (value) => {
        const repo = normalizeRepoUrlInput(value);
        if (!repo) return { repo: "", error: "git repo URL is required" };
        if (!REPO_URL_PREFIX_PATTERN.test(repo)) return { repo, error: REPO_URL_HELP };
        return { repo, error: "" };
      };
      const BUILTIN_GIT_REPOS = [
        { repoUrl: "https://github.com/ORESoftware/live-mutex.git", displayName: "ORESoftware/live-mutex", provider: "github", defaultBranch: "dev", status: "active" },
        { repoUrl: "https://github.com/benefactor-cc/benefactor-cc.github.io.git", displayName: "benefactor-cc/benefactor-cc.github.io", provider: "github", defaultBranch: "main", status: "active" },
        { repoUrl: "https://github.com/ORESoftware/k8s-cluster.git", displayName: "ORESoftware/k8s-cluster", provider: "github", defaultBranch: "main", status: "active" },
        { repoUrl: "https://github.com/ORESoftware/us-anti-corruption-court-project.git", displayName: "ORESoftware/us-anti-corruption-court-project", provider: "github", defaultBranch: "main", status: "active" },
        { repoUrl: "https://github.com/dancing-dragons/dd-next-1.git", displayName: "dancing-dragons/dd-next-1", provider: "github", defaultBranch: "dev", status: "active" },
      ];
      const repoMergeKey = (repoUrl) => {
        const normalized = normalizeRepoUrlInput(repoUrl || "").replace(/\.git$/i, "");
        const githubSsh = normalized.match(/^git@github\.com:([^/]+\/[^/]+)$/i);
        if (githubSsh) return `github:${githubSsh[1].toLowerCase()}`;
        const githubHttps = normalized.match(/^https:\/\/github\.com\/([^/]+\/[^/]+)$/i);
        if (githubHttps) return `github:${githubHttps[1].toLowerCase()}`;
        return normalized.toLowerCase();
      };
      const mergeKnownRepos = (builtinRepos, storedRepos) => {
        const merged = new Map();
        for (const repo of [...builtinRepos, ...(storedRepos || [])]) {
          const repoUrl = normalizeRepoUrlInput(repo.repoUrl || "");
          if (!repoUrl) continue;
          const key = repoMergeKey(repoUrl);
          const existing = merged.get(key) || {};
          merged.set(key, {
            ...existing,
            ...repo,
            repoUrl,
            displayName: repo.displayName || existing.displayName || repoUrl,
            defaultBranch: repo.defaultBranch || existing.defaultBranch || "dev",
            provider: repo.provider || existing.provider || "github",
            status: repo.status || existing.status || "active",
          });
        }
        return [...merged.values()];
      };
      const fetchPgKnownRepos = async () => {
        const response = await fetch("/api/agents/git-repos?limit=100", { cache: "no-store" });
        if (!response.ok) throw new Error(`known repos request failed (${response.status}): ${await response.text()}`);
        const data = await response.json();
        return data.repos || [];
      };
      const loadMergedKnownRepos = () => {
        if (!window.rxjs) {
          return fetchPgKnownRepos()
            .catch(() => [])
            .then((storedRepos) => mergeKnownRepos(BUILTIN_GIT_REPOS, storedRepos));
        }
        const { combineLatest, from, of } = window.rxjs;
        const { catchError, map } = window.rxjs.operators || window.rxjs;
        return new Promise((resolve) => {
          combineLatest([
            of(BUILTIN_GIT_REPOS),
            from(fetchPgKnownRepos()).pipe(catchError(() => of([]))),
          ])
            .pipe(map(([builtinRepos, storedRepos]) => mergeKnownRepos(builtinRepos, storedRepos)))
            .subscribe(resolve);
        });
      };
      const currentChatRepoRawValue = () => {
        const selected = $("chat-repo-url").value.trim();
        return selected === NEW_REPO_VALUE ? $("chat-repo-url-new").value.trim() : selected;
      };
      const currentChatRepoUrl = () => {
        return validateRepoUrlInput(currentChatRepoRawValue()).repo;
      };
      const validateCurrentChatRepoUrl = () => {
        const selected = $("chat-repo-url").value;
        const input = selected === NEW_REPO_VALUE ? $("chat-repo-url-new") : $("chat-repo-url");
        const rawRepo = currentChatRepoRawValue();
        const validation = validateRepoUrlInput(rawRepo);
        input.setCustomValidity(validation.error || "");
        if (!validation.error && selected === NEW_REPO_VALUE && rawRepo && rawRepo !== validation.repo) {
          $("chat-repo-url-new").value = validation.repo;
        }
        return validation;
      };
      const validateChatRepoUrlField = () => {
        if ($("chat-repo-url").value !== NEW_REPO_VALUE) return true;
        const input = $("chat-repo-url-new");
        if (!input.value.trim()) {
          input.setCustomValidity("");
          return true;
        }
        return !validateCurrentChatRepoUrl().error;
      };
      const currentChatBaseBranch = () => $("chat-base-branch").value.trim() || "dev";
      const repoOptionLabel = (repo) => `${repo.displayName || repo.repoUrl} (${repo.defaultBranch || "dev"})`;
      const updateChatRepoUrlMode = () => {
        const selected = $("chat-repo-url").value;
        const isNew = selected === NEW_REPO_VALUE;
        $("chat-repo-url").setCustomValidity("");
        $("chat-repo-url-new-row").hidden = !isNew;
        if (!isNew) $("chat-repo-url-new").setCustomValidity("");
        if (!isNew) {
          const repo = knownRepos.find((item) => item.repoUrl === selected);
          if (repo?.defaultBranch) $("chat-base-branch").value = repo.defaultBranch;
        }
      };
      const setChatRepoSelection = (repoUrl) => {
        if (!repoUrl) {
          $("chat-repo-url").value = "";
          updateChatRepoUrlMode();
          return;
        }
        const known = knownRepos.some((repo) => repo.repoUrl === repoUrl);
        if (known) {
          $("chat-repo-url").value = repoUrl;
        } else {
          $("chat-repo-url").value = NEW_REPO_VALUE;
          $("chat-repo-url-new").value = repoUrl;
        }
        updateChatRepoUrlMode();
      };
      const renderKnownRepos = () => {
        const select = $("chat-repo-url");
        const selected = currentChatRepoUrl();
        select.textContent = "";
        const placeholder = document.createElement("option");
        placeholder.value = "";
        placeholder.textContent = "Select a repo";
        select.appendChild(placeholder);
        for (const repo of knownRepos) {
          const option = document.createElement("option");
          option.value = repo.repoUrl;
          option.textContent = repoOptionLabel(repo);
          select.appendChild(option);
        }
        const newOption = document.createElement("option");
        newOption.value = NEW_REPO_VALUE;
        newOption.textContent = "New repo URL...";
        select.appendChild(newOption);
        setChatRepoSelection(selected);
      };
      const loadKnownRepos = async () => {
        knownRepos = await loadMergedKnownRepos();
        renderKnownRepos();
      };
      const saveChatRepo = async () => {
        const repoValidation = validateCurrentChatRepoUrl();
        if (repoValidation.error) {
          appendStreamLine(repoValidation.error);
          return;
        }
        const repoUrl = repoValidation.repo;
        const response = await fetch("/api/agents/git-repos", {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({
            repoUrl,
            defaultBranch: currentChatBaseBranch()
          })
        });
        const body = await response.text();
        if (!response.ok) {
          appendStreamLine(`repo URL save failed ${response.status}: ${adminPreview("repo URL save response body", body)}`);
          return;
        }
        appendStreamLine(`repo URL saved ${adminPreview("repo URL save response body", body)}`);
        await loadKnownRepos();
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
        if (!response.ok) throw new Error(`runtime request failed ${response.status}: ${await response.text()}`);
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
          ? "Thread runtime is asleep/suspended. Merge will wake the worker, merge the configured base branch, and push."
          : "Merge the configured base branch into this thread branch and push.";
      }
      const resetRealtimeState = (threadId, taskId) => {
        activeTaskKey = `${threadId}:${taskId}`;
        seenStreamEvents = new Set();
      };
      const shouldRenderEvent = (source, threadId, taskId, seq, kind, messageId = null) => {
        if (activeTaskKey && `${threadId || ""}:${taskId || ""}` !== activeTaskKey) return false;
        const key = messageId || (seq === undefined || seq === null
          ? `${source}:${taskId || "none"}:no-seq:${kind}`
          : `${taskId || "none"}:${seq}:${kind}`);
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
          const messageId = parsed.messageId || parsed.message_id || parsed.id || null;
          if (!shouldRenderEvent(source, parsed.threadId, parsed.taskId, parsed.seq, event.kind || kind, messageId)) return;
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
        const repoValidation = validateCurrentChatRepoUrl();
        const repo = repoValidation.repo;
        const baseBranch = currentChatBaseBranch();
        if (!threadId || !taskId || !prompt) {
          appendStreamLine("thread UUID, task UUID, and prompt are required");
          return;
        }
        if (repoValidation.error) {
          appendStreamLine(repoValidation.error);
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
            appendStreamLine(`runtime ${adminPreview("runtime state error", error)}`);
          }
        }, 5000);
        fetchRuntimeSummary(threadId).then((summary) => {
          lastRuntimeSummary = summary;
          appendStreamLine(`runtime ${summary}`);
        }).catch((error) => appendStreamLine(`runtime ${adminPreview("runtime state error", error)}`));
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
              repo,
              baseBranch,
              threadTitle: prompt.slice(0, 120)
            })
          });
        } finally {
          window.clearInterval(runtimePoll);
        }
        const textBody = await response.text();
        if (!response.ok) {
          appendStreamLine(`dispatch failed ${response.status}: ${adminPreview("dispatch response body", textBody)}`);
          return;
        }
        appendStreamLine(`dispatch accepted ${adminPreview("dispatch accepted response body", textBody)}`);
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
            label: "Pause/Sleep",
            action: "sleep",
            route: `/api/agents/threads/${encodeURIComponent(threadId)}/sleep`,
            confirm: "Scale this thread runtime to zero replicas?"
          },
          archive: {
            label: "Archive",
            action: "archive",
            route: `/api/agents/threads/${encodeURIComponent(threadId)}/archive`,
            confirm: "Archive this thread runtime?"
          },
          delete: {
            label: "Delete runtime",
            action: "hard-delete",
            route: `/api/agents/threads/${encodeURIComponent(threadId)}/hard-delete`,
            confirm: "Delete the Kubernetes runtime resources for this thread? GitHub PRs are not deleted."
          },
          merge: {
            label: "Merge with upstream",
            action: "merge-upstream",
            route: `/api/agents/threads/${encodeURIComponent(threadId)}/merge-upstream`,
            confirm: "Merge the configured base branch into this thread branch and push?"
          },
          makeCommit: {
            label: "Make commit",
            action: "make-commit",
            route: `/api/agents/threads/${encodeURIComponent(threadId)}/make-commit`,
            confirm: "Commit current worker changes and push this thread branch?",
            reason: "manual commit"
          },
          openPr: {
            label: "Open draft PR",
            action: "open-pr",
            route: `/api/agents/threads/${encodeURIComponent(threadId)}/open-pr`,
            confirm: "Open or reuse a draft WIP pull request for this thread branch?"
          },
          terminal: {
            label: "Terminal",
            action: "terminal",
            route: `/api/agents/threads/${encodeURIComponent(threadId)}/terminal`,
            confirm: "Open a terminal to this thread worker container?"
          }
        }[action];
        if (!config || !confirm(config.confirm)) return;
        const terminalWindow = config.action === "terminal" ? window.open("about:blank", "_blank") : null;
        const payload = {
          kind: "thread-control",
          action: config.action,
          threadId,
          taskId: $("chat-task-id").value.trim() || undefined,
          requestedBy: "rust-web-home",
          reason: config.reason || config.label
        };
        const taskId = payload.taskId || newUuid();
        payload.taskId = taskId;
        $("chat-task-id").value = taskId;
        openTaskWebSocket(threadId, taskId);
        if (["merge-upstream", "make-commit", "open-pr", "terminal"].includes(config.action)) {
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
          if (terminalWindow) terminalWindow.close();
          appendStreamLine(`${config.label} failed ${response.status}: ${adminPreview(`${config.label} response body`, textBody)}`);
          return;
        }
        appendStreamLine(`${config.label} accepted ${adminPreview(`${config.label} response body`, textBody)}`);
        if (config.action === "terminal") {
          const targetUrl = threadTerminalUrlFromControlResponse(threadId, textBody);
          if (terminalWindow) terminalWindow.location.href = targetUrl;
          else window.open(targetUrl, "_blank");
        }
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
          const selectedThread = (data.threads || []).find((thread) => thread.id === $("chat-thread-id").value.trim());
          if (selectedThread?.repo) setChatRepoSelection(selectedThread.repo);
          if (selectedThread?.baseBranch) $("chat-base-branch").value = selectedThread.baseBranch;
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
      $("save-chat-repo").addEventListener("click", () => {
        saveChatRepo().catch((error) => appendStreamLine(`repo URL save error: ${adminPreview("repo URL save error", error)}`));
      });
      $("chat-repo-url").addEventListener("change", updateChatRepoUrlMode);
      $("chat-repo-url-new").addEventListener("blur", validateChatRepoUrlField);
      $("chat-repo-url-new").addEventListener("input", () => $("chat-repo-url-new").setCustomValidity(""));
      $("thread-sleep").addEventListener("click", () => {
        runThreadControl("sleep").catch((error) => appendStreamLine(`sleep error: ${adminPreview("sleep error", error)}`));
      });
      $("thread-archive").addEventListener("click", () => {
        runThreadControl("archive").catch((error) => appendStreamLine(`archive error: ${adminPreview("archive error", error)}`));
      });
      $("thread-delete").addEventListener("click", () => {
        runThreadControl("delete").catch((error) => appendStreamLine(`delete error: ${adminPreview("delete error", error)}`));
      });
      $("thread-merge").addEventListener("click", () => {
        runThreadControl("merge").catch((error) => appendStreamLine(`merge error: ${adminPreview("merge error", error)}`));
      });
      $("thread-commit").addEventListener("click", () => {
        runThreadControl("makeCommit").catch((error) => appendStreamLine(`commit error: ${adminPreview("commit error", error)}`));
      });
      $("thread-open-pr").addEventListener("click", () => {
        runThreadControl("openPr").catch((error) => appendStreamLine(`open PR error: ${adminPreview("open PR error", error)}`));
      });
      $("thread-terminal").addEventListener("click", () => {
        runThreadControl("terminal").catch((error) => appendStreamLine(`terminal error: ${adminPreview("terminal error", error)}`));
      });
      $("send-chat").addEventListener("click", () => {
        dispatchChat().catch((error) => appendStreamLine(`dispatch error: ${adminPreview("dispatch error", error)}`));
      });
      $("chat-thread-id").addEventListener("input", setChatRoute);
      $("refresh").addEventListener("click", () => {
        loadKnownRepos().catch((error) => appendStreamLine(`known repos error: ${adminPreview("known repos error", error)}`));
        load();
      });
      $("limit").addEventListener("change", load);
      resetThreadId();
      loadKnownRepos().catch((error) => appendStreamLine(`known repos error: ${adminPreview("known repos error", error)}`));
      load();
      setInterval(load, 10000);
"#;
const WSS_TEST_HTML: &str = r##"<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>wss test lab</title>
    <style>
      :root {
        color-scheme: dark;
        --bg: #0b1117;
        --panel: #111923;
        --panel-2: #0f1720;
        --field: #0e1520;
        --line: rgba(148, 163, 184, 0.24);
        --text: #eef2f6;
        --muted: #a8b3c1;
        --accent: #5eead4;
        --warn: #fbbf24;
        --danger: #fb7185;
        --ok: #86efac;
      }
      * { box-sizing: border-box; }
      body {
        margin: 0;
        min-height: 100vh;
        background: var(--bg);
        color: var(--text);
        font: 14px/1.45 ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
      }
      header {
        position: sticky;
        top: 0;
        z-index: 10;
        display: grid;
        gap: 10px;
        padding: 12px 16px;
        background: var(--panel);
        border-bottom: 1px solid var(--line);
      }
      .topline {
        display: flex;
        align-items: center;
        gap: 10px;
        flex-wrap: wrap;
      }
      h1 { font-size: 18px; margin: 0 12px 0 0; }
      label { display: grid; gap: 3px; color: var(--muted); font-size: 11px; }
      input, select, textarea, button {
        background: var(--field);
        color: var(--text);
        border: 1px solid var(--line);
        border-radius: 6px;
        padding: 6px 8px;
        font: inherit;
      }
      input:focus, select:focus, textarea:focus, button:focus { outline: 1px solid var(--accent); }
      button { cursor: pointer; background: var(--panel-2); }
      button:hover { background: #182032; }
      button.primary { border-color: var(--accent); color: var(--accent); }
      button.danger { border-color: var(--danger); color: var(--danger); }
      .pill {
        display: inline-flex;
        align-items: center;
        min-height: 24px;
        padding: 2px 8px;
        border-radius: 999px;
        color: var(--accent);
        border: 1px solid rgba(94, 234, 212, 0.35);
        background: rgba(94, 234, 212, 0.08);
        font-size: 12px;
      }
      .pill.warn { color: var(--warn); border-color: rgba(251, 191, 36, 0.35); background: rgba(251, 191, 36, 0.08); }
      .pill.bad { color: var(--danger); border-color: rgba(251, 113, 133, 0.35); background: rgba(251, 113, 133, 0.08); }
      .pill.ok { color: var(--ok); border-color: rgba(134, 239, 172, 0.35); background: rgba(134, 239, 172, 0.08); }
      .stats {
        display: flex;
        gap: 8px;
        flex-wrap: wrap;
      }
      .grid {
        display: grid;
        grid-template-columns: minmax(0, 360px) minmax(0, 1fr);
        gap: 14px;
        padding: 16px;
      }
      .panel {
        min-width: 0;
        border: 1px solid var(--line);
        border-radius: 8px;
        background: var(--panel);
        overflow: hidden;
      }
      .panel h2 {
        margin: 0;
        padding: 10px 12px;
        font-size: 13px;
        background: var(--panel-2);
        border-bottom: 1px solid var(--line);
      }
      .panel-body { display: grid; gap: 10px; padding: 12px; }
      .fields { display: grid; grid-template-columns: 1fr 1fr; gap: 8px; }
      .field-wide { grid-column: 1 / -1; }
      .actions { display: flex; gap: 8px; flex-wrap: wrap; align-items: center; }
      textarea {
        width: 100%;
        min-height: 168px;
        resize: vertical;
        line-height: 1.45;
      }
      .log {
        margin: 0;
        min-height: 488px;
        max-height: calc(100vh - 210px);
        overflow: auto;
        padding: 10px;
        background: #090f16;
        color: var(--text);
        border-top: 1px solid var(--line);
        white-space: pre-wrap;
        word-break: break-word;
      }
      .row { padding: 2px 0; }
      .row.in { color: var(--ok); }
      .row.out { color: var(--accent); }
      .row.warn { color: var(--warn); }
      .row.bad { color: var(--danger); }
      .row.meta { color: var(--muted); }
      .ts { color: var(--muted); }
      code {
        display: inline-block;
        max-width: 100%;
        overflow-wrap: anywhere;
        border: 1px solid rgba(148, 163, 184, 0.2);
        border-radius: 6px;
        padding: 1px 5px;
        background: #0a1017;
        color: #d7fbf4;
      }
      @media (max-width: 860px) {
        .grid { grid-template-columns: 1fr; }
        .fields { grid-template-columns: 1fr; }
        .log { min-height: 320px; max-height: none; }
      }
    </style>
  </head>
  <body>
    <header>
      <div class="topline">
        <h1>wss test lab</h1>
        <label>preset
          <select id="preset">
            <option value="gleam">Gleam fan-out</option>
            <option value="webrtc">Rust WebRTC signaling</option>
            <option value="gcs">gms/gcs/chat.vibe router</option>
            <option value="fsrx">F# Rx burst</option>
          </select>
        </label>
        <label>base
          <input id="base" placeholder="same origin" style="width: 260px" />
        </label>
        <label>path
          <input id="path" style="width: 330px" />
        </label>
        <span id="status" class="pill warn">idle</span>
        <span id="counter" class="pill">0 frames</span>
        <span id="sent-counter" class="pill">0 sent</span>
        <span id="recv-counter" class="pill">0 recv</span>
      </div>
      <div class="topline">
        <code id="url-preview">ws://...</code>
      </div>
    </header>

    <main class="grid">
      <section class="panel">
        <h2>connection</h2>
        <div class="panel-body">
          <div class="fields">
            <label>thread id<input id="thread-id" /></label>
            <label>task id<input id="task-id" /></label>
            <label>room<input id="room-id" /></label>
            <label>peer<input id="peer-id" /></label>
            <label>user id<input id="user-id" /></label>
            <label>device id<input id="device-id" /></label>
            <label>conversation id<input id="conv-id" /></label>
            <label>burst count<input id="burst-count" type="number" min="1" max="500" value="12" /></label>
            <label>interval ms<input id="interval-ms" type="number" min="50" max="60000" value="1000" /></label>
            <label>gcs route
              <select id="gcs-route">
                <option value="conv">conv</option>
                <option value="user">user</option>
                <option value="device">device</option>
              </select>
            </label>
          </div>
          <div class="actions">
            <button id="connect" class="primary" type="button">connect</button>
            <button id="disconnect" class="danger" type="button">disconnect</button>
            <button id="copy-url" type="button">copy url</button>
            <button id="check-health" type="button">health</button>
            <button id="clear" type="button">clear</button>
          </div>
          <div class="actions">
            <a href="/presence-test?user=alice&amp;device=d1&amp;autoconnect=1"><code>/presence-test</code></a>
            <a href="/gleam/home"><code>/gleam/home</code></a>
            <a href="/webrtc/"><code>/webrtc/</code></a>
            <a href="/gcs/ws-health"><code>/gcs/ws-health</code></a>
            <a href="/wss-test?preset=fsrx"><code>/fsws/ws/rx-burst</code></a>
          </div>
        </div>
      </section>

      <section class="panel">
        <h2>frames</h2>
        <div class="panel-body">
          <textarea id="payload" spellcheck="false"></textarea>
          <div class="actions">
            <button id="send" class="primary" type="button">send</button>
            <button id="send-ping" type="button">ping</button>
            <button id="send-hello" type="button">hello</button>
            <button id="send-sample" type="button">sample</button>
            <button id="send-burst" type="button">burst</button>
            <button id="start-interval" type="button">start interval</button>
            <button id="stop-interval" type="button">stop interval</button>
          </div>
        </div>
      </section>

      <section class="panel" style="grid-column: 1 / -1">
        <h2>log</h2>
        <pre id="log" class="log"></pre>
      </section>
    </main>

    <script>
      const $ = (id) => document.getElementById(id);
      const params = new URLSearchParams(location.search);
      const defaults = {
        threadId: "00000000-0000-4000-8000-000000000001",
        taskId: "00000000-0000-4000-8000-000000000002",
        roomId: "browser-room",
        peerId: "peer-" + Math.random().toString(16).slice(2, 8),
        userId: "65c48f2f47d56fec05a41b38",
        deviceId: "65c48f2f47d56fec05a41b39",
        convId: "65c48f2f47d56fec05a41b3a",
      };
      const state = { ws: null, frames: 0, sent: 0, received: 0, intervalTimer: null };

      function sameOriginWsBase() {
        const proto = location.protocol === "https:" ? "wss" : "ws";
        return `${proto}://${location.host}`;
      }
      function httpToWs(value) {
        return value.replace(/^http:\/\//, "ws://").replace(/^https:\/\//, "wss://");
      }
      function wsToHttp(value) {
        return value.replace(/^ws:\/\//, "http://").replace(/^wss:\/\//, "https://");
      }
      function trimSlash(value) {
        return value.replace(/\/+$/, "");
      }
      function ensureLeadingSlash(value) {
        return value.startsWith("/") ? value : "/" + value;
      }
      function ts() {
        const d = new Date();
        return d.toTimeString().slice(0, 8) + "." + String(d.getMilliseconds()).padStart(3, "0");
      }
      function log(text, cls = "meta") {
        const row = document.createElement("div");
        row.className = "row " + cls;
        const stamp = document.createElement("span");
        stamp.className = "ts";
        stamp.textContent = ts() + " ";
        row.append(stamp, document.createTextNode(text));
        $("log").appendChild(row);
        while ($("log").childNodes.length > 500) $("log").removeChild($("log").firstChild);
        $("log").scrollTop = $("log").scrollHeight;
      }
      function setStatus(text, cls = "warn") {
        $("status").textContent = text;
        $("status").className = "pill " + cls;
      }
      function updateCounters() {
        $("counter").textContent = `${state.frames} frames`;
        $("sent-counter").textContent = `${state.sent} sent`;
        $("recv-counter").textContent = `${state.received} recv`;
      }
      function countFrame(direction) {
        state.frames += 1;
        if (direction === "out") state.sent += 1;
        if (direction === "in") state.received += 1;
        updateCounters();
      }
      function pretty(raw) {
        try { return JSON.stringify(JSON.parse(raw), null, 2); } catch (_) { return String(raw); }
      }
      function setGcsPath() {
        $("path").value = `/gcs/ws/${$("gcs-route").value}/${encodeURIComponent(gcsRouteId())}`;
      }

      function applyPreset() {
        const preset = $("preset").value;
        if (!$("base").value) $("base").placeholder = sameOriginWsBase();
        if (preset === "gleam") {
          $("path").value = "/gleam/ws";
          $("payload").value = "ping";
        } else if (preset === "webrtc") {
          $("path").value = "/webrtc/signal";
          $("payload").value = JSON.stringify({
            type: "hello",
            metadata: { client: "web-home-rs/wss-test", at: new Date().toISOString() }
          }, null, 2);
        } else if (preset === "gcs") {
          setGcsPath();
          $("payload").value = JSON.stringify({
            Meta: {},
            List: [{
              "@vibe-meta": {},
              "@vibe-type": "PollForKafkaMessages",
              "@vibe-data": JSON.stringify({ TopicIds: [$("user-id").value] })
            }]
          }, null, 2);
        } else {
          $("path").value = "/fsws/ws/rx-burst";
          $("payload").value = JSON.stringify({
            id: "rx-" + Date.now().toString(36),
            payload: "sample from web-home-rs/wss-test"
          }, null, 2);
        }
        updateUrlPreview();
      }

      function gcsRouteId() {
        const route = $("gcs-route").value;
        if (route === "user") return $("user-id").value;
        if (route === "device") return $("device-id").value;
        return $("conv-id").value;
      }

      function buildUrl() {
        const preset = $("preset").value;
        const base = trimSlash(httpToWs($("base").value.trim() || sameOriginWsBase()));
        if (preset === "gcs") setGcsPath();
        const path = ensureLeadingSlash($("path").value.trim());
        const url = new URL(base + path);

        if (preset === "gleam") {
          url.searchParams.set("threadId", $("thread-id").value.trim());
          url.searchParams.set("taskId", $("task-id").value.trim());
        } else if (preset === "webrtc") {
          url.searchParams.set("room", $("room-id").value.trim());
          url.searchParams.set("peer", $("peer-id").value.trim());
        } else {
          url.searchParams.set("userId", $("user-id").value.trim());
          url.searchParams.set("deviceId", $("device-id").value.trim());
          url.searchParams.set("conversationIds", JSON.stringify([$("conv-id").value.trim()]));
        }

        return url.toString();
      }

      function updateUrlPreview() {
        $("url-preview").textContent = buildUrl();
      }

      function healthPath() {
        const preset = $("preset").value;
        if (preset === "gleam") return "/gleam/healthz";
        if (preset === "webrtc") return "/webrtc/healthz";
        if (preset === "gcs") return "/gcs/ws-health";
        return "/fsws/healthz";
      }

      function httpBase() {
        const raw = $("base").value.trim();
        if (!raw) return location.origin;
        return trimSlash(wsToHttp(httpToWs(raw)));
      }

      async function checkHealth() {
        const url = httpBase() + healthPath();
        log("GET " + url, "meta");
        try {
          const response = await fetch(url, { cache: "no-store" });
          const text = await response.text();
          log(`health ${response.status}: ${text.slice(0, 600)}`, response.ok ? "in" : "bad");
        } catch (error) {
          log("health error: " + String(error), "bad");
        }
      }

      function connect() {
        disconnect();
        const url = buildUrl();
        const ws = new WebSocket(url);
        state.ws = ws;
        setStatus("connecting", "warn");
        log("open " + url, "meta");
        ws.onopen = () => {
          setStatus("open", "ok");
          log("connected", "meta");
          if ($("preset").value === "webrtc") sendHello();
        };
        ws.onmessage = (event) => {
          countFrame("in");
          log("in  " + pretty(event.data), "in");
        };
        ws.onerror = () => {
          setStatus("error", "bad");
          log("websocket error; check browser devtools network panel", "bad");
        };
        ws.onclose = (event) => {
          stopInterval();
          setStatus(`closed ${event.code}`, event.code === 1000 ? "warn" : "bad");
          log(`closed code=${event.code} reason="${event.reason || ""}"`, "warn");
          if (state.ws === ws) state.ws = null;
        };
      }

      function disconnect() {
        stopInterval();
        if (state.ws) {
          try { state.ws.close(1000, "ui disconnect"); } catch (_) {}
          state.ws = null;
        }
        setStatus("idle", "warn");
      }

      function sendRaw(raw) {
        if (!state.ws || state.ws.readyState !== WebSocket.OPEN) {
          log("not connected", "bad");
          return;
        }
        state.ws.send(raw);
        countFrame("out");
        log("out " + pretty(raw), "out");
      }

      function sendPayload() {
        const raw = $("payload").value;
        if (raw.trim()) sendRaw(raw);
      }

      function sendPing() {
        if ($("preset").value === "webrtc") {
          sendRaw(JSON.stringify({ type: "ping" }));
        } else {
          sendRaw("ping");
        }
      }

      function sendHello() {
        if ($("preset").value === "webrtc") {
          sendRaw(JSON.stringify({
            type: "hello",
            metadata: { client: "web-home-rs/wss-test", peer: $("peer-id").value }
          }));
        } else {
          sendRaw("hello from web-home-rs/wss-test");
        }
      }

      function sampleFrame(index = null) {
        if ($("preset").value === "gleam") {
          return JSON.stringify({
            type: "task-event",
            threadId: $("thread-id").value,
            taskId: $("task-id").value,
            body: index === null ? "sample from wss-test" : `sample ${index} from wss-test`,
            at: new Date().toISOString()
          });
        }
        if ($("preset").value === "webrtc") {
          return JSON.stringify({
            type: "message",
            payload: {
              body: index === null ? "sample signaling message" : `sample signaling message ${index}`,
              at: new Date().toISOString()
            }
          });
        }
        if ($("preset").value === "gcs") {
          return JSON.stringify({
            Meta: {},
            List: [{
              "@vibe-meta": {},
              "@vibe-type": "PollForKafkaMessages",
              "@vibe-data": JSON.stringify({
                TopicIds: [$("user-id").value, $("conv-id").value],
                Sequence: index
              })
            }]
          });
        }
        return JSON.stringify({
          id: `rx-${Date.now().toString(36)}-${index === null ? "sample" : index}`,
          payload: index === null ? "sample from wss-test" : `burst payload ${index}`
        });
      }

      function sendSample() {
        sendRaw(sampleFrame());
      }

      function sendBurst() {
        const count = Math.min(500, Math.max(1, Number.parseInt($("burst-count").value, 10) || 1));
        for (let i = 0; i < count; i += 1) {
          sendRaw(sampleFrame(i + 1));
        }
      }

      function stopInterval() {
        if (state.intervalTimer !== null) {
          clearInterval(state.intervalTimer);
          state.intervalTimer = null;
          log("interval stopped", "meta");
        }
      }

      function startInterval() {
        stopInterval();
        const ms = Math.min(60000, Math.max(50, Number.parseInt($("interval-ms").value, 10) || 1000));
        state.intervalTimer = setInterval(sendSample, ms);
        log(`interval started ${ms}ms`, "meta");
      }

      $("preset").value = params.get("preset") || "gleam";
      $("base").value = params.get("base") || "";
      $("thread-id").value = params.get("threadId") || defaults.threadId;
      $("task-id").value = params.get("taskId") || defaults.taskId;
      $("room-id").value = params.get("room") || defaults.roomId;
      $("peer-id").value = params.get("peer") || defaults.peerId;
      $("user-id").value = params.get("userId") || defaults.userId;
      $("device-id").value = params.get("deviceId") || defaults.deviceId;
      $("conv-id").value = params.get("convId") || defaults.convId;

      $("preset").addEventListener("change", applyPreset);
      $("gcs-route").addEventListener("change", applyPreset);
      for (const id of ["base", "path", "thread-id", "task-id", "room-id", "peer-id", "user-id", "device-id", "conv-id"]) {
        $(id).addEventListener("input", updateUrlPreview);
      }
      for (const id of ["burst-count", "interval-ms"]) {
        $(id).addEventListener("input", updateUrlPreview);
      }
      $("connect").onclick = connect;
      $("disconnect").onclick = disconnect;
      $("send").onclick = sendPayload;
      $("send-ping").onclick = sendPing;
      $("send-hello").onclick = sendHello;
      $("send-sample").onclick = sendSample;
      $("send-burst").onclick = sendBurst;
      $("start-interval").onclick = startInterval;
      $("stop-interval").onclick = stopInterval;
      $("check-health").onclick = () => { checkHealth().catch((error) => log("health error: " + String(error), "bad")); };
      $("clear").onclick = () => {
        $("log").textContent = "";
        state.frames = 0;
        state.sent = 0;
        state.received = 0;
        updateCounters();
      };
      $("copy-url").onclick = async () => {
        try {
          await navigator.clipboard.writeText(buildUrl());
          log("copied url", "meta");
        } catch (_) {
          log("copy failed", "bad");
        }
      };
      $("payload").addEventListener("keydown", (event) => {
        if ((event.metaKey || event.ctrlKey) && event.key === "Enter") sendPayload();
      });

      applyPreset();
      window.addEventListener("beforeunload", disconnect);
      if (params.get("autoconnect") === "1") setTimeout(connect, 50);
    </script>
  </body>
</html>
"##;
const PRESENCE_TEST_HTML: &str = r##"<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>presence test</title>
    <style>
      :root {
        color-scheme: dark;
        --bg: #0b1117;
        --panel: #111923;
        --panel-2: #0f1720;
        --field: #0e1520;
        --line: rgba(148, 163, 184, 0.24);
        --text: #eef2f6;
        --muted: #a8b3c1;
        --accent: #5eead4;
        --warn: #fbbf24;
        --danger: #fb7185;
        --ok: #86efac;
      }
      * { box-sizing: border-box; }
      body {
        margin: 0;
        min-height: 100vh;
        background: var(--bg);
        color: var(--text);
        font: 14px/1.45 ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
      }
      header {
        display: flex;
        flex-wrap: wrap;
        gap: 8px 16px;
        align-items: center;
        padding: 12px 16px;
        background: var(--panel);
        border-bottom: 1px solid var(--line);
        position: sticky;
        top: 0;
        z-index: 10;
      }
      header label { display: flex; flex-direction: column; gap: 2px; font-size: 11px; color: var(--muted); }
      header input { width: 180px; }
      input, button, select {
        background: var(--field);
        color: var(--text);
        border: 1px solid var(--line);
        border-radius: 6px;
        padding: 5px 8px;
        font: inherit;
      }
      input:focus, button:focus { outline: 1px solid var(--accent); }
      button {
        cursor: pointer;
        background: var(--panel-2);
        transition: background .12s ease;
      }
      button:hover { background: #182032; }
      button.primary { border-color: var(--accent); color: var(--accent); }
      button.danger { border-color: var(--danger); color: var(--danger); }
      .pill {
        display: inline-block;
        padding: 1px 7px;
        border-radius: 9px;
        font-size: 11px;
        background: rgba(94, 234, 212, 0.12);
        color: var(--accent);
        border: 1px solid rgba(94, 234, 212, 0.3);
      }
      .pill.warn { background: rgba(251,191,36,.12); color: var(--warn); border-color: rgba(251,191,36,.3); }
      .pill.bad { background: rgba(251,113,133,.12); color: var(--danger); border-color: rgba(251,113,133,.3); }
      .pill.ok { background: rgba(134,239,172,.12); color: var(--ok); border-color: rgba(134,239,172,.3); }
      main { padding: 16px; display: grid; gap: 16px; grid-template-columns: 1fr; }
      @media (min-width: 1080px) {
        main { grid-template-columns: 1fr 1fr; }
        .user-panel { grid-column: 1 / -1; }
      }
      .panel {
        background: var(--panel);
        border: 1px solid var(--line);
        border-radius: 8px;
        display: flex;
        flex-direction: column;
        min-height: 240px;
        overflow: hidden;
      }
      .panel-head {
        display: flex;
        gap: 8px;
        align-items: center;
        flex-wrap: wrap;
        padding: 8px 12px;
        border-bottom: 1px solid var(--line);
        background: var(--panel-2);
      }
      .panel-head .title { font-weight: 600; color: var(--text); }
      .panel-head .meta { font-size: 11px; color: var(--muted); }
      .panel-body { display: flex; flex-direction: column; flex: 1; min-height: 0; }
      .controls { display: flex; gap: 6px; padding: 8px 12px; flex-wrap: wrap; align-items: center; }
      .controls input[type="text"] { flex: 1; min-width: 140px; }
      .log {
        flex: 1;
        margin: 0 12px 12px;
        padding: 8px;
        background: var(--panel-2);
        border: 1px solid var(--line);
        border-radius: 6px;
        overflow: auto;
        font-size: 12px;
        min-height: 160px;
        max-height: 320px;
      }
      .log .row { padding: 1px 0; white-space: pre-wrap; word-break: break-word; }
      .log .row.system { color: var(--accent); }
      .log .row.warn { color: var(--warn); }
      .log .row.bad { color: var(--danger); }
      .log .row.muted { color: var(--muted); }
      .log .ts { color: var(--muted); }
      .quick-bar {
        display: flex;
        gap: 8px;
        flex-wrap: wrap;
        padding: 8px 16px;
        background: var(--panel-2);
        border-bottom: 1px solid var(--line);
      }
      footer { padding: 12px 16px; color: var(--muted); font-size: 12px; border-top: 1px solid var(--line); }
      code { background: var(--panel-2); padding: 1px 6px; border-radius: 4px; }
    </style>
  </head>
  <body>
    <header>
      <label>user-id<input id="user" value="alice" /></label>
      <label>device-id<input id="device" value="d1" /></label>
      <label>presence base<input id="presence" value="http://localhost:8081" style="width: 220px;" /></label>
      <label>conv ids (comma)<input id="convs" value="conv-1,conv-2,conv-3,conv-4,conv-5" style="width: 260px;" /></label>
      <div style="flex:1"></div>
      <button id="connect" class="primary" type="button">Connect all</button>
      <button id="disconnect" type="button">Disconnect all</button>
      <span id="status" class="pill warn">idle</span>
    </header>

    <div class="quick-bar">
      <span class="pill" id="self-info">no session</span>
      <span class="pill warn" id="ws-count">0 / 6 ws open</span>
      <span class="pill" id="hello-node">node: ?</span>
      <span class="muted" style="margin-left:auto">open this page in 3 tabs (alice/d1, bob/d2, carol/d3) to test cross-user fan-out</span>
    </div>

    <main id="grid">
      <section class="panel user-panel" id="user-panel">
        <div class="panel-head">
          <span class="title">user-ws</span>
          <span class="meta" id="user-meta">/ws?user=…&amp;device=…</span>
          <span id="user-status" class="pill bad" style="margin-left:auto">closed</span>
        </div>
        <div class="controls">
          <input id="user-broadcast-input" type="text" placeholder="send to /user/&lt;me&gt;/broadcast — every user-ws of me on every node" />
          <button id="user-broadcast-send" type="button">user-broadcast</button>
          <button id="user-logout" class="danger" type="button">logout this device</button>
          <button id="user-clear" type="button">clear log</button>
        </div>
        <div class="log" id="user-log"></div>
      </section>
      <!-- conv panels are injected here -->
    </main>

    <footer>
      <div>Quick links:</div>
      <div>
        <a href="?user=alice&amp;device=d1&amp;autoconnect=1">alice / d1</a> ·
        <a href="?user=bob&amp;device=d2&amp;autoconnect=1">bob / d2</a> ·
        <a href="?user=carol&amp;device=d3&amp;autoconnect=1">carol / d3</a>
      </div>
      <div style="margin-top:6px">
        Routes exercised:
        <code>GET /ws?user=…&amp;device=…</code>,
        <code>GET /ws?user=…&amp;conv=…&amp;device=…</code>,
        <code>POST /conv/&lt;id&gt;/members/&lt;user&gt;</code>,
        <code>DELETE /conv/&lt;id&gt;/members/&lt;user&gt;</code>,
        <code>POST /conv/&lt;id&gt;/broadcast</code>,
        <code>POST /user/&lt;id&gt;/broadcast</code>,
        <code>POST /user/&lt;u&gt;/devices/&lt;d&gt;/logout</code>.
      </div>
    </footer>

    <script>
      const $ = (id) => document.getElementById(id);

      // Apply ?user=, ?device=, ?presence=, ?convs=, ?autoconnect= from URL.
      const params = new URLSearchParams(location.search);
      for (const k of ["user", "device", "presence", "convs"]) {
        if (params.has(k)) $(k).value = params.get(k);
      }

      // ───────────────────────────────────────────────────────────────────
      // state
      const state = {
        userWs: null,
        convs: {},          // convId → { ws, panel, logEl, statusEl, membersEl }
        helloUserNode: null,
      };

      function nowTs() {
        const d = new Date();
        return d.toTimeString().slice(0, 8) + "." + String(d.getMilliseconds()).padStart(3, "0");
      }

      function log(panelLog, text, cls = "") {
        const row = document.createElement("div");
        row.className = "row " + cls;
        const ts = document.createElement("span");
        ts.className = "ts";
        ts.textContent = nowTs() + " ";
        row.append(ts, document.createTextNode(text));
        panelLog.appendChild(row);
        // Keep ~400 lines max.
        while (panelLog.childNodes.length > 400) panelLog.removeChild(panelLog.firstChild);
        panelLog.scrollTop = panelLog.scrollHeight;
      }

      function setPill(el, text, cls) {
        el.textContent = text;
        el.className = "pill " + cls;
      }

      function wsBase() {
        const http = $("presence").value.trim().replace(/\/$/, "");
        return http.replace(/^http:\/\//, "ws://").replace(/^https:\/\//, "wss://");
      }
      function httpBase() {
        return $("presence").value.trim().replace(/\/$/, "");
      }

      function updateWsCount() {
        let open = 0;
        if (state.userWs && state.userWs.readyState === WebSocket.OPEN) open++;
        for (const k in state.convs) {
          const c = state.convs[k];
          if (c.ws && c.ws.readyState === WebSocket.OPEN) open++;
        }
        const total = 1 + Object.keys(state.convs).length;
        const el = $("ws-count");
        el.textContent = `${open} / ${total} ws open`;
        el.className = "pill " + (open === total ? "ok" : open === 0 ? "bad" : "warn");
      }

      function applySelfInfo() {
        $("self-info").textContent = `me: ${$("user").value || "?"}@${$("device").value || "?"}`;
      }

      // ───────────────────────────────────────────────────────────────────
      // Conv panels — built once from the comma-separated list, never re-
      // rendered. Each panel owns one conv-ws lifecycle.
      function buildConvPanels() {
        const grid = $("grid");
        // Remove any existing conv panels (everything after user-panel).
        Array.from(grid.querySelectorAll(".panel.conv-panel")).forEach((n) => n.remove());
        state.convs = {};

        const convIds = $("convs").value.split(",").map((s) => s.trim()).filter(Boolean);
        for (const convId of convIds) {
          const panel = document.createElement("section");
          panel.className = "panel conv-panel";
          panel.dataset.conv = convId;
          panel.innerHTML = `
            <div class="panel-head">
              <span class="title">${convId}</span>
              <span class="meta">/ws?user=&hellip;&amp;conv=${convId}</span>
              <span class="pill" data-role="members">members: —</span>
              <span class="pill bad" data-role="status" style="margin-left:auto">closed</span>
            </div>
            <div class="controls">
              <button type="button" data-act="join">join (me)</button>
              <button type="button" data-act="leave" class="danger">leave (me)</button>
              <button type="button" data-act="open">open ws</button>
              <button type="button" data-act="close" class="danger">close ws</button>
              <button type="button" data-act="refresh">refresh members</button>
            </div>
            <div class="controls">
              <input type="text" data-role="broadcast-input" placeholder="broadcast to ${convId} — every conv-ws of every member" />
              <button type="button" data-act="broadcast">send</button>
              <button type="button" data-act="clear">clear log</button>
            </div>
            <div class="log" data-role="log"></div>
          `;
          grid.appendChild(panel);

          const logEl = panel.querySelector('[data-role="log"]');
          const statusEl = panel.querySelector('[data-role="status"]');
          const membersEl = panel.querySelector('[data-role="members"]');
          const broadcastInput = panel.querySelector('[data-role="broadcast-input"]');

          panel.querySelector('[data-act="join"]').onclick = () => joinConv(convId);
          panel.querySelector('[data-act="leave"]').onclick = () => leaveConv(convId);
          panel.querySelector('[data-act="open"]').onclick = () => openConvWs(convId);
          panel.querySelector('[data-act="close"]').onclick = () => closeConvWs(convId);
          panel.querySelector('[data-act="refresh"]').onclick = () => refreshConvMembers(convId);
          panel.querySelector('[data-act="broadcast"]').onclick = () => {
            const v = broadcastInput.value;
            if (!v) return;
            convBroadcast(convId, v);
            broadcastInput.value = "";
          };
          broadcastInput.addEventListener("keydown", (e) => {
            if (e.key === "Enter") panel.querySelector('[data-act="broadcast"]').click();
          });
          panel.querySelector('[data-act="clear"]').onclick = () => { logEl.textContent = ""; };

          state.convs[convId] = { ws: null, panel, logEl, statusEl, membersEl };
        }
      }

      // ───────────────────────────────────────────────────────────────────
      // user-ws lifecycle
      function openUserWs() {
        if (state.userWs && state.userWs.readyState <= 1) return;
        const user = $("user").value.trim();
        const device = $("device").value.trim();
        if (!user) { log($("user-log"), "missing user-id", "bad"); return; }
        const qs = new URLSearchParams({ user });
        if (device) qs.set("device", device);
        const url = `${wsBase()}/ws?${qs}`;
        $("user-meta").textContent = url;
        const ws = new WebSocket(url);
        state.userWs = ws;
        setPill($("user-status"), "connecting", "warn");
        log($("user-log"), `→ open ${url}`, "muted");
        ws.onopen = () => { setPill($("user-status"), "open", "ok"); updateWsCount(); };
        ws.onclose = (e) => {
          setPill($("user-status"), `closed (${e.code})`, "bad");
          log($("user-log"), `← close code=${e.code} reason="${e.reason || ""}"`, "warn");
          updateWsCount();
        };
        ws.onerror = () => log($("user-log"), "← error (see devtools)", "bad");
        ws.onmessage = (e) => handleUserFrame(e.data);
      }

      function closeUserWs() {
        if (state.userWs) {
          try { state.userWs.close(); } catch (_) {}
          state.userWs = null;
        }
        setPill($("user-status"), "closed", "bad");
        updateWsCount();
      }

      function handleUserFrame(raw) {
        const sys = tryParseSystemFrame(raw);
        if (!sys) {
          log($("user-log"), `← payload: ${raw}`);
          return;
        }
        log($("user-log"), `← ${raw}`, "system");
        if (sys.type === "hello") {
          $("hello-node").textContent = `node: ${sys.node}`;
          state.helloUserNode = sys.node;
        } else if (sys.type === "membership-changed") {
          if (sys.change === "added" && state.convs[sys.conv]) {
            const c = state.convs[sys.conv];
            const members = Array.isArray(sys.members) ? sys.members : [];
            setPill(c.membersEl, `members: ${members.join(",") || "—"}`, "");
            log(c.logEl, `(user-ws) added; members=[${members.join(",")}]`, "system");
          } else if (sys.change === "removed" && state.convs[sys.conv]) {
            const c = state.convs[sys.conv];
            log(c.logEl, "(user-ws) removed from conv", "warn");
            setPill(c.membersEl, "members: (you left)", "warn");
          }
        } else if (sys.type === "kick") {
          log($("user-log"), `kick: ${sys.reason}`, "bad");
        }
      }

      // ───────────────────────────────────────────────────────────────────
      // conv-ws lifecycle
      function openConvWs(convId) {
        const c = state.convs[convId];
        if (!c) return;
        if (c.ws && c.ws.readyState <= 1) return;
        const user = $("user").value.trim();
        const device = $("device").value.trim();
        const qs = new URLSearchParams({ user, conv: convId });
        if (device) qs.set("device", device);
        const url = `${wsBase()}/ws?${qs}`;
        const ws = new WebSocket(url);
        c.ws = ws;
        setPill(c.statusEl, "connecting", "warn");
        log(c.logEl, `→ open ${url}`, "muted");
        ws.onopen = () => { setPill(c.statusEl, "open", "ok"); updateWsCount(); };
        ws.onclose = (e) => {
          setPill(c.statusEl, `closed (${e.code})`, "bad");
          log(c.logEl, `← close code=${e.code} reason="${e.reason || ""}"`, "warn");
          updateWsCount();
        };
        ws.onerror = () => log(c.logEl, "← error", "bad");
        ws.onmessage = (e) => handleConvFrame(convId, e.data);
      }

      function closeConvWs(convId) {
        const c = state.convs[convId];
        if (c && c.ws) {
          try { c.ws.close(); } catch (_) {}
          c.ws = null;
        }
        if (c) setPill(c.statusEl, "closed", "bad");
        updateWsCount();
      }

      function handleConvFrame(convId, raw) {
        const c = state.convs[convId];
        if (!c) return;
        const sys = tryParseSystemFrame(raw);
        if (!sys) {
          log(c.logEl, `← payload: ${raw}`);
          return;
        }
        log(c.logEl, `← ${raw}`, "system");
        if (sys.type === "kick") {
          setPill(c.statusEl, `kicked`, "bad");
        }
      }

      // ───────────────────────────────────────────────────────────────────
      // HTTP API calls
      async function joinConv(convId) {
        const user = $("user").value.trim();
        const c = state.convs[convId];
        const res = await postPlain(`${httpBase()}/conv/${enc(convId)}/members/${enc(user)}`);
        if (c) log(c.logEl, `POST /members/${user} → ${res}`, "system");
        // Refresh membership pill (the user-ws will also see the membership-
        // changed JSON if I'm registered).
        refreshConvMembers(convId);
      }

      async function leaveConv(convId) {
        const user = $("user").value.trim();
        const c = state.convs[convId];
        const res = await deletePlain(`${httpBase()}/conv/${enc(convId)}/members/${enc(user)}`);
        if (c) log(c.logEl, `DELETE /members/${user} → ${res}`, "warn");
        refreshConvMembers(convId);
      }

      async function refreshConvMembers(convId) {
        const c = state.convs[convId];
        if (!c) return;
        try {
          const r = await fetch(`${httpBase()}/conv/${enc(convId)}/members`);
          const body = (await r.text()).trim();
          const members = body ? body.split("\n") : [];
          setPill(c.membersEl, `members: ${members.join(",") || "—"}`, members.length ? "" : "warn");
        } catch (e) {
          setPill(c.membersEl, "members: ?", "bad");
        }
      }

      async function convBroadcast(convId, payload) {
        const c = state.convs[convId];
        const res = await postPlain(`${httpBase()}/conv/${enc(convId)}/broadcast`, payload);
        if (c) log(c.logEl, `POST /broadcast (${payload.length}B) → ${res}`, "muted");
      }

      async function userBroadcast(payload) {
        const user = $("user").value.trim();
        const res = await postPlain(`${httpBase()}/user/${enc(user)}/broadcast`, payload);
        log($("user-log"), `POST /user/${user}/broadcast → ${res}`, "muted");
      }

      async function deviceLogout() {
        const user = $("user").value.trim();
        const device = $("device").value.trim();
        if (!device) { log($("user-log"), "device-id required for logout", "bad"); return; }
        const res = await postPlain(`${httpBase()}/user/${enc(user)}/devices/${enc(device)}/logout`, "ui-button");
        log($("user-log"), `POST /devices/${device}/logout → ${res}`, "warn");
      }

      // ───────────────────────────────────────────────────────────────────
      // helpers
      async function postPlain(url, body = "") {
        try {
          const r = await fetch(url, { method: "POST", body, headers: { "content-type": "text/plain" } });
          return `HTTP ${r.status} ${(await r.text()).trim()}`;
        } catch (e) { return `error: ${e}`; }
      }
      async function deletePlain(url) {
        try {
          const r = await fetch(url, { method: "DELETE" });
          return `HTTP ${r.status} ${(await r.text()).trim()}`;
        } catch (e) { return `error: ${e}`; }
      }
      function enc(s) { return encodeURIComponent(s); }

      function tryParseSystemFrame(raw) {
        if (typeof raw !== "string") return null;
        const s = raw.trimStart();
        if (!s.startsWith("{")) return null;
        try {
          const o = JSON.parse(s);
          return typeof o === "object" && o && typeof o.type === "string" ? o : null;
        } catch (_) { return null; }
      }

      // ───────────────────────────────────────────────────────────────────
      // top-level connect / disconnect
      async function connectAll() {
        setPill($("status"), "connecting", "warn");
        openUserWs();
        // Join every conv THEN open its ws. Membership is required for the
        // conv-ws upgrade to succeed.
        for (const convId of Object.keys(state.convs)) {
          await joinConv(convId);
          openConvWs(convId);
        }
        setPill($("status"), "connected", "ok");
      }

      function disconnectAll() {
        closeUserWs();
        for (const convId of Object.keys(state.convs)) closeConvWs(convId);
        setPill($("status"), "idle", "warn");
      }

      // ───────────────────────────────────────────────────────────────────
      // wire up
      $("user").addEventListener("input", applySelfInfo);
      $("device").addEventListener("input", applySelfInfo);
      $("convs").addEventListener("change", buildConvPanels);

      $("connect").onclick = connectAll;
      $("disconnect").onclick = disconnectAll;
      $("user-broadcast-send").onclick = () => {
        const v = $("user-broadcast-input").value;
        if (!v) return;
        userBroadcast(v);
        $("user-broadcast-input").value = "";
      };
      $("user-broadcast-input").addEventListener("keydown", (e) => {
        if (e.key === "Enter") $("user-broadcast-send").click();
      });
      $("user-logout").onclick = deviceLogout;
      $("user-clear").onclick = () => { $("user-log").textContent = ""; };

      applySelfInfo();
      buildConvPanels();
      updateWsCount();
      // Periodic ws-count refresh in case readyState changes silently.
      setInterval(updateWsCount, 1000);

      if (params.get("autoconnect") === "1") {
        // Defer one tick so panels are in the DOM before the WSes try to
        // resolve. Then fire-and-forget.
        setTimeout(connectAll, 50);
      }
    </script>
  </body>
</html>
"##;
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
      .check-row {
        min-height: 34px;
        display: flex;
        align-items: center;
        gap: 8px;
        padding-top: 19px;
      }
      .check-row input { width: auto; min-height: auto; }
      .check-row span { margin: 0; }
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
                <option value="nodejs">nodejs</option>
                <option value="python3">python3</option>
                <option value="ruby">ruby</option>
                <option value="bash">bash</option>
              </select>
            </label>
            <label class="check-row">
              <input id="containerized" type="checkbox" />
              <span>Containerize</span>
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
            <label>
              <span>Container image</span>
              <input id="container-image" autocomplete="off" readonly spellcheck="false" />
            </label>
            <label>
              <span>Build status</span>
              <input id="container-build-status" autocomplete="off" readonly spellcheck="false" />
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
      const entryCommands = {
        nodejs: "env -i PATH=\"$PATH\" NODE_ENV=production node --permission --allow-net child-runtimes/js-function-runner.mjs",
        python3: "env -i PATH=\"$PATH\" PYTHONUNBUFFERED=1 python3 child-runtimes/python-function-runner.py",
        ruby: "env -i PATH=\"$PATH\" ruby child-runtimes/ruby-function-runner.rb",
        bash: "env -i PATH=\"$PATH\" node --permission --allow-net --allow-child-process child-runtimes/bash-function-runner.mjs",
      };
      const hostAllowedRuntimes = new Set(["nodejs"]);
      const defaultCommand = entryCommands.nodejs;
      const state = {
        functions: [],
        selectedId: null,
      };

      function normalizeRuntime(value) {
        if (value === "javascript" || value === "typescript" || value === "node") return "nodejs";
        if (value === "python") return "python3";
        if (value === "shell") return "bash";
        return entryCommands[value] ? value : "nodejs";
      }

      function defaultFunctionBody(runtime) {
        switch (normalizeRuntime(runtime)) {
          case "python3":
            return "result = { \"status\": 200, \"body\": { \"ok\": True, \"echo\": request.get(\"body\") } }";
          case "ruby":
            return "{ status: 200, body: { ok: true, echo: request[\"body\"] } }";
          case "bash":
            return "printf '%s\\n' '{\"status\":200,\"body\":{\"ok\":true}}'";
          default:
            return "return { status: 200, body: { ok: true, echo: request.body ?? null } };";
        }
      }

      function syncEntryCommand() {
        $("entry-command").value = entryCommands[normalizeRuntime($("runtime").value)] || defaultCommand;
      }

      function syncContainerPolicy() {
        const requiresContainer = !hostAllowedRuntimes.has(normalizeRuntime($("runtime").value));
        $("containerized").disabled = requiresContainer;
        $("containerized").title = requiresContainer ? "This runtime requires container execution." : "";
        if (requiresContainer) $("containerized").checked = true;
      }

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
          runtime: normalizeRuntime($("runtime").value),
          entryCommand: entryCommands[normalizeRuntime($("runtime").value)] || defaultCommand,
          functionBody: $("function-body").value,
          reuseKey: $("reuse-key").value.trim() || null,
          idleTimeoutSeconds: Number($("idle-timeout").value || 300),
          maxRunMs: Number($("max-run").value || 30000),
          containerized: $("containerized").checked,
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
        $("runtime").value = normalizeRuntime(fn?.runtime || "nodejs");
        $("reuse-key").value = fn?.reuseKey || "";
        $("idle-timeout").value = fn?.idleTimeoutSeconds || 300;
        $("max-run").value = fn?.maxRunMs || 30000;
        syncEntryCommand();
        $("containerized").checked = Boolean(fn?.containerized);
        syncContainerPolicy();
        $("container-image").value = fn?.containerImage || "";
        $("container-build-status").value = fn?.containerBuildStatus || (fn?.containerized ? "pending" : "not_requested");
        $("description").value = fn?.description || "";
        $("function-body").value = fn?.functionBody || defaultFunctionBody($("runtime").value);
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
          const mode = fn.containerized ? `container ${fn.containerBuildStatus || "pending"}` : "host";
          meta.textContent = `${fn.slug} - ${fn.id.slice(0, 8)} - ${normalizeRuntime(fn.runtime)} - ${mode} - updated ${fmt(fn.updatedAt)}`;
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
      $("runtime").addEventListener("change", () => {
        syncEntryCommand();
        syncContainerPolicy();
        if (!selectedFunction() && !$("function-body").value.trim()) {
          $("function-body").value = defaultFunctionBody($("runtime").value);
        }
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
        .route("/presence-test", get(presence_test_page))
        .route("/presence-test/", get(presence_test_page))
        .route("/wss-test", get(wss_test_page))
        .route("/wss-test/", get(wss_test_page))
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
