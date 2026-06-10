//! Optional Postgres CDC consumer.
//!
//! Billing's primary database is separate from the shared pg-defs RDS that
//! the central `wal-gateway-rs` service streams from. Most deployments
//! therefore won't enable this — but the plumbing is here so an operator
//! can wire it on with three env vars when they want a coalesced event /
//! audit feed (cache invalidation, search-index updates, cross-region
//! mirroring, etc.) without writing a new consumer from scratch.
//!
//! ## Enabling
//!
//! ```text
//! BILLING_CDC_NATS_URL=nats://dd-nats.messaging.svc.cluster.local:4222
//! BILLING_CDC_STREAM=CDC                       # optional (default "CDC")
//! BILLING_CDC_FILTER_SUBJECT=cdc.public.>      # optional (default "cdc.>")
//! BILLING_CDC_DURABLE_NAME=billing-server-audit
//! ```
//!
//! Once on, every row change matching the filter is logged via `tracing`
//! at INFO level with structured fields (schema, table, op, lsn, pk).
//! Maintainers replace the body of `on_change` with a real reaction when
//! the use case is known (the [examples] in the comment below show the
//! patterns we've used in trading-server / rest-api).
//!
//! [examples]: trading-server-rs/src/main.rs::run_cdc_refresh_subscription

use std::env;

pub fn first_env(keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        env::var(key)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    })
}

/// Spawn the CDC subscription, if `BILLING_CDC_NATS_URL` (or `NATS_URL`)
/// is set. Returns immediately. Silent no-op when unconfigured.
pub fn spawn() {
    tokio::spawn(async move {
        if let Err(error) = start_inner().await {
            tracing::warn!(
                error = %error,
                "billing cdc subscription not started"
            );
        }
    });
}

async fn start_inner() -> anyhow::Result<()> {
    let Some(nats_url) = first_env(&["BILLING_CDC_NATS_URL", "NATS_URL"]) else {
        tracing::info!("BILLING_CDC_NATS_URL unset; cdc subscription disabled");
        return Ok(());
    };
    let stream = first_env(&["BILLING_CDC_STREAM"]).unwrap_or_else(|| "CDC".to_string());
    let filter = first_env(&["BILLING_CDC_FILTER_SUBJECT"]).unwrap_or_else(|| "cdc.>".to_string());
    let durable = first_env(&["BILLING_CDC_DURABLE_NAME"])
        .unwrap_or_else(|| "billing-server-audit".to_string());

    // CDC audit needs NATS to do anything, so wait for the broker on a transient
    // boot outage (retry with backoff) instead of crash-looping the pod.
    let nats = async_nats::ConnectOptions::new()
        .retry_on_initial_connect()
        .connect(&nats_url)
        .await?;
    let jetstream = async_nats::jetstream::new(nats);

    let join = dd_wal_consumer::Subscription::builder()
        .stream(stream)
        .durable_name(durable.clone())
        .filter_subject(filter.clone())
        .start(
            &jetstream,
            |change: dd_wal_consumer::RowChange| async move {
                on_change(change).await;
            },
        )
        .await
        .map_err(|error| anyhow::anyhow!("dd-wal-consumer: {error}"))?;
    tracing::info!(
        durable = %durable,
        filter = %filter,
        "billing cdc subscription started"
    );
    // Hold the handle for the program's lifetime so the task isn't dropped.
    // We don't await — the task runs forever.
    std::mem::forget(join);
    Ok(())
}

/// CDC handler. Default behaviour: log the change at INFO. Override with
/// real reactions (cache invalidation, downstream publish, etc.) when the
/// use case is known.
async fn on_change(change: dd_wal_consumer::RowChange) {
    tracing::info!(
        target = "billing.cdc",
        schema = %change.schema,
        table = %change.table,
        op = ?change.op,
        lsn = %change.lsn,
        xid = ?change.xid,
        ts_ms = change.ts_ms,
        "cdc row change observed"
    );
}
