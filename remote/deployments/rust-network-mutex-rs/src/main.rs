//! Entrypoint for the `dd-rust-network-mutex` broker.
//!
//! All configuration comes from environment variables so the binary plays
//! nicely with `envFrom: secretRef` and ConfigMap-style Kubernetes wiring.
//!
//! Required: at least one of `LMX_TCP_PORT`, `LMX_UDS_PATH`, or
//! `LMX_HTTP_PORT` must produce a listener. Defaults bind TCP on 6970 and HTTP
//! on 6971; UDS is off unless `LMX_UDS_PATH` is set.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use dd_rust_network_mutex::{
    server::{self, ServerConfig},
    BrokerConfig,
};
use tracing::info;
#[cfg(feature = "tls")]
use tracing::warn;

#[tokio::main]
async fn main() -> std::io::Result<()> {
    init_tracing();
    let config = config_from_env();
    log_startup(&config);
    server::run(config).await
}

fn init_tracing() {
    let format = std::env::var("LMX_LOG_FORMAT").unwrap_or_else(|_| "text".into());
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let subscriber = tracing_subscriber::fmt().with_env_filter(filter);
    if format == "json" {
        let _ = subscriber.json().try_init();
    } else {
        let _ = subscriber.try_init();
    }
}

fn first_env(keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|k| {
        std::env::var(k)
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    })
}

fn env_u16(key: &str, fallback: u16) -> u16 {
    first_env(&[key])
        .and_then(|v| v.parse::<u16>().ok())
        .unwrap_or(fallback)
}

fn env_u64(key: &str, fallback: u64) -> u64 {
    first_env(&[key])
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(fallback)
}

fn env_u32(key: &str, fallback: u32) -> u32 {
    first_env(&[key])
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(fallback)
}

fn env_bool(key: &str, fallback: bool) -> bool {
    first_env(&[key])
        .map(|v| matches!(v.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
        .unwrap_or(fallback)
}

fn config_from_env() -> ServerConfig {
    let tcp_port = env_u16("LMX_TCP_PORT", 6970);
    let http_port = env_u16("LMX_HTTP_PORT", 6971);
    let bind_host = first_env(&["LMX_BIND_HOST"]).unwrap_or_else(|| "0.0.0.0".into());

    let tcp_bind = if env_bool("LMX_DISABLE_TCP", false) {
        None
    } else {
        Some(format!("{bind_host}:{tcp_port}").parse::<SocketAddr>().expect("invalid LMX_BIND_HOST/LMX_TCP_PORT"))
    };
    let http_bind = if env_bool("LMX_DISABLE_HTTP", false) {
        None
    } else {
        Some(format!("{bind_host}:{http_port}").parse::<SocketAddr>().expect("invalid LMX_BIND_HOST/LMX_HTTP_PORT"))
    };

    // Optional dedicated HTML status page listener — upstream
    // `live-mutex#108`. Default off because the same page is also
    // available on the main HTTP listener at `/` and `/status`.
    let status_bind = first_env(&["LMX_STATUS_PORT"])
        .and_then(|v| v.parse::<u16>().ok())
        .map(|p| {
            format!("{bind_host}:{p}")
                .parse::<SocketAddr>()
                .expect("invalid LMX_BIND_HOST/LMX_STATUS_PORT")
        });

    let uds_path = first_env(&["LMX_UDS_PATH"]).map(PathBuf::from);
    let auth_token = first_env(&["LMX_AUTH_TOKEN"]);

    let broker = BrokerConfig {
        default_ttl: Duration::from_millis(env_u64("LMX_DEFAULT_TTL_MS", 4000)),
        max_lock_holders: env_u32("LMX_MAX_LOCK_HOLDERS", 1).max(1),
        // Single periodic sweep (upstream live-mutex#13) instead of
        // per-request timers. 0 disables auto-eviction entirely.
        ttl_sweep_interval: Duration::from_millis(env_u64("LMX_TTL_SWEEP_INTERVAL_MS", 10)),
        // Hard ceiling on per-key semaphore concurrency. A `lock`
        // request whose `max` exceeds this is silently clamped and
        // counted in `dd_rust_network_mutex_concurrency_cap_clamps_total`.
        max_concurrency_cap: env_u32(
            "LMX_MAX_CONCURRENCY_CAP",
            dd_rust_network_mutex::protocol::DEFAULT_MAX_CONCURRENCY_CAP,
        )
        .max(1),
    };

    #[cfg(feature = "tls")]
    let tls = build_tls_config();

    ServerConfig {
        tcp_bind,
        uds_path,
        http_bind,
        auth_token,
        broker,
        // Socket-tuning experiment from upstream `live-mutex#22`.
        // Defaults match the upstream issue's recommendation: NODELAY on,
        // QUICKACK on. Both can be flipped independently for A/B testing.
        tcp_nodelay: env_bool("LMX_TCP_NODELAY", true),
        tcp_quickack: env_bool("LMX_TCP_QUICKACK", true),
        status_bind,
        #[cfg(feature = "tls")]
        tls,
    }
}

#[cfg(feature = "tls")]
fn build_tls_config() -> Option<server::TlsConfig> {
    match (first_env(&["LMX_TLS_CERT"]), first_env(&["LMX_TLS_KEY"])) {
        (Some(cert), Some(key)) => Some(server::TlsConfig {
            cert_path: PathBuf::from(cert),
            key_path: PathBuf::from(key),
        }),
        (Some(_), None) | (None, Some(_)) => {
            warn!("LMX_TLS_CERT and LMX_TLS_KEY must both be set; TLS disabled");
            None
        }
        _ => None,
    }
}

fn log_startup(config: &ServerConfig) {
    info!(
        target: "lmx",
        tcp = ?config.tcp_bind,
        uds = ?config.uds_path,
        http = ?config.http_bind,
        status = ?config.status_bind,
        auth = config.auth_token.is_some(),
        max_lock_holders = config.broker.max_lock_holders,
        max_concurrency_cap = config.broker.max_concurrency_cap,
        default_ttl_ms = config.broker.default_ttl.as_millis() as u64,
        ttl_sweep_interval_ms = config.broker.ttl_sweep_interval.as_millis() as u64,
        tcp_nodelay = config.tcp_nodelay,
        // `tcp_quickack` is configured but only effective on Linux.
        // Surfacing the *effective* value (`& quickack_supported`) prevents
        // a confusing log on darwin where the option is a no-op.
        tcp_quickack_effective =
            config.tcp_quickack && dd_rust_network_mutex::sockopt::quickack_supported(),
        protocol = dd_rust_network_mutex::PROTOCOL_VERSION,
        "starting dd-rust-network-mutex"
    );
}
