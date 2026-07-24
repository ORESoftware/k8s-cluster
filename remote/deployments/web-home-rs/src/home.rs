use axum::response::Html;
use dd_nats_subject_defs::{
    CONTRACTS_SOLANA_VALIDATE_SUBJECT, DES_SIMULATE_SUBJECT, FABRICATION_REQUESTS_SUBJECT,
    FABRICATION_RESULTS_SUBJECT, MDP_OPTIMIZE_SUBJECT, ML_FEATURES_SUBJECT, TELEMETRY_MDP_SUBJECT,
    TELEMETRY_RAW_SUBJECT, THREAD_TASKS_WILDCARD, TRADING_ORDER_INTENTS_SUBJECT,
    TRADING_SIGNALS_SUBJECT,
};
use maud::{html, Markup, PreEscaped, DOCTYPE};

use crate::grafana::grafana_deployment_path;
use crate::shared::{shared_header, SHARED_HEADER_BOOT_JS};
use crate::state::AppState;

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

pub(crate) fn home_document(state: &AppState) -> Html<String> {
    Html(
        html! {
            (DOCTYPE)
            html lang="en" data-dd-mode="dark" {
                head {
                    meta charset="utf-8";
                    meta name="viewport" content="width=device-width, initial-scale=1";
                    title { "dd-remote-web" }
                    script { (PreEscaped(SHARED_HEADER_BOOT_JS)) }
                    style { (PreEscaped(HOME_CSS)) }
                    link rel="stylesheet" href="/assets/web-home/shared-header.css";
                    script defer="defer" src="/assets/web-home/shared-header.js" {}
                }
                body {
                    (shared_header("home"))
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
            code { "/factmachine-markets" } ", " code { "/jello" } ", " code { "/music/" } ", " code { "/presence-test" } ", "
            code { "/wss-test" } ", " code { "/api-docs" } ". Server-auth paths: "
            code { "/api/agents/" } ", " code { "/lambdas/functions" } ", " code { "/lambdas/invoke/<function-id>" } ", "
            code { "/api/lambdas/" } ", " code { "/api/agent-worker/" } ", "
            code { "/container-pools" } ", " code { "/bastion/" } ", " code { "/scrape" } ", "
            code { "/trading/" } ", " code { "/contracts/" } ", " code { "/compliance/" } ", " code { "/ml/" } ", "
            code { "/builds" } ", " code { "/gleam/" } ", " code { "/presence/" } ", " code { "/mcp" } ", and "
            code { "/gcs/" } ", " code { "/webrtc/" } ", " code { "/webrtc-media/" } ", " code { "/fsws/" } ", "
            code { "/mdp/" } ", " code { "/des/" } ", and " code { "/des-rs/" } ". Internal-access ops: " code { "/headlamp/" } ", "
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
            td {
                (code_list(row.deployments))
                div class="grafana-links" {
                    @for deployment in row.deployments {
                        a href=(grafana_deployment_path(deployment)) {
                            "Grafana"
                            @if row.deployments.len() > 1 {
                                " " code { (deployment) }
                            }
                        }
                    }
                }
            }
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
                    p id="live-containers-status" { "loading managed deployment pods from bastion" }
                }
                button id="live-containers-refresh" type="button" { "Refresh" }
            }
            table class="live-containers-table" {
                thead {
                    tr {
                        th style="width: 18%" { "Deployment" }
                        th style="width: 12%" { "Namespace" }
                        th style="width: 22%" { "Pod" }
                        th { "Containers" }
                        th style="width: 17%" { "Actions" }
                    }
                }
                // Each pod row gets a sibling expansion row that the JS
                // populates on demand with an inline terminal/logs panel.
                // The expansion row is rendered in-place beneath its pod
                // row, so opening a session pushes the rest of the table
                // down rather than docking it elsewhere on the page.
                tbody id="live-containers-body" {
                    tr {
                        td colspan="5" class="muted" {
                            "Loading live container inventory from " code { "/bastion/runtime/deployments" } "."
                        }
                    }
                }
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
    DeploymentRow { deployments: &["dd-des-simulator"], service: &["dd-des-simulator:8099"], service_note: None, access: SERVER_AUTH, notes: "Rust DES simulator with declared des.v1 schema, validation endpoint, async job status, and NATS result publishing." },
    DeploymentRow { deployments: &["dd-des-rs"], service: &["dd-des-rs:8112"], service_note: None, access: SERVER_AUTH, notes: "Rust DES engine deployment that imports the discrete-event-system.rs crate as a library (git submodule), runs its simulation catalogue in-process, and serves the rendered HTML/JSON results at /des-rs/." },
    DeploymentRow { deployments: &["dd-music-rs"], service: &["dd-music-rs:8115"], service_note: None, access: PUBLIC, notes: "Rust generative music shelf that renders daily DES music-engine WAV tracks, uploads published audio to S3, records anonymous votes in RDS Postgres, and uses Redis for generation locks plus vote throttling." },
    DeploymentRow { deployments: &["dd-contract-service"], service: &["dd-contract-service:8101"], service_note: None, access: SERVER_AUTH, notes: "Rust Solana contract gateway for solana.contract.v1 validation, signed transaction simulation, metrics, and NATS validation results." },
    DeploymentRow { deployments: &["dd-vpn"], service: &["dd-vpn-ui.vpn:51821"], service_note: None, access: VPN_PRIVATE, notes: "WireGuard wg-easy VPN server and private admin UI for split-tunnel access to the cluster service and pod CIDRs." },
    DeploymentRow { deployments: &["dd-live-mutex"], service: &["dd-live-mutex:6970"], service_note: None, access: CLUSTER_LOCAL, notes: "Singleton live-mutex broker deployment for TCP lock coordination." },
    DeploymentRow { deployments: &["dd-bastion"], service: &["dd-bastion.vpn:8111"], service_note: None, access: SERVER_AUTH, notes: "Rust bastion/jumphost access broker for VPN profile, kubeconfig export, managed deployment inventory, and browser exec terminals." },
    DeploymentRow { deployments: &["dd-redis-cache"], service: &["dd-redis-cache:6379"], service_note: None, access: CLUSTER_LOCAL, notes: "Ephemeral Redis cache deployment with bounded memory and Redis health probes." },
    DeploymentRow { deployments: &["dd-lock-loadtest-trigger"], service: &["dd-lock-loadtest-trigger:8110"], service_note: None, access: INTERNAL, notes: "Node.js HTTP trigger for live-mutex versus Redis aggregate lock load tests." },
    DeploymentRow { deployments: &["dd-trading-server"], service: &["dd-trading-server:8103"], service_note: None, access: SERVER_AUTH, notes: "Rust trading decision service for trading.decision.v1 scoring, scraper and AI/ML signals, MDP/POMDP policy hints, risk gates, and NATS order intents." },
    DeploymentRow { deployments: &["dd-fabrication-server"], service: &["dd-fabrication-server:8113"], service_note: None, access: SERVER_AUTH, notes: "Rust fabrication planner for 3D printers, CNC mills, routers, lathes, hybrid assemblies, instruction validation, and MDP/POMDP/neural policy learning hooks." },
    DeploymentRow { deployments: &["dd-compliance-rs"], service: &["dd-compliance-rs:8118"], service_note: None, access: SERVER_AUTH, notes: "Rust compliance readiness job server for artifacts, codebases, networks, systems, infra diagrams, PDF/Markdown reports, bounded vulnerability/malware/dependency/secret scans, and fraud/bot/login-anomaly detection across SOC 2, ISO, GDPR, PCI DSS, HIPAA, FedRAMP, CMMC, NIST, AI, quality, and ESG frameworks." },
    DeploymentRow { deployments: &["dd-container-pool"], service: &["dd-container-pool:8102"], service_note: None, access: SERVER_AUTH, notes: "Rust warm container pool service that loads runtime pool config from Postgres and starts local containerd workers through nerdctl." },
    DeploymentRow { deployments: &["headlamp"], service: &["headlamp.headlamp:80"], service_note: Some("(pod 4466)"), access: SERVER_AUTH, notes: "Kubernetes web UI served at /headlamp/. Use the headlamp-viewer service-account token for read-only pod, container, log, workload, Argo CD, KEDA, and External Secrets inspection." },
    DeploymentRow { deployments: &["dd-gleam-lambda-runner"], service: &["dd-gleam-lambda-runner:8083"], service_note: None, access: SERVER_AUTH, notes: "Gleam child-process runner deployment for POST /lambdas/invoke/<function-id>. It uses its own Argo CD app and dd-gleam-lambda-runner-secrets." },
    DeploymentRow { deployments: &["dd-remote-gateway"], service: &["dd-remote-gateway:80/443"], service_note: None, access: PUBLIC, notes: "nginx Ingress for the EC2 single-node cluster. Owns hostPort 80/443 and proxies every documented public/auth path into its in-cluster service." },
    DeploymentRow { deployments: &["dd-remote-web-home"], service: &["dd-remote-web-home:8080"], service_note: None, access: PUBLIC, notes: "This Rust service. Renders /, /home, /jello, /agents/tasks, /agents/threads, /lambdas/functions, /presence-test, and /wss-test; also exposes /healthz and /metrics." },
    DeploymentRow { deployments: &["dd-remote-auth"], service: &["dd-remote-auth:8083"], service_note: None, access: PUBLIC, notes: "Rust PIN auth service. Issues the short-lived dd_auth cookie that the gateway accepts in place of the legacy Auth header for browser sessions." },
    DeploymentRow { deployments: &["dd-remote-rest-api"], service: &["dd-remote-rest-api:8082"], service_note: None, access: SERVER_AUTH, notes: "Rust REST API boundary for RDS/Postgres-backed agent task data. Serves /api/agents/* and /api/lambdas/* JSON behind gateway auth." },
    DeploymentRow { deployments: &["dd-agent-worker-broker"], service: &["dd-agent-worker-broker:8098"], service_note: None, access: SERVER_AUTH, notes: "Rust NATS-first worker dispatch broker behind /api/agent-worker/. Handles wakeup and direct-if-awake handoff to the UUID-pinned worker." },
    DeploymentRow { deployments: &["dd-dev-server-api"], service: &["dd-dev-server-api:8080"], service_note: None, access: SERVER_AUTH, notes: "Bootstrap Node.js coding-agent task manager. Backs /tasks, /status, /agents, /healthz, and /stream/<taskId> until per-thread Ingress is the only path." },
    DeploymentRow { deployments: &["dd-remote-queue-consumer"], service: &["dd-remote-queue-consumer"], service_note: None, access: INTERNAL, notes: "Rust NATS shadow consumer. Reads dd.remote.thread.*.tasks, pins thread affinity, and prepares the matching UUID-bound worker; it does not execute prompts." },
    DeploymentRow { deployments: &["dd-idle-reaper"], service: &["dd-idle-reaper"], service_note: Some("(no http)"), access: INTERNAL, notes: "Rust maintenance supervisor: idle sweep, 90-minute cluster doctor loop, NATS watchdog, and the 04:00 ET worker-image rebuild for dd-dev-server:dev." },
    DeploymentRow { deployments: &["dd-billing-server"], service: &["dd-billing-server:80"], service_note: Some("(pod 8087)"), access: CLUSTER_LOCAL, notes: "Rust multi-tenant AR/AP ledger. Serves /v1/tenants/* billing/payable state, ledger primitives, provider connections, OAuth, webhooks, locks, scheduled jobs, and notifications. Not yet exposed through the public gateway." },
    DeploymentRow { deployments: &["dd-wal-gateway"], service: &["dd-wal-gateway:8104"], service_note: None, access: INTERNAL, notes: "Rust Postgres -> NATS JetStream CDC gateway. Owns one logical replication slot, publishes cdc.<schema>.<table>.<op> envelopes on stream CDC, and exposes /healthz, /readyz, /metrics." },
    DeploymentRow { deployments: &["dd-gleamlang-server"], service: &["dd-gleamlang-server:8081"], service_note: None, access: SERVER_AUTH, notes: "Gleam/OTP WebSocket fan-out behind /gleam/*. Exposes /gleam/home, /gleam/healthz, /gleam/metrics, and wss://<host>/gleam/ws." },
    DeploymentRow { deployments: &["presence"], service: &["presence-svc.presence:8081"], service_note: Some("(StatefulSet)"), access: SERVER_AUTH, notes: "Gleam gleamlang-presence-server behind /presence/*. Distributed-Erlang StatefulSet that powers user-scoped and conv-scoped websockets driving the /presence-test browser harness." },
    DeploymentRow { deployments: &["dd-cluster-mcp-rs"], service: &["dd-cluster-mcp-rs:8091"], service_note: None, access: SERVER_AUTH, notes: "Rust JSON-RPC MCP service behind /cluster-mcp and /cluster-mcp/*. Ships read-only cluster inventory, service wiring, telemetry tools, Prometheus metrics, dd.log.v1 stdout, and OTLP spans." },
    DeploymentRow { deployments: &["dd-gleam-mcp-server"], service: &["dd-gleam-mcp-server:8090"], service_note: None, access: SERVER_AUTH, notes: "Legacy Gleam JSON-RPC MCP service behind /mcp and /mcp/*. Ships read-only runtime tools, Prometheus metrics, and Loki-collected stdout." },
    DeploymentRow { deployments: &["dd-webrtc-signaling"], service: &["dd-webrtc-signaling:8095"], service_note: None, access: SERVER_AUTH, notes: "Rust WebRTC signaling service behind /webrtc/. Room WebSocket signaling for browser/mobile peer handshakes; media and data channels stay peer-to-peer." },
    DeploymentRow { deployments: &["dd-webrtc-media"], service: &["dd-webrtc-media:8125"], service_note: None, access: SERVER_AUTH, notes: "Rust WebRTC media-plane config service behind /webrtc-media/. Publishes STUN/TURN ICE metadata and optional SFU/media-relay endpoints; UDP media still requires a separate data-plane deployment." },
    DeploymentRow { deployments: &["dd-mdp-optimizer"], service: &["dd-mdp-optimizer:8096"], service_note: None, access: SERVER_AUTH, notes: "Rust MDP/POMDP/RL optimizer behind /mdp/. Consumes dd.remote.mdp.optimize and dd.remote.telemetry.mdp." },
    DeploymentRow { deployments: &["dd-akka-ws-server"], service: &["dd-akka-ws-server:8086"], service_note: None, access: INTERNAL, notes: "Scala/Akka WebSocket reference server backing the akka-streams and async-java load-test targets." },
    DeploymentRow { deployments: &["dd-fsharp-ws-server"], service: &["dd-fsharp-ws-server:8087"], service_note: None, access: SERVER_AUTH, notes: "F# + ASP.NET Core WebSocket server behind /fsws/. Exposes /fsws/healthz, /fsws/livez, /fsws/ws/rx, and /fsws/ws/async." },
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
    PathRow { paths: &[PathEntry { label: "/api-docs", href: Some("/api-docs") }, PathEntry { label: "/api-docs.json", href: Some("/api-docs.json") }, PathEntry { label: "/docs/api", href: Some("/docs/api") }, PathEntry { label: "/api/docs", href: Some("/api/docs") }], target: "Generated API documentation", access: PUBLIC, notes: "Central generated index plus this deployment's standard generated docs endpoints. Each HTTP API deployment also serves /docs/api, /api/docs, and /api/docs.json on its own service port." },
    PathRow { paths: &[PathEntry { label: "/jello", href: Some("/jello") }, PathEntry { label: "/jello/sample", href: Some("/jello/sample?product=athlet") }], target: "Athlet-O performance gelatin storefront", access: PUBLIC, notes: "Brand/product concept page for protein gelatin cups with fiber, vitamin C, electrolytes, probiotics, stevia, retailer search links, and htmx sample-pack fragments." },
    PathRow { paths: &[PathEntry { label: "/music/", href: Some("/music/") }, PathEntry { label: "/music/songs", href: Some("/music/songs") }, PathEntry { label: "/music/docs/api", href: Some("/music/docs/api") }, PathEntry { label: "POST /music/songs/<song_id>/votes", href: None }], target: "dd-music-rs generative music shelf", access: PUBLIC, notes: "Native browser audio player for daily generated tracks. The service publishes curated WAV files to S3, stores song/vote state in Postgres, and keeps generation coordination in Redis. Manual generation stays behind /music/internal/generate." },
    PathRow { paths: &[PathEntry { label: "/factmachine-markets", href: Some("/factmachine-markets") }], target: "FactMachine MDP/POMDP market simulation", access: PUBLIC, notes: "Single generated HTML artifact with a 50-day multi-market simulation, time-step controls, and scalar/binary/scale comparison plots." },
    PathRow { paths: &[PathEntry { label: "/tasks", href: Some("/tasks") }, PathEntry { label: "/status", href: Some("/status") }, PathEntry { label: "/stream/<uuid>", href: Some("/stream/example-task-id") }], target: "Node.js Coding Agent Task Manager", access: SERVER_AUTH, notes: "Runs inside the already-selected worker container. It executes prompts, tracks taskIds, streams events, and rejects requests for the wrong pinned thread." },
    PathRow { paths: &[PathEntry { label: "/api/agents/tasks", href: Some("/api/agents/tasks") }, PathEntry { label: "/api/agents/threads/<uuid>/context", href: Some("/api/agents/threads/example-thread-id/context") }], target: "Rust REST API (JSON only)", access: SERVER_AUTH, notes: "JSON-only boundary for task snapshots and thread context. The browser UI lives at /agents/tasks and uses the dd_auth cookie for same-origin API reads." },
    PathRow { paths: &[PathEntry { label: "/lambdas/functions", href: Some("/lambdas/functions") }, PathEntry { label: "/api/lambdas/functions", href: Some("/api/lambdas/functions") }, PathEntry { label: "POST /lambdas/invoke/<function-id>", href: Some("/lambdas/invoke/00000000-0000-0000-0000-000000000000") }], target: "dd-gleam-lambda-runner deployment + Rust REST API", access: SERVER_AUTH, notes: "CRUD/read models stay in the REST API. Invocation traffic is routed directly by the gateway to the Gleam child-process runner." },
    PathRow { paths: &[PathEntry { label: "/presence-test", href: Some("/presence-test?user=alice&device=d1") }], target: "gleamlang-presence-server browser harness", access: PUBLIC, notes: "Self-contained page for one user-scoped ws plus N conv-scoped ws connections against the presence server. Operators can opt into connection attempts from the page after the presence deployment is installed." },
    PathRow { paths: &[PathEntry { label: "/presence/healthz", href: None }, PathEntry { label: "/presence/ws", href: None }, PathEntry { label: "/presence/user/<id>/broadcast", href: None }], target: "gleamlang-presence-server gateway proxy", access: SERVER_AUTH, notes: "Authenticated same-origin proxy reserved for the presence lab. It is intentionally not linked as a finite page until the presence Argo app and image are deployed." },
    PathRow { paths: &[PathEntry { label: "/wss-test", href: Some("/wss-test") }, PathEntry { label: "?preset=gleam", href: Some("/wss-test?preset=gleam") }, PathEntry { label: "?preset=webrtc", href: Some("/wss-test?preset=webrtc") }, PathEntry { label: "?preset=gcs", href: Some("/wss-test?preset=gcs") }, PathEntry { label: "?preset=fsrx", href: Some("/wss-test?preset=fsrx") }], target: "Gateway WebSocket test lab", access: PUBLIC, notes: "Rust-served browser harness. Preset health checks and sockets use gateway-authenticated upstream routes when they leave the public page." },
    PathRow { paths: &[PathEntry { label: "/auth", href: Some("/auth?return=/home") }, PathEntry { label: "/auth/status", href: Some("/auth/status") }], target: "dd-remote-auth Rust PIN auth", access: PUBLIC, notes: "Sets the temporary dd_auth cookie so the gateway can accept browser sessions without the legacy Auth header." },
    PathRow { paths: &[PathEntry { label: "/bastion/runtime/deployments", href: Some("/bastion/runtime/deployments") }, PathEntry { label: "/bastion/profile", href: Some("/bastion/profile") }, PathEntry { label: "/bastion/terminal", href: None }], target: "Rust bastion/jumphost access broker", access: SERVER_AUTH, notes: "Same-origin gateway access to bastion inventory and allowlisted browser exec terminals." },
    PathRow { paths: &[PathEntry { label: "/headlamp/", href: Some("/headlamp/") }], target: "Headlamp Kubernetes UI", access: SERVER_AUTH, notes: "Read-only cluster browser for workload, pod, container, logs, node, Argo CD, KEDA, and External Secrets state. Paste a token from `kubectl -n headlamp create token headlamp-viewer`." },
    PathRow { paths: &[PathEntry { label: THREAD_TASKS_WILDCARD, href: None }, PathEntry { label: "POST /api/agents/threads/<uuid>/prepare", href: Some("/api/agents/threads/example-thread-id/prepare") }], target: "Rust NATS Queue Consumer", access: INTERNAL_ACCESS, notes: "Queued consumer reads task.dispatch messages, routes repo-matched work into warm container pools, and falls back to the UUID-bound worker when needed. Legacy shadow messages only prepare workers." },
    PathRow { paths: &[PathEntry { label: "/dd-thread/<short>", href: Some("/dd-thread/example") }, PathEntry { label: "/dd-thread/<short>/tasks", href: Some("/dd-thread/example/tasks") }, PathEntry { label: "/dd-thread/<short>/stream/<taskId>", href: Some("/dd-thread/example/stream/example-task-id") }, PathEntry { label: "/dd-thread/<short>/ws", href: Some("/dd-thread/example/ws") }], target: "Kubernetes per-thread Ingress", access: SERVER_AUTH, notes: "Ingress selects the UUID-bound worker Service; Node.js handles only the task inside that selected container." },
    PathRow { paths: &[PathEntry { label: "/gleam/home", href: Some("/gleam/home") }, PathEntry { label: "/gleam/healthz", href: Some("/gleam/healthz") }, PathEntry { label: "/gleam/metrics", href: Some("/gleam/metrics") }, PathEntry { label: "/gleam/ws", href: None }], target: "Gleam WebSocket service", access: INTERNAL_ACCESS, notes: "Gleam/OTP fan-out socket behind the gateway; WebSocket endpoint is wss://<host>/gleam/ws." },
    PathRow { paths: &[PathEntry { label: "/cluster-mcp", href: Some("/cluster-mcp") }, PathEntry { label: "/cluster-mcp/home", href: Some("/cluster-mcp/home") }, PathEntry { label: "/cluster-mcp/healthz", href: Some("/cluster-mcp/healthz") }, PathEntry { label: "/cluster-mcp/metrics", href: Some("/cluster-mcp/metrics") }], target: "Rust cluster MCP service", access: INTERNAL_ACCESS, notes: "Primary dd_cluster MCP deployment with read-only Kubernetes inventory, service discovery, observability, Prometheus metrics, dd.log.v1 stdout logs, and explicit OTLP spans." },
    PathRow { paths: &[PathEntry { label: "/mcp", href: Some("/mcp") }, PathEntry { label: "/mcp/home", href: Some("/mcp/home") }, PathEntry { label: "/mcp/healthz", href: Some("/mcp/healthz") }, PathEntry { label: "/mcp/metrics", href: Some("/mcp/metrics") }], target: "Legacy Gleam MCP service", access: INTERNAL_ACCESS, notes: "Legacy dedicated MCP deployment with read-only runtime tools, Prometheus metrics, and Loki-collected stdout logs." },
    PathRow { paths: &[PathEntry { label: "/webrtc/", href: Some("/webrtc/") }, PathEntry { label: "/webrtc/healthz", href: Some("/webrtc/healthz") }, PathEntry { label: "/webrtc/metrics", href: Some("/webrtc/metrics") }, PathEntry { label: "/webrtc/signal test", href: Some("/wss-test?preset=webrtc") }], target: "Rust WebRTC signaling service", access: SERVER_AUTH, notes: "Room WebSocket signaling for browser/mobile peer handshakes. Media and data channels stay peer-to-peer. The gateway requires the operator Auth header or dd_auth cookie before forwarding." },
    PathRow { paths: &[PathEntry { label: "/webrtc-media/", href: Some("/webrtc-media/") }, PathEntry { label: "/webrtc-media/config", href: Some("/webrtc-media/config") }, PathEntry { label: "/webrtc-media/ice", href: Some("/webrtc-media/ice") }, PathEntry { label: "/webrtc-media/metrics", href: Some("/webrtc-media/metrics") }], target: "Rust WebRTC media config service", access: SERVER_AUTH, notes: "Advertises ICE servers plus optional TURN/SFU/media-relay metadata. The gateway carries only HTTP config; media UDP/TCP paths require a separate public data-plane route." },
    PathRow { paths: &[PathEntry { label: "/mdp/", href: Some("/mdp/") }, PathEntry { label: "/mdp/healthz", href: Some("/mdp/healthz") }, PathEntry { label: "/mdp/metrics", href: Some("/mdp/metrics") }, PathEntry { label: "POST /mdp/optimize", href: Some("/mdp/optimize") }, PathEntry { label: "POST /mdp/telemetry/learn", href: Some("/mdp/telemetry/learn") }, PathEntry { label: MDP_OPTIMIZE_SUBJECT, href: None }, PathEntry { label: TELEMETRY_MDP_SUBJECT, href: None }], target: "Rust MDP/POMDP optimizer", access: SERVER_AUTH, notes: "Async optimizer behind the authenticated gateway; it subscribes to NATS optimization and telemetry jobs, then publishes runtime events." },
    PathRow { paths: &[PathEntry { label: "/fabrication/", href: Some("/fabrication/") }, PathEntry { label: "/fabrication/healthz", href: Some("/fabrication/healthz") }, PathEntry { label: "/fabrication/metrics", href: Some("/fabrication/metrics") }, PathEntry { label: "/fabrication/docs/api", href: Some("/fabrication/docs/api") }, PathEntry { label: "/fabrication/jobs", href: Some("/fabrication/jobs") }, PathEntry { label: "/grafana/fabrication", href: Some("/grafana/fabrication") }, PathEntry { label: "POST /fabrication/plan", href: Some("/fabrication/plan") }, PathEntry { label: "POST /fabrication/instructions/analyze", href: Some("/fabrication/instructions/analyze") }, PathEntry { label: FABRICATION_REQUESTS_SUBJECT, href: None }, PathEntry { label: FABRICATION_RESULTS_SUBJECT, href: None }], target: "Rust fabrication planner", access: SERVER_AUTH, notes: "Hybrid additive/subtractive/turning planner with draft machine programs, instruction analysis, failure-boundary detection, artifact inspection, and optional MDP optimizer publication." },
    PathRow { paths: &[PathEntry { label: "/des/", href: Some("/des/") }, PathEntry { label: "/des/healthz", href: Some("/des/healthz") }, PathEntry { label: "/des/metrics", href: Some("/des/metrics") }, PathEntry { label: "/des/model/schema", href: Some("/des/model/schema") }, PathEntry { label: "/des/model/example", href: Some("/des/model/example") }, PathEntry { label: "POST /des/validate", href: Some("/des/validate") }, PathEntry { label: "POST /des/simulate", href: Some("/des/simulate") }, PathEntry { label: DES_SIMULATE_SUBJECT, href: None }], target: "Rust discrete event simulator", access: SERVER_AUTH, notes: "Async DES job runner behind the authenticated gateway, with declared des.v1 schema, strict validation, in-memory job status, metrics, and NATS result publishing." },
    PathRow { paths: &[PathEntry { label: "/des-rs/", href: Some("/des-rs/") }, PathEntry { label: "/des-rs/info", href: Some("/des-rs/info") }, PathEntry { label: "/des-rs/simulations", href: Some("/des-rs/simulations") }, PathEntry { label: "POST /des-rs/simulate", href: Some("/des-rs/simulate") }, PathEntry { label: "/des-rs/out/", href: Some("/des-rs/out/") }, PathEntry { label: "/des-rs/out/soccer-sim.html", href: Some("/des-rs/out/soccer-sim.html") }, PathEntry { label: "/des-rs/healthz", href: Some("/des-rs/healthz") }, PathEntry { label: "/des-rs/docs/api", href: Some("/des-rs/docs/api") }, PathEntry { label: "/des-rs/api/docs.json", href: Some("/des-rs/api/docs.json") }], target: "Rust DES engine (library) result pages", access: SERVER_AUTH, notes: "Landing page with per-simulation run buttons that execute the discrete-event-system.rs engine (git submodule) in-process and serve its rendered HTML/JSON result pages, including the 2D 11v11 soccer videogame artifact. Ships a canonical machine-readable service descriptor at /api/docs.json (built JSON-first by the engine's des::service module), an HTML view at /docs/api, and RFC 8288 service-doc/service-desc discovery headers on / and /info." },
    PathRow { paths: &[PathEntry { label: "/contracts/", href: Some("/contracts/") }, PathEntry { label: "/contracts/healthz", href: Some("/contracts/healthz") }, PathEntry { label: "/contracts/metrics", href: Some("/contracts/metrics") }, PathEntry { label: "/contracts/schema", href: Some("/contracts/schema") }, PathEntry { label: "/contracts/example", href: Some("/contracts/example") }, PathEntry { label: "POST /contracts/validate", href: Some("/contracts/validate") }, PathEntry { label: "POST /contracts/simulate", href: Some("/contracts/simulate") }, PathEntry { label: CONTRACTS_SOLANA_VALIDATE_SUBJECT, href: None }], target: "Rust Solana contract service", access: SERVER_AUTH, notes: "Validates solana.contract.v1 instruction envelopes, proxies signed simulation through Solana JSON-RPC, and publishes NATS validation results." },
    PathRow { paths: &[PathEntry { label: "/compliance/", href: Some("/compliance/") }, PathEntry { label: "/compliance/standards", href: Some("/compliance/standards") }, PathEntry { label: "/compliance/controls", href: Some("/compliance/controls") }, PathEntry { label: "/compliance/example", href: Some("/compliance/example") }, PathEntry { label: "/compliance/diagrams/example", href: Some("/compliance/diagrams/example") }, PathEntry { label: "/compliance/reports/example", href: Some("/compliance/reports/example") }, PathEntry { label: "/compliance/vulnerability-scan/example", href: Some("/compliance/vulnerability-scan/example") }, PathEntry { label: "/compliance/docs/api", href: Some("/compliance/docs/api") }, PathEntry { label: "POST /compliance/audits", href: Some("/compliance/audits") }, PathEntry { label: "POST /compliance/audit-sync", href: Some("/compliance/audit-sync") }, PathEntry { label: "POST /compliance/diagrams/infrastructure", href: Some("/compliance/diagrams/infrastructure") }, PathEntry { label: "POST /compliance/reports/system", href: Some("/compliance/reports/system") }, PathEntry { label: "POST /compliance/vulnerability-scan", href: Some("/compliance/vulnerability-scan") }, PathEntry { label: "POST /compliance/malware-scan", href: Some("/compliance/malware-scan/example") }, PathEntry { label: "POST /compliance/dependency-audit", href: Some("/compliance/dependency-audit/example") }, PathEntry { label: "POST /compliance/secret-scan", href: Some("/compliance/secret-scan/example") }, PathEntry { label: "POST /compliance/fraud-detection", href: Some("/compliance/fraud-detection/example") }, PathEntry { label: "POST /compliance/bot-detection", href: Some("/compliance/bot-detection/example") }, PathEntry { label: "POST /compliance/login-anomaly", href: Some("/compliance/login-anomaly/example") }], target: "Rust compliance readiness server", access: SERVER_AUTH, notes: "Runs bounded evidence-readiness audit jobs, infra parity diagrams, Markdown/PDF system reports, static vulnerability scans, plus malware/dependency/secret scanners and fraud/bot/login-anomaly detectors for artifacts, codebases, networks, and systems across global compliance frameworks." },
    PathRow { paths: &[PathEntry { label: "/ml/", href: Some("/ml/") }, PathEntry { label: "/ml/healthz", href: Some("/ml/healthz") }, PathEntry { label: "/ml/metrics", href: Some("/ml/metrics") }, PathEntry { label: "/ml/status", href: Some("/ml/status") }, PathEntry { label: "POST /ml/analyze", href: Some("/ml/analyze") }, PathEntry { label: "POST /ml/ingest", href: Some("/ml/ingest") }, PathEntry { label: TELEMETRY_RAW_SUBJECT, href: None }, PathEntry { label: ML_FEATURES_SUBJECT, href: None }], target: "Python AI/ML feature pipeline", access: SERVER_AUTH, notes: "Normalizes runtime telemetry into features, EWMA baselines, z-score anomalies, transition estimates, and MDP telemetry requests." },
    PathRow { paths: &[PathEntry { label: "/trading/", href: Some("/trading/") }, PathEntry { label: "/trading/healthz", href: Some("/trading/healthz") }, PathEntry { label: "/trading/metrics", href: Some("/trading/metrics") }, PathEntry { label: "/trading/schema", href: Some("/trading/schema") }, PathEntry { label: "/trading/example", href: Some("/trading/example") }, PathEntry { label: "POST /trading/decide", href: Some("/trading/decide") }, PathEntry { label: TRADING_SIGNALS_SUBJECT, href: None }, PathEntry { label: TRADING_ORDER_INTENTS_SUBJECT, href: None }], target: "Rust trading decision service", access: SERVER_AUTH, notes: "Combines scraped web sentiment, AI/ML features, market snapshots, and MDP/POMDP hints into risk-gated buy/sell/hold decisions." },
    PathRow { paths: &[PathEntry { label: "POST /scrape", href: Some("/scrape") }, PathEntry { label: "/scrape/strategies", href: Some("/scrape/strategies") }, PathEntry { label: "/scrape/healthz", href: Some("/scrape/healthz") }, PathEntry { label: "/scrape/metrics", href: Some("/scrape/metrics") }], target: "dd-web-scraper Fastify deployment", access: SERVER_AUTH, notes: "Long-running strategy router for native fetch, Cheerio, JSDOM, LinkeDOM, Playwright, Puppeteer, and Browserless scraping." },
    PathRow { paths: &[PathEntry { label: "POST /builds", href: Some("/builds") }, PathEntry { label: "/builds/<jobId>", href: Some("/builds/example-job") }, PathEntry { label: "/builds/<jobId>/logs", href: Some("/builds/example-job/logs") }], target: "dd-build-server Rust CI/CD deployment", access: SERVER_AUTH, notes: "Authenticated repo build queue. Jobs are build-server.v1 JSON, push only to allowlisted ECR prefixes, and deploy only allowlisted manifests/namespaces." },
    PathRow { paths: &[PathEntry { label: "/telemetry/", href: Some("/telemetry/") }, PathEntry { label: "/grafana/observability", href: Some("/grafana/observability") }, PathEntry { label: "/grafana/fabrication", href: Some("/grafana/fabrication") }], target: "Grafana", access: INTERNAL_ACCESS, notes: "Primary HTML dashboards for Prometheus metrics, Loki logs, Tempo traces, NATS metrics, the observability control plane, and the Rust fabrication planner." },
    PathRow { paths: &[PathEntry { label: "/grafana/depl/<deployment>", href: Some("/grafana/depl/dd-remote-web-home") }, PathEntry { label: "/grafana/depl/dd-dart-server", href: Some("/grafana/depl/dd-dart-server") }, PathEntry { label: "/grafana/depl/des-rs", href: Some("/grafana/depl/des-rs") }], target: "Grafana deployment drilldown", access: INTERNAL_ACCESS, notes: "Rust web-home redirect into the canonical per-deployment Grafana page, backed by Kubernetes resource metrics plus Loki logs." },
    PathRow { paths: &[PathEntry { label: "/prometheus/", href: Some("/prometheus/") }], target: "Prometheus", access: INTERNAL_ACCESS, notes: "Low-level metrics UI and query surface." },
    PathRow { paths: &[PathEntry { label: "/nats/", href: Some("/nats/") }, PathEntry { label: "/nats-metrics/metrics", href: Some("/nats-metrics/metrics") }], target: "NATS monitor and exporter", access: INTERNAL_ACCESS, notes: "NATS should usually be inspected through Grafana; these paths expose raw health and metrics." },
    PathRow { paths: &[PathEntry { label: "/reaper/", href: Some("/reaper/") }, PathEntry { label: "/cron/", href: Some("/cron/") }], target: "Runtime service status", access: INTERNAL_ACCESS, notes: "Gateway status surfaces for idle reaper and cron scheduler deployments." },
    PathRow { paths: &[PathEntry { label: "/fsws/", href: Some("/fsws/") }, PathEntry { label: "/fsws/healthz", href: Some("/fsws/healthz") }, PathEntry { label: "/fsws/livez", href: Some("/fsws/livez") }, PathEntry { label: "/fsws/ws/rx", href: None }, PathEntry { label: "/fsws/ws/async", href: None }, PathEntry { label: "/wss-test?preset=fsrx", href: Some("/wss-test?preset=fsrx") }], target: "dd-fsharp-ws-server", access: SERVER_AUTH, notes: "F# + ASP.NET Core burst WebSocket server. The authenticated gateway strips the /fsws/ prefix before proxying to the upstream." },
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
}
.shell { max-width: 1180px; margin: 0 auto; padding: 24px; }
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
.grafana-links {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  margin-top: 7px;
  font-size: 12px;
}
.grafana-links a {
  color: var(--accent);
}
.grafana-links code {
  font-size: 11px;
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
.metrics-line {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  align-items: center;
  font-size: 12px;
  color: var(--muted);
}
.metric-chip {
  display: inline-flex;
  gap: 4px;
  align-items: baseline;
  padding: 2px 6px;
  border: 1px solid var(--line);
  border-radius: 4px;
  background: rgba(94, 234, 212, 0.06);
  color: var(--text);
  font-variant-numeric: tabular-nums;
  font-size: 11px;
}
.metric-chip span.label {
  color: var(--muted);
  font-size: 10px;
  text-transform: uppercase;
  letter-spacing: 0.04em;
}
.metric-chip.metric-warn {
  border-color: rgba(251, 191, 36, 0.35);
  background: rgba(251, 191, 36, 0.08);
  color: var(--warn);
}
.container-actions {
  display: flex;
  flex-direction: column;
  gap: 6px;
}
.container-actions .row {
  display: flex;
  gap: 6px;
  flex-wrap: wrap;
  align-items: center;
}
.container-actions .row > .name {
  font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
  font-size: 12px;
  color: var(--muted);
  margin-right: 4px;
}
button.action-btn {
  padding: 4px 8px;
  font-size: 11px;
}
button.action-btn.active {
  border-color: rgba(94, 234, 212, 0.72);
  background: rgba(94, 234, 212, 0.18);
  color: var(--accent);
}
tr.inline-panel-row > td {
  padding: 0;
  background: var(--panel-2, #0f1720);
  border-top: 1px dashed var(--line);
}
.inline-panel {
  display: grid;
  gap: 8px;
  padding: 12px;
}
.inline-panel-head {
  display: flex;
  justify-content: space-between;
  gap: 10px;
  align-items: center;
}
.inline-panel-head h3 {
  margin: 0;
  font-size: 13px;
  color: var(--accent);
  font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
}
.inline-panel-head p {
  margin: 0;
  color: var(--muted);
  font-size: 11px;
}
.inline-panel iframe {
  width: 100%;
  height: 460px;
  border: 1px solid var(--line);
  border-radius: 8px;
  background: #05080d;
}
.inline-panel pre.logs {
  margin: 0;
  width: 100%;
  height: 460px;
  overflow: auto;
  border: 1px solid var(--line);
  border-radius: 8px;
  background: #05080d;
  color: #d5f5e3;
  padding: 10px 12px;
  font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
  font-size: 12px;
  line-height: 1.45;
  white-space: pre-wrap;
  word-break: break-word;
}
ol {
  margin: 8px 0 0;
  padding-left: 22px;
  color: var(--muted);
  line-height: 1.55;
}
@media (max-width: 880px) {
  .shell { padding: 14px; }
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
  const runtimeReloadIntervalMs = 30000;
  let inventoryStatus = "loading managed deployment pods from bastion";
  let lastUpdatedAt = "";
  let loading = false;
  let reloadTimer = 0;
  let liveInventoryEnabled = false;
  let runtimeSocketsStarted = false;
  let metricsAvailable = false;
  // Active inline panels keyed by `${podKey}::${kind}::${container}` so the
  // open panels survive table re-renders triggered by the 30s reload.
  const inlinePanels = new Map();
  const wsStatus = { gleam: "idle", rust: "idle" };
  const renderStatus = () => {
    const updated = lastUpdatedAt ? ` · updated ${lastUpdatedAt}` : "";
    const metrics = metricsAvailable ? "metrics ok" : "metrics unavailable";
    status.textContent = `${inventoryStatus}${updated} · ${metrics} · gleam ws ${wsStatus.gleam} · rust ws ${wsStatus.rust}`;
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
  const formatCpu = (millicores) => {
    if (millicores == null) return "";
    if (millicores < 1) return "0m";
    if (millicores < 1000) return `${millicores}m`;
    return `${(millicores / 1000).toFixed(2)} cores`;
  };
  const formatMemory = (bytes) => {
    if (bytes == null) return "";
    const kib = bytes / 1024;
    if (kib < 1024) return `${kib.toFixed(0)} KiB`;
    const mib = kib / 1024;
    if (mib < 1024) return `${mib.toFixed(1)} MiB`;
    const gib = mib / 1024;
    return `${gib.toFixed(2)} GiB`;
  };
  const metricChip = (label, value, warn) => {
    if (value === "") return null;
    const el = document.createElement("span");
    el.className = warn ? "metric-chip metric-warn" : "metric-chip";
    const labelEl = document.createElement("span");
    labelEl.className = "label";
    labelEl.textContent = label;
    el.append(labelEl, document.createTextNode(value));
    return el;
  };
  const metricsLine = (metrics) => {
    if (!metrics) return null;
    const wrap = document.createElement("span");
    wrap.className = "metrics-line";
    const cpu = metricChip("cpu", formatCpu(metrics.cpuMillicores ?? 0));
    const mem = metricChip("mem", formatMemory(metrics.memoryBytes ?? 0));
    if (cpu) wrap.appendChild(cpu);
    if (mem) wrap.appendChild(mem);
    return wrap.children.length ? wrap : null;
  };
  const safeBastionUrl = (value, expectedPath) => {
    try {
      const url = new URL(String(value || ""), window.location.origin);
      if (url.origin !== window.location.origin || url.pathname !== expectedPath) return "";
      for (const key of ["namespace", "deployment", "pod", "container"]) {
        if (!url.searchParams.get(key)) return "";
      }
      return `${url.pathname}${url.search}`;
    } catch {
      return "";
    }
  };
  const safeBastionTerminalUrl = (value) => safeBastionUrl(value, "/bastion/terminal");
  const safeBastionLogsUrl = (value) => safeBastionUrl(value, "/bastion/logs/ws");
  const podKey = (deployment, pod) => `${deployment.namespace}/${deployment.deployment}/${pod.name}`;
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
  const closeInlinePanel = (panelKey) => {
    const tracked = inlinePanels.get(panelKey);
    if (!tracked) return;
    const { row, button, cleanup } = tracked;
    if (cleanup) {
      try { cleanup(); } catch (_error) {}
    }
    if (row && row.parentNode) row.parentNode.removeChild(row);
    if (button) button.classList.remove("active");
    inlinePanels.delete(panelKey);
  };
  const closeAllInlinePanels = () => {
    for (const key of Array.from(inlinePanels.keys())) closeInlinePanel(key);
  };
  const openTerminalPanel = (anchorRow, deployment, pod, container, url, button) => {
    const panelKey = `${podKey(deployment, pod)}::terminal::${container.name}`;
    if (inlinePanels.has(panelKey)) {
      closeInlinePanel(panelKey);
      return;
    }
    const tr = document.createElement("tr");
    tr.className = "inline-panel-row";
    const td = document.createElement("td");
    td.colSpan = 5;
    const wrap = document.createElement("div");
    wrap.className = "inline-panel";
    const head = document.createElement("div");
    head.className = "inline-panel-head";
    const title = document.createElement("div");
    const h3 = document.createElement("h3");
    h3.textContent = `${deployment.namespace}/${pod.name}/${container.name} terminal`;
    const sub = document.createElement("p");
    sub.textContent = "Bastion exec session · runs as the in-cluster service account.";
    title.append(h3, sub);
    const closeBtn = document.createElement("button");
    closeBtn.type = "button";
    closeBtn.textContent = "Close";
    closeBtn.addEventListener("click", () => closeInlinePanel(panelKey));
    head.append(title, closeBtn);
    const iframe = document.createElement("iframe");
    iframe.title = "Bastion container terminal";
    iframe.src = url;
    wrap.append(head, iframe);
    td.appendChild(wrap);
    tr.appendChild(td);
    anchorRow.parentNode.insertBefore(tr, anchorRow.nextSibling);
    button.classList.add("active");
    inlinePanels.set(panelKey, {
      row: tr,
      button,
      cleanup: () => { iframe.src = "about:blank"; },
    });
  };
  const openLogsPanel = (anchorRow, deployment, pod, container, url, button) => {
    const panelKey = `${podKey(deployment, pod)}::logs::${container.name}`;
    if (inlinePanels.has(panelKey)) {
      closeInlinePanel(panelKey);
      return;
    }
    const tr = document.createElement("tr");
    tr.className = "inline-panel-row";
    const td = document.createElement("td");
    td.colSpan = 5;
    const wrap = document.createElement("div");
    wrap.className = "inline-panel";
    const head = document.createElement("div");
    head.className = "inline-panel-head";
    const title = document.createElement("div");
    const h3 = document.createElement("h3");
    h3.textContent = `${deployment.namespace}/${pod.name}/${container.name} logs`;
    const sub = document.createElement("p");
    sub.textContent = "kubectl logs -f --tail=500 (live stream)";
    title.append(h3, sub);
    const closeBtn = document.createElement("button");
    closeBtn.type = "button";
    closeBtn.textContent = "Close";
    closeBtn.addEventListener("click", () => closeInlinePanel(panelKey));
    head.append(title, closeBtn);
    const pre = document.createElement("pre");
    pre.className = "logs";
    wrap.append(head, pre);
    td.appendChild(wrap);
    tr.appendChild(td);
    anchorRow.parentNode.insertBefore(tr, anchorRow.nextSibling);
    button.classList.add("active");
    const protocol = window.location.protocol === "https:" ? "wss" : "ws";
    const ws = new WebSocket(`${protocol}://${window.location.host}${url}`);
    const append = (line) => {
      const atBottom = pre.scrollTop + pre.clientHeight >= pre.scrollHeight - 8;
      pre.appendChild(document.createTextNode(line));
      if (atBottom) pre.scrollTop = pre.scrollHeight;
    };
    ws.onopen = () => append("[connected]\n");
    ws.onmessage = (event) => {
      let parsed;
      try { parsed = JSON.parse(event.data); } catch { append(String(event.data)); return; }
      if (parsed.type === "logs-output") append(String(parsed.data || ""));
      else if (parsed.type === "logs-status") append(`[${parsed.status || "status"}]\n`);
      else if (parsed.type === "logs-error") append(`[error] ${parsed.message || "logs error"}\n`);
      else if (parsed.type === "logs-exit") append(`[exit code=${parsed.code} signal=${parsed.signal || "none"}]\n`);
    };
    ws.onerror = () => append("[connection error]\n");
    ws.onclose = () => append("[disconnected]\n");
    inlinePanels.set(panelKey, {
      row: tr,
      button,
      cleanup: () => { try { ws.close(1000, "panel closed"); } catch (_error) {} },
    });
  };
  const render = (data) => {
    metricsAvailable = !!data.metricsAvailable;
    closeAllInlinePanels();
    body.textContent = "";
    let rowCount = 0;
    let containerCount = 0;
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
        containerCount += containers.length;
        const containerCell = document.createElement("div");
        containerCell.className = "container-cell";
        const actionsCell = document.createElement("div");
        actionsCell.className = "container-actions";
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
          const containerMetrics = metricsLine(container.metrics);
          if (containerMetrics) item.appendChild(containerMetrics);
          containerCell.appendChild(item);

          const actionRow = document.createElement("div");
          actionRow.className = "row";
          const actionLabel = document.createElement("span");
          actionLabel.className = "name";
          actionLabel.textContent = container.name;
          actionRow.appendChild(actionLabel);

          const safeTerminalUrl = safeBastionTerminalUrl(container.terminalUrl);
          const termBtn = document.createElement("button");
          termBtn.type = "button";
          termBtn.className = "action-btn";
          termBtn.textContent = "Terminal";
          termBtn.disabled = !safeTerminalUrl || !data.terminalEnabled;
          termBtn.title = termBtn.disabled
            ? "terminal unavailable (set BASTION_TERMINAL_ENABLED=true and the dd-bastion-exec ClusterRoleBinding)"
            : "Open inline bastion exec terminal";
          actionRow.appendChild(termBtn);

          const safeLogsUrl = safeBastionLogsUrl(container.logsUrl);
          const logsBtn = document.createElement("button");
          logsBtn.type = "button";
          logsBtn.className = "action-btn";
          logsBtn.textContent = "Logs";
          logsBtn.disabled = !safeLogsUrl;
          logsBtn.title = logsBtn.disabled
            ? "logs unavailable"
            : "Open inline kubectl logs -f stream";
          actionRow.appendChild(logsBtn);

          actionsCell.appendChild(actionRow);

          // Wire the click handlers after the pod row is appended so we can
          // pass the actual <tr> as the insertion anchor.
          termBtn.addEventListener("click", () => {
            if (!safeTerminalUrl || !data.terminalEnabled) return;
            openTerminalPanel(podRow, deployment, pod, container, safeTerminalUrl, termBtn);
          });
          logsBtn.addEventListener("click", () => {
            if (!safeLogsUrl) return;
            openLogsPanel(podRow, deployment, pod, container, safeLogsUrl, logsBtn);
          });
        }
        const podCell = document.createElement("div");
        podCell.className = "container-cell";
        const podLine = document.createElement("span");
        podLine.append(code(pod.name));
        podLine.append(" ");
        podLine.append(pill(pod.phase || "unknown", pod.phase !== "Running"));
        podCell.appendChild(podLine);
        const podMetrics = metricsLine(pod.metrics);
        if (podMetrics) podCell.appendChild(podMetrics);

        const podRow = document.createElement("tr");
        podRow.append(
          cell(deployment.deployment),
          cell(deployment.namespace),
          cell(podCell),
          cell(containerCell),
          cell(actionsCell)
        );
        body.appendChild(podRow);
        rowCount += 1;
      }
    }
    if (!rowCount) renderEmpty("No managed deployment pods returned.");
    lastUpdatedAt = new Date().toLocaleTimeString();
    setStatus(`${containerCount} containers across ${rowCount} pods from ${(data.deployments || []).length} managed deployments · HTTP poll plus websocket-triggered refresh · ${data.terminalEnabled ? "terminal enabled" : "terminal disabled"}`);
  };
  const load = async () => {
    if (loading) return;
    loading = true;
    setStatus("loading managed deployment pods");
    refresh.disabled = true;
    try {
      const response = await fetch("/bastion/runtime/deployments", { cache: "no-store", credentials: "same-origin" });
      if (response.status === 401) {
        renderEmpty("Sign in through /auth?return=/home to load live containers and bastion terminals.");
        setStatus("auth required");
        liveInventoryEnabled = false;
        return;
      }
      if (!response.ok) throw new Error("runtime inventory failed " + response.status);
      render(await response.json());
      connectRuntimeSockets();
    } catch (error) {
      renderEmpty(String(error));
      setStatus("live container inventory unavailable");
    } finally {
      refresh.disabled = false;
      loading = false;
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
    if (runtimeSocketsStarted) return;
    runtimeSocketsStarted = true;
    const clientId = Math.random().toString(36).slice(2);
    openRuntimeSocket("gleam", `/admin/gleam/ws?channel=k8s-runtime-admin&client=home-${clientId}`);
    openRuntimeSocket("rust", `/admin/webrtc/runtime/ws?client=home-${clientId}`);
  };
  const startTimedReload = () => {
    window.setInterval(() => {
      if (liveInventoryEnabled && document.visibilityState === "visible") load();
    }, runtimeReloadIntervalMs);
  };
  refresh.addEventListener("click", () => {
    liveInventoryEnabled = true;
    load();
  });
  renderEmpty("Sign in through /auth?return=/home, then refresh to load live containers and bastion terminals.");
  setStatus("auth required for live container inventory");
  startTimedReload();
})();
"##;

