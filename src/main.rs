//! Entrypoint for the `dd-rust-network-mutex` broker.
//!
//! Configuration comes from optional TOML defaults plus environment-variable
//! overrides, so the binary still plays nicely with `envFrom: secretRef` and
//! ConfigMap-style Kubernetes wiring.
//!
//! Required: at least one of `LMX_TCP_PORT`, `LMX_UDS_PATH`, or
//! `LMX_HTTP_PORT` must produce a listener. Defaults bind TCP on 6970 and HTTP
//! on 6971; UDS is off unless `LMX_UDS_PATH` is set.

use std::path::Path;

use dd_rust_network_mutex::{
    cli_flags::{self, BrokerCliConfig},
    config, routine_id,
    server::{self, ServerConfig},
    BrokerRaftConfig,
};
use tracing::info;

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let cli_config = cli_flags::load_broker_cli_config()
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err.to_string()))?;
    let cli_env = match cli_config {
        BrokerCliConfig::Run(cli_env) => cli_env,
        BrokerCliConfig::Help(help) => {
            print!("{}", help.table());
            return Ok(());
        }
    };
    cli_env.apply_cli_overrides_to_process_env();

    // Initialise tracing/OTel BEFORE we touch the macro, so the very first
    // `info!("enter")` line lands on a real subscriber. The init helper is
    // idempotent.
    dd_rust_network_mutex::init_tracing();
    routine_id!("ddl-routine-jmlgJQLHYkR5XOM7Ck");

    let runtime = config::load_runtime_config()
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err.to_string()))?;
    log_startup(
        &runtime.server,
        &runtime.raft,
        runtime.source_path.as_deref(),
    );
    let result = if runtime.raft.enabled {
        server::run_raft(runtime.server, runtime.raft).await
    } else {
        server::run(runtime.server).await
    };
    dd_rust_network_mutex::shutdown_tracing();
    result
}

fn log_startup(config: &ServerConfig, raft: &BrokerRaftConfig, config_path: Option<&Path>) {
    routine_id!("ddl-routine-glxWxKt782M6i2YFXl");

    info!(
        target: "lmx",
        routine_id = ROUTINE_ID,
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
        tcp_quickack_effective =
            config.tcp_quickack && dd_rust_network_mutex::sockopt::quickack_supported(),
        config_path = ?config_path,
        raft_enabled = raft.enabled,
        raft_node_id = %raft.node_id,
        raft_cluster_size = raft.cluster_size(),
        raft_quorum_size = raft.quorum_size(),
        raft_data_dir = ?raft.data_dir,
        protocol = dd_rust_network_mutex::PROTOCOL_VERSION,
        "starting dd-rust-network-mutex"
    );
}
