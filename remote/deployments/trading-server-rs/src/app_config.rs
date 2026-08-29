use std::{io::BufReader, sync::atomic::Ordering};

use dd_nats_subject_defs::cdc_table_filter_subject;
use serde_json::{json, Value};
use tokio::time::sleep;

use crate::platforms::{
    default_platform_config, platform_config_from_app_config_value, TradingPlatformConfig,
};
use crate::state::{AppState, Config};
use crate::util::env_value;

fn is_missing_app_config_table_error(error: &str) -> bool {
    error.contains("relation \"app_config\" does not exist")
}

fn row_value(row: &tokio_postgres::Row, column: &str, fallback: Value) -> Value {
    row.try_get::<_, Value>(column).unwrap_or(fallback)
}

fn add_rds_root_certificates(root_store: &mut rustls::RootCertStore) -> Result<(), String> {
    let mut reader = BufReader::new(&include_bytes!("../certs/rds-us-east-1-bundle.pem")[..]);
    let mut added = 0usize;

    for cert in rustls_pemfile::certs(&mut reader) {
        let cert = cert.map_err(|error| format!("failed to parse RDS CA certificate: {error}"))?;
        if root_store.add(cert).is_ok() {
            added += 1;
        }
    }

    if added == 0 {
        return Err("no RDS CA certificates loaded".to_string());
    }

    Ok(())
}

async fn connect_postgres(config: &Config) -> Result<tokio_postgres::Client, String> {
    let database_url = config
        .database_url
        .as_deref()
        .ok_or_else(|| "trading database URL is not configured".to_string())?;
    let mut root_store = rustls::RootCertStore::empty();
    root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    add_rds_root_certificates(&mut root_store)?;
    let tls_config = rustls::ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth();
    let tls = tokio_postgres_rustls::MakeRustlsConnect::new(tls_config);
    let (client, connection) = tokio_postgres::connect(database_url, tls)
        .await
        .map_err(|error| error.to_string())?;

    tokio::spawn(async move {
        if let Err(error) = connection.await {
            tracing::error!("trading server postgres connection error: {error}");
        }
    });
    Ok(client)
}

async fn fetch_platform_config_from_app_config(
    client: &tokio_postgres::Client,
    config: &Config,
) -> Result<Option<TradingPlatformConfig>, String> {
    let rows = client
        .query(
            r#"
            select value
            from app_config
            where scope = $1
              and key = $2
              and status = 'active'
              and is_soft_deleted = false
            order by updated_at desc
            limit 1
            "#,
            &[&config.app_config_scope, &config.app_config_key],
        )
        .await
        .map_err(|error| error.to_string())?;
    let Some(row) = rows.first() else {
        return Ok(None);
    };
    platform_config_from_app_config_value(row_value(row, "value", json!({}))).map(Some)
}

async fn fetch_platform_config(config: &Config) -> Result<TradingPlatformConfig, String> {
    let Some(_) = config.database_url.as_ref() else {
        return Ok(default_platform_config());
    };
    let client = connect_postgres(config).await?;
    match fetch_platform_config_from_app_config(&client, config).await {
        Ok(Some(next)) => Ok(next),
        Ok(None) => Err(format!(
            "missing active app_config row scope={} key={}",
            config.app_config_scope, config.app_config_key
        )),
        Err(error) if is_missing_app_config_table_error(&error) => {
            tracing::error!("trading app_config table is missing; using built-in platform defaults");
            Ok(default_platform_config())
        }
        Err(error) => Err(error),
    }
}

fn store_platform_config(state: &AppState, next: TradingPlatformConfig) -> Result<(), String> {
    let mut current = state
        .platform_config
        .write()
        .map_err(|_| "trading platform config lock is poisoned".to_string())?;
    *current = next;
    Ok(())
}

pub(crate) async fn refresh_platform_config(state: &AppState) -> Result<(), String> {
    let next = fetch_platform_config(&state.config).await?;
    store_platform_config(state, next)?;
    state
        .metrics
        .config_refresh_total
        .fetch_add(1, Ordering::Relaxed);
    Ok(())
}

pub(crate) async fn record_config_error(state: &AppState, error: String) {
    state
        .metrics
        .config_refresh_failures_total
        .fetch_add(1, Ordering::Relaxed);
    if let Ok(mut config) = state.platform_config.write() {
        config.last_config_error = Some(error);
    }
}

pub(crate) async fn run_config_refresh_loop(state: AppState) {
    loop {
        sleep(state.config.config_refresh).await;
        if let Err(error) = refresh_platform_config(&state).await {
            tracing::error!("trading platform config refresh failed: {error}");
            record_config_error(&state, error).await;
        }
    }
}

/// Subscribe to the WAL gateway's CDC stream and refresh the trading
/// platform config the instant `app_config` changes are committed.
///
/// We deliberately keep the wider poll-based refresh loop alive: CDC can
/// drop messages if JetStream is down or the consumer is far enough
/// behind that the broker has aged messages out of the stream. The poll
/// loop is the catch-up path.
///
/// The handler filters down to the specific scope+key tuple this server
/// cares about (`trading.platforms.v1` by default) so unrelated
/// `app_config` rows don't trigger refreshes — saves a Postgres query
/// per noisy write.
pub(crate) async fn run_cdc_refresh_subscription(state: AppState) {
    let Some(nats) = state.nats.clone() else {
        tracing::info!("trading server cdc subscription disabled: no NATS_URL configured");
        return;
    };
    let jetstream = async_nats::jetstream::new(nats);
    let scope = state.config.app_config_scope.clone();
    let key = state.config.app_config_key.clone();
    let label = format!(
        "dd-trading-server-app-config-{}",
        sanitize_for_durable_name(&format!("{scope}.{key}"))
    );
    let trigger_state = state.clone();
    let builder = dd_wal_consumer::Subscription::builder()
        .stream(env_value("TRADING_CDC_STREAM", "CDC"))
        .durable_name(label.clone())
        .filter_subject(cdc_table_filter_subject("cdc", "public", "app_config"));
    let start = builder
        .start(&jetstream, move |change: dd_wal_consumer::RowChange| {
            let scope = scope.clone();
            let key = key.clone();
            let task_state = trigger_state.clone();
            async move {
                // The gateway sends every row in `app_config`. Skip rows
                // for other scopes/keys entirely; this is what makes the
                // CDC path cheap even when the table is busy with other
                // services' configs.
                let row_scope = change
                    .column("scope")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                let row_key = change
                    .column("key")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                if row_scope.as_deref() != Some(&scope) || row_key.as_deref() != Some(&key) {
                    return;
                }
                if let Err(error) = refresh_platform_config(&task_state).await {
                    tracing::error!(
                        "trading platform CDC-driven refresh failed (scope={scope} key={key}): \
                         {error}"
                    );
                    record_config_error(&task_state, error).await;
                }
            }
        })
        .await;
    match start {
        Ok(_join) => {
            tracing::info!(
                "trading server cdc subscription started: durable={label} \
                 subject=cdc.public.app_config.>"
            );
        }
        Err(error) => {
            tracing::error!(
                "trading server cdc subscription failed to start ({error}); \
                 falling back to poll-only refresh"
            );
        }
    }
}

fn sanitize_for_durable_name(input: &str) -> String {
    input
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}
