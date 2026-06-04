use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    net::SocketAddr,
    process::Stdio,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Query, State,
    },
    http::{header, HeaderMap, HeaderName, HeaderValue, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::get,
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::{
    fs,
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command,
    signal,
};

const SERVICE_NAME: &str = "dd-bastion";
const DEFAULT_PORT: u16 = 8111;
const DEFAULT_CA_PATH: &str = "/var/run/secrets/kubernetes.io/serviceaccount/ca.crt";
const DEFAULT_TOKEN_PATH: &str = "/var/run/secrets/kubernetes.io/serviceaccount/token";
const TERMINAL_SHELL: &str =
    "if command -v bash >/dev/null 2>&1; then exec bash -i; else exec sh -i; fi";

#[derive(Clone, Copy)]
struct ManagedDeployment {
    slug: &'static str,
    title: &'static str,
    namespace: &'static str,
    deployment: &'static str,
    service: &'static str,
    access: &'static str,
    notes: &'static str,
}

const MANAGED_DEPLOYMENTS: &[ManagedDeployment] = &[
    ManagedDeployment {
        slug: "web-scraper",
        title: "Web scraper service",
        namespace: "default",
        deployment: "dd-web-scraper",
        service: "dd-web-scraper.default.svc.cluster.local:8097",
        access: "server auth",
        notes: "Fastify scraping strategy router for browser and DOM-backed fetch pipelines.",
    },
    ManagedDeployment {
        slug: "build-server",
        title: "Build server",
        namespace: "default",
        deployment: "dd-build-server",
        service: "dd-build-server.default.svc.cluster.local:8100",
        access: "server auth",
        notes: "Rust CI/CD service for allowlisted repo builds and constrained deploys.",
    },
    ManagedDeployment {
        slug: "ai-ml-pipeline",
        title: "AI/ML feature pipeline",
        namespace: "ai-ml",
        deployment: "dd-ai-ml-pipeline",
        service: "dd-ai-ml-pipeline.ai-ml.svc.cluster.local:8099",
        access: "server auth",
        notes: "Python feature and anomaly pipeline for runtime telemetry.",
    },
    ManagedDeployment {
        slug: "des-simulator",
        title: "Discrete event simulator",
        namespace: "default",
        deployment: "dd-des-simulator",
        service: "dd-des-simulator.default.svc.cluster.local:8099",
        access: "public",
        notes: "Rust DES model validation and simulation service.",
    },
    ManagedDeployment {
        slug: "solana-contracts",
        title: "Solana contract server",
        namespace: "default",
        deployment: "dd-contract-service",
        service: "dd-contract-service.default.svc.cluster.local:8101",
        access: "server auth",
        notes: "Rust Solana contract validation and simulation gateway.",
    },
    ManagedDeployment {
        slug: "vpn",
        title: "VPN server",
        namespace: "vpn",
        deployment: "dd-vpn",
        service: "dd-vpn-ui.vpn.svc.cluster.local:51821",
        access: "vpn/private",
        notes: "WireGuard wg-easy overlay and private admin UI.",
    },
    ManagedDeployment {
        slug: "live-mutex",
        title: "Live mutex server",
        namespace: "default",
        deployment: "dd-live-mutex",
        service: "dd-live-mutex.default.svc.cluster.local:6970",
        access: "cluster local",
        notes: "Singleton TCP lock broker for live-mutex clients.",
    },
    ManagedDeployment {
        slug: "bastion",
        title: "Bastion/jumphost server",
        namespace: "vpn",
        deployment: "dd-bastion",
        service: "dd-bastion.vpn.svc.cluster.local:8111",
        access: "server auth",
        notes: "Authenticated access broker for VPN, kubeconfig, runtime inventory, and exec terminals.",
    },
    ManagedDeployment {
        slug: "redis-cache",
        title: "Redis cache server",
        namespace: "default",
        deployment: "dd-redis-cache",
        service: "dd-redis-cache.default.svc.cluster.local:6379",
        access: "cluster local",
        notes: "Ephemeral Redis cache with memory cap and health probes.",
    },
    ManagedDeployment {
        slug: "lock-loadtest-trigger",
        title: "Node.js lock loadtest trigger",
        namespace: "default",
        deployment: "dd-lock-loadtest-trigger",
        service: "dd-lock-loadtest-trigger.default.svc.cluster.local:8110",
        access: "internal",
        notes: "HTTP trigger for live-mutex versus Redis aggregate lock load tests.",
    },
    ManagedDeployment {
        slug: "trading",
        title: "Rust algorithmic trading server",
        namespace: "default",
        deployment: "dd-trading-server",
        service: "dd-trading-server.default.svc.cluster.local:8103",
        access: "server auth",
        notes: "Risk-gated trading decision service for paper/live order intents.",
    },
    ManagedDeployment {
        slug: "container-pool",
        title: "Container pool service",
        namespace: "default",
        deployment: "dd-container-pool",
        service: "dd-container-pool.default.svc.cluster.local:8102",
        access: "server auth",
        notes: "Rust warm-worker pool backed by Postgres config and local containerd.",
    },
    ManagedDeployment {
        slug: "gleam-lambda-runner",
        title: "Gleam lambda runner",
        namespace: "default",
        deployment: "dd-gleam-lambda-runner",
        service: "dd-gleam-lambda-runner.default.svc.cluster.local:8083",
        access: "server auth",
        notes: "Gleam child-process function runner for lambda invocation traffic.",
    },
    ManagedDeployment {
        slug: "remote-gateway",
        title: "Public gateway",
        namespace: "default",
        deployment: "dd-remote-gateway",
        service: "dd-remote-gateway.default.svc.cluster.local:80/443",
        access: "public",
        notes: "nginx gateway that owns EC2 hostPort 80/443 and proxies public/auth paths.",
    },
    ManagedDeployment {
        slug: "web-home",
        title: "Rust web home",
        namespace: "default",
        deployment: "dd-remote-web-home",
        service: "dd-remote-web-home.default.svc.cluster.local:8080",
        access: "public",
        notes: "Rust homepage, task/thread UI, presence test, and WebSocket test lab.",
    },
    ManagedDeployment {
        slug: "remote-auth",
        title: "Rust PIN auth",
        namespace: "default",
        deployment: "dd-remote-auth",
        service: "dd-remote-auth.default.svc.cluster.local:8083",
        access: "public",
        notes: "Browser PIN auth service that mints the dd_auth gateway cookie.",
    },
    ManagedDeployment {
        slug: "remote-rest-api",
        title: "Rust REST API",
        namespace: "default",
        deployment: "dd-remote-rest-api",
        service: "dd-remote-rest-api.default.svc.cluster.local:8082",
        access: "public",
        notes: "RDS/Postgres-backed JSON API for agent and lambda data.",
    },
    ManagedDeployment {
        slug: "agent-worker-broker",
        title: "Agent worker broker",
        namespace: "default",
        deployment: "dd-agent-worker-broker",
        service: "dd-agent-worker-broker.default.svc.cluster.local:8098",
        access: "server auth",
        notes: "Rust NATS-first worker dispatch broker for UUID-bound workers.",
    },
    ManagedDeployment {
        slug: "dev-server-api",
        title: "Node.js dev server API",
        namespace: "default",
        deployment: "dd-dev-server-api",
        service: "dd-dev-server-api.default.svc.cluster.local:8080",
        access: "server auth",
        notes: "Bootstrap coding-agent task manager behind /tasks, /status, and /stream.",
    },
    ManagedDeployment {
        slug: "queue-consumer",
        title: "Queue consumer",
        namespace: "default",
        deployment: "dd-remote-queue-consumer",
        service: "dd-remote-queue-consumer.default.svc.cluster.local",
        access: "internal",
        notes: "Rust NATS shadow consumer that prepares thread-affined workers.",
    },
    ManagedDeployment {
        slug: "idle-reaper",
        title: "Idle reaper",
        namespace: "default",
        deployment: "dd-idle-reaper",
        service: "dd-idle-reaper.default.svc.cluster.local",
        access: "internal",
        notes: "Runtime maintenance supervisor and Kubernetes pod/deployment event watcher.",
    },
    ManagedDeployment {
        slug: "billing",
        title: "Billing server",
        namespace: "default",
        deployment: "dd-billing-server",
        service: "dd-billing-server.default.svc.cluster.local:80",
        access: "cluster local",
        notes: "Rust multi-tenant AR/AP ledger and provider integration service.",
    },
    ManagedDeployment {
        slug: "gleamlang-server",
        title: "Gleam WebSocket fan-out",
        namespace: "default",
        deployment: "dd-gleamlang-server",
        service: "dd-gleamlang-server.default.svc.cluster.local:8081",
        access: "server auth",
        notes: "Gleam/OTP WebSocket fan-out for /gleam/ws plus NATS runtime event broadcast.",
    },
    ManagedDeployment {
        slug: "gleam-mcp",
        title: "Gleam MCP server",
        namespace: "default",
        deployment: "dd-gleam-mcp-server",
        service: "dd-gleam-mcp-server.default.svc.cluster.local:8090",
        access: "server auth",
        notes: "Read-only JSON-RPC MCP runtime tools, metrics, and log surface.",
    },
    ManagedDeployment {
        slug: "webrtc-signaling",
        title: "Rust WebRTC signaling",
        namespace: "default",
        deployment: "dd-webrtc-signaling",
        service: "dd-webrtc-signaling.default.svc.cluster.local:8095",
        access: "public",
        notes: "Rust WebSocket signaling service and admin runtime event relay.",
    },
    ManagedDeployment {
        slug: "mdp-optimizer",
        title: "MDP/POMDP optimizer",
        namespace: "default",
        deployment: "dd-mdp-optimizer",
        service: "dd-mdp-optimizer.default.svc.cluster.local:8096",
        access: "public",
        notes: "Rust optimizer service for MDP/POMDP/RL jobs and telemetry learning.",
    },
    ManagedDeployment {
        slug: "akka-ws",
        title: "Akka WebSocket reference server",
        namespace: "default",
        deployment: "dd-akka-ws-server",
        service: "dd-akka-ws-server.default.svc.cluster.local:8086",
        access: "internal",
        notes: "Scala/Akka WebSocket target for streams and async Java load tests.",
    },
    ManagedDeployment {
        slug: "fsharp-ws",
        title: "F# WebSocket server",
        namespace: "default",
        deployment: "dd-fsharp-ws-server",
        service: "dd-fsharp-ws-server.default.svc.cluster.local:8087",
        access: "public",
        notes: "F# ASP.NET Core WebSocket server behind /fsws/.",
    },
    ManagedDeployment {
        slug: "formal-methods-server",
        title: "Formal methods server",
        namespace: "default",
        deployment: "dd-formal-methods-server",
        service: "dd-formal-methods-server.default.svc.cluster.local:8110",
        access: "internal",
        notes: "Rust annotation-driven formal verification runtime.",
    },
    ManagedDeployment {
        slug: "formal-methods-service",
        title: "Formal methods service",
        namespace: "default",
        deployment: "dd-formal-methods-service",
        service: "dd-formal-methods-service.default.svc.cluster.local:8111",
        access: "internal",
        notes: "Rust orchestration layer for formal verification jobs.",
    },
    ManagedDeployment {
        slug: "spark-pipeline",
        title: "Spark pipeline server",
        namespace: "ai-ml",
        deployment: "dd-spark-pipeline-server",
        service: "dd-spark-pipeline-server.ai-ml.svc.cluster.local:8085",
        access: "internal",
        notes: "Java/Spark batch and stream coordination service.",
    },
    ManagedDeployment {
        slug: "ws-loadtest-rs",
        title: "Rust WebSocket load generator",
        namespace: "default",
        deployment: "dd-ws-loadtest-rs",
        service: "dd-ws-loadtest-rs.default.svc.cluster.local",
        access: "internal",
        notes: "Rust 5k-client WebSocket load generator targeting the Gleam fan-out path.",
    },
    ManagedDeployment {
        slug: "ws-loadtest-rs-akka-streams",
        title: "Rust Akka streams load generator",
        namespace: "default",
        deployment: "dd-ws-loadtest-rs-akkaws-akkastreams",
        service: "dd-ws-loadtest-rs-akkaws-akkastreams.default.svc.cluster.local",
        access: "internal",
        notes: "Rust WebSocket load generator targeting Akka Streams.",
    },
    ManagedDeployment {
        slug: "ws-loadtest-rs-asyncjava",
        title: "Rust async Java load generator",
        namespace: "default",
        deployment: "dd-ws-loadtest-rs-akkaws-asyncjava",
        service: "dd-ws-loadtest-rs-akkaws-asyncjava.default.svc.cluster.local",
        access: "internal",
        notes: "Rust WebSocket load generator targeting async Java.",
    },
    ManagedDeployment {
        slug: "gleamlang-ws-loadtest",
        title: "Gleam WebSocket load generator",
        namespace: "default",
        deployment: "dd-gleamlang-ws-loadtest",
        service: "dd-gleamlang-ws-loadtest.default.svc.cluster.local",
        access: "internal",
        notes: "Gleam WebSocket load generator targeting the Gleam fan-out path.",
    },
    ManagedDeployment {
        slug: "gleamlang-ws-loadtest-akka-streams",
        title: "Gleam Akka streams load generator",
        namespace: "default",
        deployment: "dd-gleamlang-ws-loadtest-akkaws-akkastreams",
        service: "dd-gleamlang-ws-loadtest-akkaws-akkastreams.default.svc.cluster.local",
        access: "internal",
        notes: "Gleam WebSocket load generator targeting Akka Streams.",
    },
    ManagedDeployment {
        slug: "gleamlang-ws-loadtest-asyncjava",
        title: "Gleam async Java load generator",
        namespace: "default",
        deployment: "dd-gleamlang-ws-loadtest-akkaws-asyncjava",
        service: "dd-gleamlang-ws-loadtest-akkaws-asyncjava.default.svc.cluster.local",
        access: "internal",
        notes: "Gleam WebSocket load generator targeting async Java.",
    },
    ManagedDeployment {
        slug: "gcs",
        title: "chat.vibe runtime",
        namespace: "default",
        deployment: "gcs",
        service: "gcs.default.svc.cluster.local:3000/3001",
        access: "server auth",
        notes: "chat.vibe REST and WebSocket runtime for /gcs/api and /gcs/ws traffic.",
    },
    ManagedDeployment {
        slug: "gcs-router",
        title: "chat.vibe WebSocket router",
        namespace: "default",
        deployment: "gcs-router",
        service: "gcs-router.default.svc.cluster.local:3001",
        access: "server auth",
        notes: "WebSocket router that pins conv/user/device traffic across gcs pods.",
    },
    ManagedDeployment {
        slug: "nats",
        title: "NATS JetStream",
        namespace: "messaging",
        deployment: "dd-nats",
        service: "dd-nats.messaging.svc.cluster.local:4222/8222/7777",
        access: "internal",
        notes: "NATS and JetStream broker for cluster runtime events and task queues.",
    },
    ManagedDeployment {
        slug: "headlamp",
        title: "Headlamp Kubernetes UI",
        namespace: "headlamp",
        deployment: "headlamp",
        service: "headlamp.headlamp.svc.cluster.local:80",
        access: "server auth",
        notes: "Read-only Kubernetes web UI deployment.",
    },
];

#[derive(Clone)]
struct AppState {
    config: Arc<Config>,
}

#[derive(Clone)]
struct Config {
    server_auth_secret: Option<String>,
    public_base_url: String,
    wireguard_endpoint: String,
    vpn_cidr: String,
    service_cidr: String,
    pod_cidr: String,
    dns: String,
    kube_api_server: String,
    kube_cluster_name: String,
    kube_context_name: String,
    kube_user_name: String,
    ca_path: String,
    token_path: String,
    kubectl_bin: String,
    script_bin: String,
    kubeconfig_enabled: bool,
    include_serviceaccount_token: bool,
    terminal_enabled: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct HealthResponse {
    ok: bool,
    service: &'static str,
    auth_configured: bool,
    kubeconfig_enabled: bool,
    terminal_enabled: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AccessProfile {
    service: &'static str,
    auth_required: bool,
    vpn: VpnProfile,
    cluster: ClusterProfile,
    endpoints: EndpointProfile,
    examples: Vec<String>,
    notes: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct VpnProfile {
    wireguard_endpoint: String,
    vpn_cidr: String,
    dns: String,
    split_tunnel_allowed_ips: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ClusterProfile {
    api_server: String,
    service_cidr: String,
    pod_cidr: String,
    kubeconfig_endpoint: String,
    kubeconfig_mode: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EndpointProfile {
    bastion_url: String,
    healthz: String,
    profile: String,
    kubeconfig: String,
    runtime_deployments: String,
    terminal: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ManagedDeploymentInfo {
    slug: &'static str,
    title: &'static str,
    namespace: &'static str,
    deployment: &'static str,
    service: &'static str,
    access: &'static str,
    notes: &'static str,
    summary: Value,
    pods: Vec<Value>,
    errors: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeDeploymentsResponse {
    ok: bool,
    service: &'static str,
    generated_at_ms: u128,
    terminal_enabled: bool,
    metrics_available: bool,
    deployments: Vec<ManagedDeploymentInfo>,
    errors: Vec<String>,
    metrics_errors: Vec<String>,
}

#[derive(Clone)]
struct TerminalTarget {
    namespace: String,
    deployment: String,
    pod: String,
    container: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TerminalQuery {
    namespace: String,
    deployment: String,
    pod: String,
    container: String,
}

fn first_env(keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        env::var(key)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    })
}

fn env_value(key: &str, fallback: &str) -> String {
    first_env(&[key]).unwrap_or_else(|| fallback.to_string())
}

fn env_bool(key: &str, fallback: bool) -> bool {
    first_env(&[key])
        .map(|value| {
            matches!(
                value.as_str(),
                "1" | "true" | "TRUE" | "yes" | "YES" | "on" | "ON"
            )
        })
        .unwrap_or(fallback)
}

fn config_from_env() -> Config {
    Config {
        server_auth_secret: first_env(&["BASTION_AUTH_SECRET", "SERVER_AUTH_SECRET"]),
        public_base_url: env_value(
            "BASTION_PUBLIC_BASE_URL",
            "http://dd-bastion.vpn.svc.cluster.local:8111",
        ),
        wireguard_endpoint: env_value("BASTION_WIREGUARD_ENDPOINT", "54.91.17.58:51820"),
        vpn_cidr: env_value("BASTION_VPN_CIDR", "10.8.0.0/24"),
        service_cidr: env_value("BASTION_SERVICE_CIDR", "10.96.0.0/12"),
        pod_cidr: env_value("BASTION_POD_CIDR", "10.244.0.0/16"),
        dns: env_value("BASTION_DNS", "10.96.0.10"),
        kube_api_server: env_value("BASTION_KUBE_API_SERVER", "https://kubernetes.default.svc"),
        kube_cluster_name: env_value("BASTION_KUBE_CLUSTER_NAME", "dd-remote-dev"),
        kube_context_name: env_value("BASTION_KUBE_CONTEXT_NAME", "dd-vpn-readonly"),
        kube_user_name: env_value("BASTION_KUBE_USER_NAME", "dd-bastion-readonly"),
        ca_path: env_value("BASTION_KUBE_CA_PATH", DEFAULT_CA_PATH),
        token_path: env_value("BASTION_KUBE_TOKEN_PATH", DEFAULT_TOKEN_PATH),
        kubectl_bin: env_value("BASTION_KUBECTL_BIN", "/usr/bin/kubectl"),
        script_bin: env_value("BASTION_SCRIPT_BIN", "/usr/bin/script"),
        kubeconfig_enabled: env_bool("BASTION_KUBECONFIG_ENABLED", true),
        include_serviceaccount_token: env_bool("BASTION_INCLUDE_SERVICEACCOUNT_TOKEN", true),
        terminal_enabled: env_bool("BASTION_TERMINAL_ENABLED", false),
    }
}

fn constant_time_eq(left: &str, right: &str) -> bool {
    let left = left.as_bytes();
    let right = right.as_bytes();
    let max_len = left.len().max(right.len());
    let mut diff = left.len() ^ right.len();

    for index in 0..max_len {
        let left_byte = *left.get(index).unwrap_or(&0);
        let right_byte = *right.get(index).unwrap_or(&0);
        diff |= usize::from(left_byte ^ right_byte);
    }

    diff == 0
}

fn request_is_authorized(headers: &HeaderMap, secret: &str) -> bool {
    let direct_header_matches = headers
        .get("x-bastion-auth")
        .or_else(|| headers.get("x-server-auth"))
        .or_else(|| headers.get("auth"))
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| constant_time_eq(value, secret));

    let bearer_matches = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .is_some_and(|value| constant_time_eq(value, secret));

    direct_header_matches || bearer_matches
}

fn require_auth(headers: &HeaderMap, state: &AppState) -> Result<(), Response> {
    let Some(secret) = state.config.server_auth_secret.as_deref() else {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "SERVER_AUTH_SECRET is not configured" })),
        )
            .into_response());
    };

    if !request_is_authorized(headers, secret) {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(json!({
                "error": "unauthorized",
                "errMessage": "missing required bastion auth header",
            })),
        )
            .into_response());
    }

    Ok(())
}

fn split_allowed_ips(config: &Config) -> Vec<String> {
    vec![
        config.vpn_cidr.clone(),
        config.service_cidr.clone(),
        config.pod_cidr.clone(),
    ]
}

fn access_profile(config: &Config) -> AccessProfile {
    let base = config.public_base_url.trim_end_matches('/').to_string();
    AccessProfile {
        service: SERVICE_NAME,
        auth_required: true,
        vpn: VpnProfile {
            wireguard_endpoint: config.wireguard_endpoint.clone(),
            vpn_cidr: config.vpn_cidr.clone(),
            dns: config.dns.clone(),
            split_tunnel_allowed_ips: split_allowed_ips(config),
        },
        cluster: ClusterProfile {
            api_server: config.kube_api_server.clone(),
            service_cidr: config.service_cidr.clone(),
            pod_cidr: config.pod_cidr.clone(),
            kubeconfig_endpoint: format!("{base}/kubeconfig"),
            kubeconfig_mode: if config.include_serviceaccount_token {
                "read-only service account token".to_string()
            } else {
                "template without token".to_string()
            },
        },
        endpoints: EndpointProfile {
            bastion_url: base.clone(),
            healthz: format!("{base}/healthz"),
            profile: format!("{base}/profile"),
            kubeconfig: format!("{base}/kubeconfig"),
            runtime_deployments: format!("{base}/runtime/deployments"),
            terminal: format!("{base}/terminal"),
        },
        examples: vec![
            format!("curl -H 'X-Bastion-Auth: $SERVER_AUTH_SECRET' {base}/profile"),
            format!(
                "curl -H 'X-Bastion-Auth: $SERVER_AUTH_SECRET' {base}/kubeconfig > dd-vpn.kubeconfig"
            ),
            format!("curl -H 'X-Bastion-Auth: $SERVER_AUTH_SECRET' {base}/runtime/deployments"),
            "KUBECONFIG=dd-vpn.kubeconfig kubectl get pods -A".to_string(),
        ],
        notes: vec![
            "Connect to WireGuard first; this service is ClusterIP-only inside the vpn namespace."
                .to_string(),
            "The generated kubeconfig is intentionally read-only and does not include Kubernetes Secrets access."
                .to_string(),
            if config.terminal_enabled {
                "Exec terminals are enabled and restricted to the managed deployment allowlist."
                    .to_string()
            } else {
                "Exec terminals are disabled in the hardened default deployment.".to_string()
            },
        ],
    }
}

async fn healthz(State(state): State<AppState>) -> Json<HealthResponse> {
    Json(HealthResponse {
        ok: true,
        service: SERVICE_NAME,
        auth_configured: state.config.server_auth_secret.is_some(),
        kubeconfig_enabled: state.config.kubeconfig_enabled,
        terminal_enabled: state.config.terminal_enabled,
    })
}

async fn profile(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<AccessProfile>, Response> {
    require_auth(&headers, &state)?;
    Ok(Json(access_profile(&state.config)))
}

fn yaml_string(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_string())
}

async fn kubeconfig(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, Response> {
    require_auth(&headers, &state)?;

    if !state.config.kubeconfig_enabled {
        return Err((
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "kubeconfig export is disabled" })),
        )
            .into_response());
    }

    let ca_bytes = fs::read(&state.config.ca_path).await.map_err(|error| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "error": "failed to read Kubernetes CA",
                "errMessage": error.to_string(),
            })),
        )
            .into_response()
    })?;
    let ca_data = STANDARD.encode(ca_bytes);

    let token = if state.config.include_serviceaccount_token {
        fs::read_to_string(&state.config.token_path)
            .await
            .map(|value| value.trim().to_string())
            .map_err(|error| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({
                        "error": "failed to read Kubernetes service account token",
                        "errMessage": error.to_string(),
                    })),
                )
                    .into_response()
            })?
    } else {
        "replace-with-a-kubernetes-bearer-token".to_string()
    };

    let body = format!(
        r#"apiVersion: v1
kind: Config
clusters:
  - name: {cluster_name}
    cluster:
      certificate-authority-data: {ca_data}
      server: {api_server}
contexts:
  - name: {context_name}
    context:
      cluster: {cluster_name}
      user: {user_name}
current-context: {context_name}
users:
  - name: {user_name}
    user:
      token: {token}
"#,
        cluster_name = yaml_string(&state.config.kube_cluster_name),
        ca_data = ca_data,
        api_server = yaml_string(&state.config.kube_api_server),
        context_name = yaml_string(&state.config.kube_context_name),
        user_name = yaml_string(&state.config.kube_user_name),
        token = yaml_string(&token),
    );

    let mut response = body.into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/yaml; charset=utf-8"),
    );
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.headers_mut().insert(
        HeaderName::from_static("x-content-type-options"),
        HeaderValue::from_static("nosniff"),
    );
    Ok(response)
}

fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn safe_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 253
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
}

fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn json_at<'a>(value: &'a Value, path: &[&str]) -> Option<&'a Value> {
    path.iter()
        .try_fold(value, |current, key| current.get(*key))
}

fn json_at_string(value: &Value, path: &[&str]) -> Option<String> {
    json_at(value, path)
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn json_at_i64(value: &Value, path: &[&str]) -> Option<i64> {
    json_at(value, path).and_then(Value::as_i64)
}

fn selector_matches_pod(deployment: &Value, pod: &Value) -> bool {
    let Some(selector_labels) =
        json_at(deployment, &["spec", "selector", "matchLabels"]).and_then(Value::as_object)
    else {
        return false;
    };
    let Some(pod_labels) = json_at(pod, &["metadata", "labels"]).and_then(Value::as_object) else {
        return false;
    };
    selector_labels
        .iter()
        .all(|(key, value)| pod_labels.get(key) == Some(value))
}

fn summarize_deployment(deployment: &Value) -> Value {
    let conditions = json_at(deployment, &["status", "conditions"])
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|condition| {
            json!({
                "type": json_at_string(&condition, &["type"]),
                "status": json_at_string(&condition, &["status"]),
                "reason": json_at_string(&condition, &["reason"]),
                "message": json_at_string(&condition, &["message"]),
            })
        })
        .collect::<Vec<_>>();

    json!({
        "name": json_at_string(deployment, &["metadata", "name"]),
        "namespace": json_at_string(deployment, &["metadata", "namespace"]),
        "createdAt": json_at_string(deployment, &["metadata", "creationTimestamp"]),
        "desiredReplicas": json_at_i64(deployment, &["spec", "replicas"]).unwrap_or(0),
        "replicas": json_at_i64(deployment, &["status", "replicas"]).unwrap_or(0),
        "readyReplicas": json_at_i64(deployment, &["status", "readyReplicas"]).unwrap_or(0),
        "availableReplicas": json_at_i64(deployment, &["status", "availableReplicas"]).unwrap_or(0),
        "updatedReplicas": json_at_i64(deployment, &["status", "updatedReplicas"]).unwrap_or(0),
        "unavailableReplicas": json_at_i64(deployment, &["status", "unavailableReplicas"]).unwrap_or(0),
        "conditions": conditions,
    })
}

/// Container-level CPU + memory snapshot, expressed in millicores and bytes.
#[derive(Clone, Copy, Default)]
struct ContainerMetrics {
    cpu_millicores: i64,
    memory_bytes: i64,
}

/// `kubectl get --raw /apis/metrics.k8s.io/v1beta1/namespaces/<ns>/pods` does
/// NOT support label-selecting, so we fetch all pods in the managed
/// namespace once and look entries up by name. The result lookup is keyed
/// by `(pod_name, container_name)` so per-container rows can carry their
/// own usage values.
type PodMetricsLookup = BTreeMap<(String, String), ContainerMetrics>;

fn build_metrics_lookup(metrics_payload: &Value) -> PodMetricsLookup {
    let mut lookup = PodMetricsLookup::new();
    for item in json_items(metrics_payload) {
        let Some(pod_name) = json_at_string(&item, &["metadata", "name"]) else {
            continue;
        };
        let Some(containers) = item.get("containers").and_then(Value::as_array) else {
            continue;
        };
        for container in containers {
            let Some(name) = json_at_string(container, &["name"]) else {
                continue;
            };
            let cpu_millicores = json_at_string(container, &["usage", "cpu"])
                .and_then(|raw| parse_cpu_millicores(&raw))
                .unwrap_or(0);
            let memory_bytes = json_at_string(container, &["usage", "memory"])
                .and_then(|raw| parse_memory_bytes(&raw))
                .unwrap_or(0);
            lookup.insert(
                (pod_name.clone(), name),
                ContainerMetrics {
                    cpu_millicores,
                    memory_bytes,
                },
            );
        }
    }
    lookup
}

/// Parse a Kubernetes CPU quantity into integer millicores.
///
/// metrics-server reports CPU in nanoCPU (suffix `n`), but other producers
/// occasionally hand back micro/milli/integer values. Always returns a
/// non-negative count of millicores; out-of-range values fall back to 0.
fn parse_cpu_millicores(raw: &str) -> Option<i64> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Some(prefix) = trimmed.strip_suffix('n') {
        return prefix.parse::<i64>().ok().map(|v| v / 1_000_000);
    }
    if let Some(prefix) = trimmed.strip_suffix('u') {
        return prefix.parse::<i64>().ok().map(|v| v / 1_000);
    }
    if let Some(prefix) = trimmed.strip_suffix('m') {
        return prefix.parse::<i64>().ok();
    }
    trimmed
        .parse::<f64>()
        .ok()
        .map(|cpus| (cpus * 1_000.0).round() as i64)
}

/// Parse a Kubernetes memory quantity into bytes.
fn parse_memory_bytes(raw: &str) -> Option<i64> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    const SUFFIXES: &[(&str, i64)] = &[
        ("Ei", 1_152_921_504_606_846_976),
        ("Pi", 1_125_899_906_842_624),
        ("Ti", 1_099_511_627_776),
        ("Gi", 1_073_741_824),
        ("Mi", 1_048_576),
        ("Ki", 1_024),
        ("E", 1_000_000_000_000_000_000),
        ("P", 1_000_000_000_000_000),
        ("T", 1_000_000_000_000),
        ("G", 1_000_000_000),
        ("M", 1_000_000),
        ("k", 1_000),
    ];
    for (suffix, multiplier) in SUFFIXES {
        if let Some(prefix) = trimmed.strip_suffix(*suffix) {
            return prefix.parse::<i64>().ok().map(|v| v * multiplier);
        }
    }
    trimmed.parse::<i64>().ok()
}

fn summarize_pod(
    pod: &Value,
    deployment_name: &str,
    terminal_base: &str,
    logs_base: &str,
    metrics: Option<&PodMetricsLookup>,
) -> Value {
    let namespace = json_at_string(pod, &["metadata", "namespace"]).unwrap_or_default();
    let pod_name = json_at_string(pod, &["metadata", "name"]).unwrap_or_default();
    let mut pod_cpu = 0_i64;
    let mut pod_mem = 0_i64;
    let mut pod_has_metrics = false;
    let containers = json_at(pod, &["status", "containerStatuses"])
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|container| {
            let name = json_at_string(&container, &["name"]).unwrap_or_default();
            let make_link = |base: &str| -> String {
                if !namespace.is_empty()
                    && !deployment_name.is_empty()
                    && !pod_name.is_empty()
                    && !name.is_empty()
                    && !base.is_empty()
                {
                    format!(
                        "{base}?namespace={namespace}&deployment={deployment_name}&pod={pod_name}&container={name}"
                    )
                } else {
                    String::new()
                }
            };
            let terminal_url = make_link(terminal_base);
            let logs_url = make_link(logs_base);
            let container_metrics = metrics
                .and_then(|lookup| lookup.get(&(pod_name.clone(), name.clone())).copied());
            if let Some(values) = container_metrics {
                pod_cpu = pod_cpu.saturating_add(values.cpu_millicores);
                pod_mem = pod_mem.saturating_add(values.memory_bytes);
                pod_has_metrics = true;
            }
            json!({
                "name": name,
                "ready": container.get("ready").and_then(Value::as_bool).unwrap_or(false),
                "restartCount": json_at_i64(&container, &["restartCount"]).unwrap_or(0),
                "image": json_at_string(&container, &["image"]),
                "imageId": json_at_string(&container, &["imageID"]),
                "containerId": json_at_string(&container, &["containerID"]),
                "state": container.get("state").cloned().unwrap_or_else(|| json!({})),
                "lastState": container.get("lastState").cloned().unwrap_or_else(|| json!({})),
                "terminalUrl": terminal_url,
                "logsUrl": logs_url,
                "metrics": container_metrics.map(|values| json!({
                    "cpuMillicores": values.cpu_millicores,
                    "memoryBytes": values.memory_bytes,
                })),
            })
        })
        .collect::<Vec<_>>();

    json!({
        "name": pod_name,
        "namespace": namespace,
        "phase": json_at_string(pod, &["status", "phase"]),
        "nodeName": json_at_string(pod, &["spec", "nodeName"]),
        "podIp": json_at_string(pod, &["status", "podIP"]),
        "createdAt": json_at_string(pod, &["metadata", "creationTimestamp"]),
        "containers": containers,
        "metrics": if pod_has_metrics {
            json!({
                "cpuMillicores": pod_cpu,
                "memoryBytes": pod_mem,
            })
        } else {
            Value::Null
        },
    })
}

/// Default kubectl timeout used by the inventory + terminal/log validation
/// paths. metrics-server lookups override this with a tighter
/// `METRICS_KUBECTL_TIMEOUT` so a sick aggregation API never stalls the
/// rest of the inventory.
const DEFAULT_KUBECTL_TIMEOUT: Duration = Duration::from_secs(15);
const METRICS_KUBECTL_TIMEOUT: Duration = Duration::from_secs(3);

async fn kubectl_json(config: &Config, args: &[String]) -> Result<Value, String> {
    kubectl_json_with_timeout(config, args, DEFAULT_KUBECTL_TIMEOUT).await
}

async fn kubectl_json_with_timeout(
    config: &Config,
    args: &[String],
    timeout: Duration,
) -> Result<Value, String> {
    let output = tokio::time::timeout(
        timeout,
        Command::new(&config.kubectl_bin).args(args).output(),
    )
    .await
    .map_err(|_| format!("kubectl timed out: {}", args.join(" ")))?
    .map_err(|error| format!("kubectl failed to start: {error}"))?;

    if !output.status.success() {
        return Err(format!(
            "kubectl {} failed {}: {}",
            args.join(" "),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("kubectl {} returned invalid json: {error}", args.join(" ")))
}

async fn executable_available(path: &str) -> bool {
    fs::metadata(path).await.is_ok()
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn json_items_by_name(value: &Value) -> BTreeMap<String, Value> {
    json_at(value, &["items"])
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|item| json_at_string(&item, &["metadata", "name"]).map(|name| (name, item)))
        .collect()
}

fn json_items(value: &Value) -> Vec<Value> {
    json_at(value, &["items"])
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn managed_deployment_namespaces() -> Vec<&'static str> {
    MANAGED_DEPLOYMENTS
        .iter()
        .map(|target| target.namespace)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// Combined per-namespace result: deployment list, pod list, and the raw
/// metrics-server payload for that namespace, each independently fallible.
type NamespaceFetch = (
    &'static str,
    Result<Value, String>,
    Result<Value, String>,
    Result<Value, String>,
);

/// Issue all kubectl calls for a single namespace concurrently.
///
/// We run deployments + pods (15 s timeout) alongside the
/// metrics.k8s.io/v1beta1 raw GET (3 s timeout) under the same task. The
/// short metrics timeout keeps a sick aggregation API from stalling the
/// inventory: when metrics fail, the rest of the row still ships.
async fn fetch_namespace(config: Arc<Config>, namespace: &'static str) -> NamespaceFetch {
    let deployments_args = vec![
        "-n".to_string(),
        namespace.to_string(),
        "get".to_string(),
        "deployments".to_string(),
        "-o".to_string(),
        "json".to_string(),
    ];
    let pods_args = vec![
        "-n".to_string(),
        namespace.to_string(),
        "get".to_string(),
        "pods".to_string(),
        "-o".to_string(),
        "json".to_string(),
    ];
    let metrics_args = vec![
        "get".to_string(),
        "--raw".to_string(),
        format!("/apis/metrics.k8s.io/v1beta1/namespaces/{namespace}/pods"),
    ];

    let deployments_fut = kubectl_json(&config, &deployments_args);
    let pods_fut = kubectl_json(&config, &pods_args);
    let metrics_fut = kubectl_json_with_timeout(&config, &metrics_args, METRICS_KUBECTL_TIMEOUT);

    let (deployments, pods, metrics) = tokio::join!(deployments_fut, pods_fut, metrics_fut);
    (namespace, deployments, pods, metrics)
}

async fn managed_deployment_infos(
    config: Arc<Config>,
    terminal_base: &str,
    logs_base: &str,
) -> (Vec<ManagedDeploymentInfo>, Vec<String>, Vec<String>) {
    let mut deployments_by_namespace = BTreeMap::new();
    let mut pods_by_namespace = BTreeMap::new();
    let mut metrics_by_namespace: BTreeMap<&'static str, PodMetricsLookup> = BTreeMap::new();
    let mut deployment_errors = BTreeMap::new();
    let mut pod_errors = BTreeMap::new();
    let mut metrics_errors = BTreeMap::new();

    // Fan out one async task per namespace; each task issues its three
    // kubectl calls concurrently with `tokio::join!`. Total wall-clock is
    // therefore max(slowest single kubectl call) instead of sum, which
    // keeps the homepage refresh fast as we add namespaces.
    let mut tasks: tokio::task::JoinSet<NamespaceFetch> = tokio::task::JoinSet::new();
    for namespace in managed_deployment_namespaces() {
        tasks.spawn(fetch_namespace(Arc::clone(&config), namespace));
    }

    while let Some(joined) = tasks.join_next().await {
        let Ok((namespace, deployments, pods, metrics)) = joined else {
            continue;
        };
        match deployments {
            Ok(value) => {
                deployments_by_namespace.insert(namespace, json_items_by_name(&value));
            }
            Err(error) => {
                deployment_errors.insert(namespace, error);
            }
        }
        match pods {
            Ok(value) => {
                pods_by_namespace.insert(namespace, json_items(&value));
            }
            Err(error) => {
                pod_errors.insert(namespace, error);
            }
        }
        // metrics-server (the kube-system Argo CD app) is the source of
        // truth for CPU and memory snapshots. The bastion fails open: if
        // the API is not yet installed or the per-call timeout fires, the
        // route still serves inventory without metrics.
        match metrics {
            Ok(value) => {
                metrics_by_namespace.insert(namespace, build_metrics_lookup(&value));
            }
            Err(error) => {
                metrics_errors.insert(namespace, error);
            }
        }
    }

    let mut errors = BTreeSet::new();
    let deployments = MANAGED_DEPLOYMENTS
        .iter()
        .copied()
        .map(|target| {
            let mut target_errors = Vec::new();
            let deployment = if let Some(namespace_deployments) =
                deployments_by_namespace.get(target.namespace)
            {
                namespace_deployments
                    .get(target.deployment)
                    .cloned()
                    .or_else(|| {
                        target_errors.push(format!(
                            "deployment {}/{} was not returned by Kubernetes",
                            target.namespace, target.deployment
                        ));
                        None
                    })
            } else {
                if let Some(error) = deployment_errors.get(target.namespace) {
                    target_errors.push(error.clone());
                }
                None
            };

            let pods = if let Some(deployment) = deployment.as_ref() {
                if let Some(namespace_pods) = pods_by_namespace.get(target.namespace) {
                    let metrics = metrics_by_namespace.get(target.namespace);
                    namespace_pods
                        .iter()
                        .filter(|pod| selector_matches_pod(deployment, pod))
                        .map(|pod| {
                            summarize_pod(
                                pod,
                                target.deployment,
                                terminal_base,
                                logs_base,
                                metrics,
                            )
                        })
                        .collect::<Vec<_>>()
                } else {
                    if let Some(error) = pod_errors.get(target.namespace) {
                        target_errors.push(error.clone());
                    }
                    Vec::new()
                }
            } else {
                Vec::new()
            };

            for error in &target_errors {
                errors.insert(error.clone());
            }

            ManagedDeploymentInfo {
                slug: target.slug,
                title: target.title,
                namespace: target.namespace,
                deployment: target.deployment,
                service: target.service,
                access: target.access,
                notes: target.notes,
                summary: deployment
                    .as_ref()
                    .map(summarize_deployment)
                    .unwrap_or_else(|| json!({})),
                pods,
                errors: target_errors,
            }
        })
        .collect::<Vec<_>>();

    let metrics_errors_vec = metrics_errors
        .into_iter()
        .map(|(namespace, error)| format!("metrics for namespace {namespace}: {error}"))
        .collect::<Vec<_>>();

    (
        deployments,
        errors.into_iter().collect(),
        metrics_errors_vec,
    )
}

async fn runtime_deployments(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<RuntimeDeploymentsResponse>, Response> {
    require_auth(&headers, &state)?;
    let terminal_base = if state.config.terminal_enabled {
        "/bastion/terminal"
    } else {
        ""
    };
    // Logs are read-only and don't require pods/exec, so they are exposed
    // even when BASTION_TERMINAL_ENABLED=false. The actual route returns
    // 403 if pods/log permission is missing.
    let logs_base = "/bastion/logs/ws";
    let (deployments, errors, metrics_errors) =
        managed_deployment_infos(Arc::clone(&state.config), terminal_base, logs_base).await;

    Ok(Json(RuntimeDeploymentsResponse {
        ok: errors.is_empty(),
        service: SERVICE_NAME,
        generated_at_ms: now_millis(),
        terminal_enabled: state.config.terminal_enabled,
        metrics_available: metrics_errors.is_empty(),
        deployments,
        errors,
        metrics_errors,
    }))
}

fn find_managed_deployment(namespace: &str, deployment: &str) -> Option<ManagedDeployment> {
    MANAGED_DEPLOYMENTS
        .iter()
        .copied()
        .find(|target| target.namespace == namespace && target.deployment == deployment)
}

/// Shared allowlist + selector check for terminal exec and log streaming.
///
/// Both endpoints take the same `namespace + deployment + pod + container`
/// quad and need to confirm the deployment is in the managed allowlist, the
/// pod is actually selected by the deployment, and the named container
/// exists on that pod. The exec endpoint additionally requires that
/// `BASTION_TERMINAL_ENABLED` is true; logs do not.
async fn resolve_pod_target(
    config: &Config,
    query: TerminalQuery,
    require_terminal: bool,
) -> Result<TerminalTarget, Response> {
    if require_terminal && !config.terminal_enabled {
        return Err((
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "terminal sessions are disabled" })),
        )
            .into_response());
    }

    if !safe_name(&query.namespace)
        || !safe_name(&query.deployment)
        || !safe_name(&query.pod)
        || !safe_name(&query.container)
    {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "invalid Kubernetes target name" })),
        )
            .into_response());
    }

    let Some(target) = find_managed_deployment(&query.namespace, &query.deployment) else {
        return Err((
            StatusCode::FORBIDDEN,
            Json(
                json!({ "error": "deployment is not in the bastion managed allowlist" }),
            ),
        )
            .into_response());
    };

    let deployment = kubectl_json(
        config,
        &[
            "-n".to_string(),
            target.namespace.to_string(),
            "get".to_string(),
            "deployment".to_string(),
            target.deployment.to_string(),
            "-o".to_string(),
            "json".to_string(),
        ],
    )
    .await
    .map_err(|error| {
        (
            StatusCode::BAD_GATEWAY,
            Json(json!({ "error": "failed to load deployment", "errMessage": error })),
        )
            .into_response()
    })?;

    let pod = kubectl_json(
        config,
        &[
            "-n".to_string(),
            target.namespace.to_string(),
            "get".to_string(),
            "pod".to_string(),
            query.pod.clone(),
            "-o".to_string(),
            "json".to_string(),
        ],
    )
    .await
    .map_err(|error| {
        (
            StatusCode::BAD_GATEWAY,
            Json(json!({ "error": "failed to load pod", "errMessage": error })),
        )
            .into_response()
    })?;

    if !selector_matches_pod(&deployment, &pod) {
        return Err((
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "pod is not selected by the requested deployment" })),
        )
            .into_response());
    }

    let has_container = json_at(&pod, &["status", "containerStatuses"])
        .and_then(Value::as_array)
        .is_some_and(|containers| {
            containers.iter().any(|container| {
                json_at_string(container, &["name"]).as_deref() == Some(query.container.as_str())
            })
        });
    if !has_container {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "container was not found on the selected pod" })),
        )
            .into_response());
    }

    Ok(TerminalTarget {
        namespace: query.namespace,
        deployment: query.deployment,
        pod: query.pod,
        container: query.container,
    })
}

async fn validate_terminal_target(
    config: &Config,
    query: TerminalQuery,
) -> Result<TerminalTarget, Response> {
    resolve_pod_target(config, query, true).await
}

fn terminal_page_html(target: &TerminalTarget) -> String {
    let namespace = html_escape(&target.namespace);
    let deployment = html_escape(&target.deployment);
    let pod = html_escape(&target.pod);
    let container = html_escape(&target.container);
    format!(
        r##"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Bastion terminal</title>
  <link rel="stylesheet" href="https://cdn.jsdelivr.net/npm/@xterm/xterm@5.5.0/css/xterm.css">
  <script defer src="https://cdn.jsdelivr.net/npm/@xterm/xterm@5.5.0/lib/xterm.js"></script>
  <style>
    :root {{ color-scheme: dark; --bg: #0d1117; --line: #263244; --text: #e5edf7; --muted: #9aa7b7; }}
    * {{ box-sizing: border-box; }}
    body {{ margin: 0; min-height: 100dvh; background: var(--bg); color: var(--text); font-family: ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; }}
    main {{ min-height: 100dvh; display: grid; grid-template-rows: auto minmax(0, 1fr); }}
    header {{ display: flex; justify-content: space-between; gap: 12px; align-items: center; padding: 12px 14px; border-bottom: 1px solid var(--line); background: #0f172a; }}
    h1 {{ margin: 0; font-size: 16px; }}
    p {{ margin: 3px 0 0; color: var(--muted); font-size: 12px; }}
    #status {{ color: var(--muted); font-size: 13px; }}
    #terminal {{ min-height: 0; padding: 10px; background: #05080d; }}
    #terminal .xterm {{ height: 100%; }}
  </style>
</head>
<body>
  <main>
    <header>
      <div>
        <h1>{deployment}</h1>
        <p>{namespace}/{pod}/{container}</p>
      </div>
      <span id="status">connecting</span>
    </header>
    <div id="terminal" aria-label="Bastion container terminal"></div>
  </main>
  <script>
    const statusNode = document.getElementById("status");
    const terminalNode = document.getElementById("terminal");
    let term;
    function write(value) {{
      if (term) term.write(String(value || ""));
    }}
    function connect() {{
      if (!window.Terminal) {{
        statusNode.textContent = "terminal assets failed";
        terminalNode.textContent = "Terminal assets failed to load.";
        return;
      }}
      term = new Terminal({{
        cursorBlink: true,
        convertEol: true,
        fontSize: 13,
        rows: 32,
        scrollback: 5000,
        theme: {{
          background: "#05080d",
          foreground: "#d5f5e3",
          cursor: "#7dd3fc",
          selectionBackground: "#1f6feb66"
        }}
      }});
      term.open(terminalNode);
      term.focus();
      const url = new URL("terminal/ws", window.location.href);
      url.protocol = window.location.protocol === "https:" ? "wss:" : "ws:";
      for (const [key, value] of new URLSearchParams(window.location.search)) {{
        url.searchParams.set(key, value);
      }}
      const socket = new WebSocket(url);
      socket.addEventListener("open", () => {{
        statusNode.textContent = "connected";
        term.onData((data) => {{
          if (socket.readyState === WebSocket.OPEN) socket.send(JSON.stringify({{ type: "input", data }}));
        }});
      }});
      socket.addEventListener("message", (event) => {{
        let message;
        try {{ message = JSON.parse(event.data); }} catch {{ write(String(event.data)); return; }}
        if (message.type === "terminal-output") write(String(message.data || ""));
        if (message.type === "terminal-status") statusNode.textContent = String(message.status || "status");
        if (message.type === "terminal-error") {{
          statusNode.textContent = "error";
          write("\r\n" + String(message.message || "terminal error") + "\r\n");
        }}
        if (message.type === "terminal-exit") statusNode.textContent = "closed";
      }});
      socket.addEventListener("close", () => {{ statusNode.textContent = "closed"; }});
      socket.addEventListener("error", () => {{ statusNode.textContent = "connection error"; }});
    }}
    window.addEventListener("load", connect);
  </script>
</body>
</html>"##
    )
}

async fn terminal_page(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<TerminalQuery>,
) -> Result<Html<String>, Response> {
    require_auth(&headers, &state)?;
    let target = validate_terminal_target(&state.config, query).await?;
    Ok(Html(terminal_page_html(&target)))
}

async fn terminal_ws(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<TerminalQuery>,
    ws: WebSocketUpgrade,
) -> Response {
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }

    let target = match validate_terminal_target(&state.config, query).await {
        Ok(target) => target,
        Err(response) => return response,
    };
    let kubectl_bin = state.config.kubectl_bin.clone();
    let script_bin = state.config.script_bin.clone();
    ws.on_upgrade(move |socket| handle_terminal_socket(socket, kubectl_bin, script_bin, target))
}

async fn send_terminal_json(socket: &mut WebSocket, payload: Value) -> bool {
    socket
        .send(Message::Text(payload.to_string()))
        .await
        .is_ok()
}

fn terminal_input(text: &str) -> String {
    match serde_json::from_str::<Value>(text) {
        Ok(value) => value
            .get("data")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        Err(_) => text.to_string(),
    }
}

async fn handle_terminal_socket(
    mut socket: WebSocket,
    kubectl_bin: String,
    script_bin: String,
    target: TerminalTarget,
) {
    let _ = send_terminal_json(
        &mut socket,
        json!({
            "type": "terminal-status",
            "source": "dd-bastion",
            "status": "starting-shell",
            "namespace": target.namespace,
            "deployment": target.deployment,
            "pod": target.pod,
            "container": target.container,
            "atMs": now_millis(),
        }),
    )
    .await;

    let use_pty = executable_available(&script_bin).await;
    let mut command = if use_pty {
        let kubectl_command = format!(
            "{} -n {} exec -it {} -c {} -- /bin/sh -lc {}",
            shell_quote(&kubectl_bin),
            shell_quote(&target.namespace),
            shell_quote(&target.pod),
            shell_quote(&target.container),
            shell_quote(TERMINAL_SHELL)
        );
        let mut command = Command::new(&script_bin);
        command.args(["-q", "-f", "-e", "-c", &kubectl_command, "/dev/null"]);
        command
    } else {
        let mut command = Command::new(&kubectl_bin);
        command.args([
            "-n",
            &target.namespace,
            "exec",
            "-i",
            &target.pod,
            "-c",
            &target.container,
            "--",
            "/bin/sh",
            "-lc",
            TERMINAL_SHELL,
        ]);
        command
    };
    let mut child = match command
        .env("TERM", "xterm-256color")
        .env("COLUMNS", "120")
        .env("LINES", "32")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            let _ = send_terminal_json(
                &mut socket,
                json!({
                    "type": "terminal-error",
                    "source": "dd-bastion",
                    "message": format!("failed to start kubectl exec: {error}"),
                    "atMs": now_millis(),
                }),
            )
            .await;
            return;
        }
    };

    let mut stdin = match child.stdin.take() {
        Some(stdin) => stdin,
        None => return,
    };
    let mut stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => return,
    };
    let mut stderr = match child.stderr.take() {
        Some(stderr) => stderr,
        None => return,
    };
    let mut stdout_open = true;
    let mut stderr_open = true;
    let mut stdout_buf = [0_u8; 4096];
    let mut stderr_buf = [0_u8; 4096];

    let _ = send_terminal_json(
        &mut socket,
        json!({
            "type": "terminal-status",
            "source": "dd-bastion",
            "status": "connected",
            "transport": if use_pty { "pty-script-kubectl" } else { "kubectl-pipe-fallback" },
            "atMs": now_millis(),
        }),
    )
    .await;

    loop {
        tokio::select! {
            message = socket.recv() => {
                match message {
                    Some(Ok(Message::Text(text))) => {
                        let data = terminal_input(&text);
                        if !data.is_empty() && stdin.write_all(data.as_bytes()).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(Message::Binary(bytes))) => {
                        if stdin.write_all(&bytes).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(Message::Ping(bytes))) => {
                        let _ = socket.send(Message::Pong(bytes)).await;
                    }
                    Some(Ok(Message::Close(_))) | None => {
                        break;
                    }
                    Some(Ok(_)) => {}
                    Some(Err(_)) => break,
                }
            }
            read = stdout.read(&mut stdout_buf), if stdout_open => {
                match read {
                    Ok(0) => stdout_open = false,
                    Ok(n) => {
                        if !send_terminal_json(&mut socket, json!({
                            "type": "terminal-output",
                            "source": "dd-bastion",
                            "data": String::from_utf8_lossy(&stdout_buf[..n]).to_string(),
                            "atMs": now_millis(),
                        })).await {
                            break;
                        }
                    }
                    Err(_) => stdout_open = false,
                }
            }
            read = stderr.read(&mut stderr_buf), if stderr_open => {
                match read {
                    Ok(0) => stderr_open = false,
                    Ok(n) => {
                        if !send_terminal_json(&mut socket, json!({
                            "type": "terminal-output",
                            "source": "dd-bastion",
                            "data": String::from_utf8_lossy(&stderr_buf[..n]).to_string(),
                            "atMs": now_millis(),
                        })).await {
                            break;
                        }
                    }
                    Err(_) => stderr_open = false,
                }
            }
            status = child.wait() => {
                let (code, signal) = match status {
                    Ok(status) => (status.code(), None::<String>),
                    Err(error) => (None, Some(error.to_string())),
                };
                let _ = send_terminal_json(&mut socket, json!({
                    "type": "terminal-exit",
                    "source": "dd-bastion",
                    "code": code,
                    "signal": signal,
                    "atMs": now_millis(),
                })).await;
                break;
            }
        }

        if !stdout_open && !stderr_open {
            break;
        }
    }

    let _ = child.kill().await;
}

/// `/logs/ws`: stream `kubectl logs -f --tail=N` for an allowlisted pod.
///
/// Lighter than terminal exec because it only requires the `pods/log` read
/// verb that the read-only `dd-bastion-readonly` ClusterRole already grants.
/// The bastion deliberately runs `kubectl logs` rather than streaming the
/// `pods/log` HTTP endpoint directly so it inherits the same auth + cluster
/// resolution path the rest of the service uses.
async fn logs_ws(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<TerminalQuery>,
    ws: WebSocketUpgrade,
) -> Response {
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }

    let target = match resolve_pod_target(&state.config, query, false).await {
        Ok(target) => target,
        Err(response) => return response,
    };

    let kubectl_bin = state.config.kubectl_bin.clone();
    ws.on_upgrade(move |socket| handle_logs_socket(socket, kubectl_bin, target))
}

async fn handle_logs_socket(mut socket: WebSocket, kubectl_bin: String, target: TerminalTarget) {
    let _ = send_terminal_json(
        &mut socket,
        json!({
            "type": "logs-status",
            "source": "dd-bastion",
            "status": "starting-logs",
            "namespace": target.namespace,
            "deployment": target.deployment,
            "pod": target.pod,
            "container": target.container,
            "atMs": now_millis(),
        }),
    )
    .await;

    let mut command = Command::new(&kubectl_bin);
    command.args([
        "-n",
        &target.namespace,
        "logs",
        "-f",
        "--tail=500",
        &target.pod,
        "-c",
        &target.container,
    ]);

    let mut child = match command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            let _ = send_terminal_json(
                &mut socket,
                json!({
                    "type": "logs-error",
                    "source": "dd-bastion",
                    "message": format!("failed to start kubectl logs: {error}"),
                    "atMs": now_millis(),
                }),
            )
            .await;
            return;
        }
    };

    let mut stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => return,
    };
    let mut stderr = match child.stderr.take() {
        Some(stderr) => stderr,
        None => return,
    };
    let mut stdout_open = true;
    let mut stderr_open = true;
    let mut stdout_buf = [0_u8; 4096];
    let mut stderr_buf = [0_u8; 4096];

    let _ = send_terminal_json(
        &mut socket,
        json!({
            "type": "logs-status",
            "source": "dd-bastion",
            "status": "streaming",
            "atMs": now_millis(),
        }),
    )
    .await;

    loop {
        tokio::select! {
            message = socket.recv() => {
                match message {
                    Some(Ok(Message::Ping(bytes))) => {
                        let _ = socket.send(Message::Pong(bytes)).await;
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(_)) => {}
                    Some(Err(_)) => break,
                }
            }
            read = stdout.read(&mut stdout_buf), if stdout_open => {
                match read {
                    Ok(0) => stdout_open = false,
                    Ok(n) => {
                        if !send_terminal_json(&mut socket, json!({
                            "type": "logs-output",
                            "source": "dd-bastion",
                            "stream": "stdout",
                            "data": String::from_utf8_lossy(&stdout_buf[..n]).to_string(),
                            "atMs": now_millis(),
                        })).await {
                            break;
                        }
                    }
                    Err(_) => stdout_open = false,
                }
            }
            read = stderr.read(&mut stderr_buf), if stderr_open => {
                match read {
                    Ok(0) => stderr_open = false,
                    Ok(n) => {
                        if !send_terminal_json(&mut socket, json!({
                            "type": "logs-output",
                            "source": "dd-bastion",
                            "stream": "stderr",
                            "data": String::from_utf8_lossy(&stderr_buf[..n]).to_string(),
                            "atMs": now_millis(),
                        })).await {
                            break;
                        }
                    }
                    Err(_) => stderr_open = false,
                }
            }
            status = child.wait() => {
                let (code, signal) = match status {
                    Ok(status) => (status.code(), None::<String>),
                    Err(error) => (None, Some(error.to_string())),
                };
                let _ = send_terminal_json(&mut socket, json!({
                    "type": "logs-exit",
                    "source": "dd-bastion",
                    "code": code,
                    "signal": signal,
                    "atMs": now_millis(),
                })).await;
                break;
            }
        }

        if !stdout_open && !stderr_open {
            break;
        }
    }

    let _ = child.kill().await;
}

async fn api_docs_html() -> axum::response::Html<&'static str> {
    axum::response::Html(include_str!("../generated/api-docs.html"))
}

async fn api_docs_json() -> impl axum::response::IntoResponse {
    (
        [("content-type", "application/json; charset=utf-8")],
        include_str!("../generated/api-docs.json"),
    )
}

#[tokio::main]
async fn main() {
    let host = env_value("HOST", "0.0.0.0");
    let port = first_env(&["PORT", "BASTION_PORT"])
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(DEFAULT_PORT);
    let addr: SocketAddr = format!("{host}:{port}")
        .parse()
        .expect("HOST and PORT must form a valid socket address");
    let state = AppState {
        config: Arc::new(config_from_env()),
    };

    let app = Router::new()
        .route("/healthz", get(healthz))
        .route("/docs/api", get(api_docs_html))
        .route("/api/docs", get(api_docs_html))
        .route("/api/docs.json", get(api_docs_json))
        .route("/profile", get(profile))
        .route("/config", get(profile))
        .route("/kubeconfig", get(kubeconfig))
        .route("/runtime/deployments", get(runtime_deployments))
        .route("/deployments", get(runtime_deployments))
        .route("/terminal", get(terminal_page))
        .route("/terminal/ws", get(terminal_ws))
        .route("/logs/ws", get(logs_ws))
        .with_state(state)
        .merge(dd_runtime_config_client::router());

    tokio::spawn(dd_runtime_config_client::register_with_control_plane());

    println!("{SERVICE_NAME} listening on {addr}");
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("failed to bind bastion listener");
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = signal::ctrl_c().await;
        })
        .await
        .expect("bastion server failed");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_cpu_handles_nano_micro_milli_and_whole_cores() {
        // metrics-server reports nano-CPU; one full core = 1_000_000_000 n.
        assert_eq!(parse_cpu_millicores("5000000n"), Some(5));
        assert_eq!(parse_cpu_millicores("1000000000n"), Some(1000));
        assert_eq!(parse_cpu_millicores("250m"), Some(250));
        assert_eq!(parse_cpu_millicores("100u"), Some(0));
        assert_eq!(parse_cpu_millicores("2"), Some(2_000));
        assert_eq!(parse_cpu_millicores("0.5"), Some(500));
        assert_eq!(parse_cpu_millicores(""), None);
    }

    #[test]
    fn parse_memory_handles_binary_and_decimal_suffixes() {
        assert_eq!(parse_memory_bytes("1024Ki"), Some(1_048_576));
        assert_eq!(parse_memory_bytes("256Mi"), Some(268_435_456));
        assert_eq!(parse_memory_bytes("2Gi"), Some(2_147_483_648));
        assert_eq!(parse_memory_bytes("500M"), Some(500_000_000));
        assert_eq!(parse_memory_bytes("12345"), Some(12_345));
        assert_eq!(parse_memory_bytes(""), None);
    }

    #[test]
    fn build_metrics_lookup_groups_pods_and_containers() {
        let payload = json!({
            "kind": "PodMetricsList",
            "items": [
                {
                    "metadata": { "name": "dd-bastion-abc", "namespace": "vpn" },
                    "containers": [
                        { "name": "bastion", "usage": { "cpu": "12000000n", "memory": "32Mi" } },
                    ],
                },
                {
                    "metadata": { "name": "dd-billing-server-xyz", "namespace": "default" },
                    "containers": [
                        { "name": "server", "usage": { "cpu": "0", "memory": "0" } },
                        { "name": "sidecar", "usage": { "cpu": "5m", "memory": "1Mi" } },
                    ],
                },
            ],
        });

        let lookup = build_metrics_lookup(&payload);
        let bastion = lookup
            .get(&("dd-bastion-abc".to_string(), "bastion".to_string()))
            .copied()
            .expect("bastion entry");
        assert_eq!(bastion.cpu_millicores, 12);
        assert_eq!(bastion.memory_bytes, 32 * 1024 * 1024);

        let sidecar = lookup
            .get(&("dd-billing-server-xyz".to_string(), "sidecar".to_string()))
            .copied()
            .expect("sidecar entry");
        assert_eq!(sidecar.cpu_millicores, 5);
        assert_eq!(sidecar.memory_bytes, 1024 * 1024);
    }
}
