//! Routine identification + OpenTelemetry plumbing.
//!
//! ## What is a `routineId`?
//!
//! Every top-level function/method in this crate carries a *static* string
//! that looks like `ddl-routine-<nanoid>` (the `ddl-` prefix stands for
//! "distributed-locking", to make the IDs grep-friendly across other ORE
//! services that follow the same convention).
//!
//! The ID is **never generated at runtime** — it's a `&'static str` literal
//! placed in the function body. Two properties make this pattern useful:
//!
//! 1. **Grep-ability.** Given a log line that contains a routine ID, you can
//!    find the source location in one search: `rg ddl-routine-<id>`. This is
//!    significantly more reliable than fuzzy-matching log message text.
//! 2. **OTel attribute key.** The same ID is attached as a span attribute
//!    (`routine_id`) on every `tracing` span this crate opens, so it flows
//!    through `tracing-opentelemetry` into OTel/OTLP without any extra work.
//!
//! ## How to use it
//!
//! Every top-level function should start with a single line:
//!
//! ```ignore
//! routine_id!("ddl-routine-abc123…");
//! ```
//!
//! The `routine_id!` macro expands to:
//!
//! 1. A `const ROUTINE_ID: &str = "ddl-routine-…";` so the literal exists in
//!    the function's local scope and shows up in `rg` results.
//! 2. A `tracing::info_span!(...)` that names the routine and attaches
//!    `routine_id` as a structured field. The span is `entered()` and stored
//!    in a binding named `_routine_span` that lives for the rest of the
//!    function — so every `info!()` / `warn!()` / etc. emitted in this fn
//!    (and every fn called from it) inherits `routine_id` as a parent
//!    attribute when exported via OTel.
//! 3. A single `info!(routine_id = ROUTINE_ID, "enter")` log line at the
//!    function boundary so plain text/JSON log readers see the entry too.
//!
//! ### Why both a span attribute *and* an enter log?
//!
//! - The span attribute is the OTel-native way to associate the routine ID
//!   with a span and all its children. It's free in tracing's "no
//!   subscriber" mode and is the canonical handle in OTel UIs.
//! - The `info!("enter")` line is what plain log readers (`kubectl logs`,
//!   `grep`) actually see when the OTel collector is offline. Without it,
//!   the routine ID would only ever surface as a span field on _other_
//!   events emitted later in the function body — and short fns may emit
//!   no events at all.
//!
//! ## OTel exporter
//!
//! [`init_tracing`] is the single entrypoint for setting up `tracing`.
//! It always installs a `tracing-subscriber::fmt` layer (text or JSON
//! controlled by `LMX_LOG_FORMAT`). When the `otel` cargo feature is on
//! **and** `OTEL_EXPORTER_OTLP_ENDPOINT` is set in the environment, it also
//! installs a `tracing-opentelemetry` layer that exports spans + events to
//! that OTLP/gRPC endpoint. With the env unset (or feature off) the layer
//! is silently omitted — the binary stays a single-process broker that
//! writes structured logs to stdout, no extra config required.
//!
//! ## Runtime kill-switch
//!
//! Once an OTLP exporter is wired up, the OTel layer is gated by a
//! per-process atomic ([`OTEL_ENABLED`]). [`set_otel_enabled`] flips the
//! flag at runtime; the next call to any instrumented function will
//! either emit (when `true`) or skip (when `false`) its OTel span. The
//! `tracing` callsite-interest cache is rebuilt on every flip so the
//! decision takes effect immediately for already-seen call sites. The
//! stdout/JSON `tracing-subscriber::fmt` layer is **not** affected — log
//! lines (including the `lmx.routine` "enter" event) keep flowing
//! regardless of the flag.
//!
//! Operators flip the flag through the broker's HTTP admin endpoint
//! (`POST /admin/otel`) which is authenticated with a shared secret —
//! see `crate::server`.

/// Establish the routine identity for the current top-level function.
///
/// Use this as the **first statement** in every top-level fn / method:
///
/// ```ignore
/// fn handle_request(...) {
///     routine_id!("ddl-routine-abcdef0123456789xyz");
///     // ... rest of fn ...
/// }
/// ```
///
/// The macro expands to a `const`, an entered `info_span!`, and a single
/// `info!("enter", ...)` log line. Implementation lives at the crate root
/// in `src/lib.rs` so call sites in any module can reach it via
/// `$crate::routine_id!` (without an explicit `use`).
///
/// See the module-level docs for the full rationale.
#[macro_export]
macro_rules! routine_id {
    ($id:expr) => {
        // The const is the grep target. Even fns that emit no other log lines
        // still have `ROUTINE_ID` literally present in their source.
        const ROUTINE_ID: &'static str = $id;

        // We create the span and IMMEDIATELY resolve it via `in_scope`,
        // emitting the enter log inside the span context. We deliberately
        // do NOT hold an `.entered()` guard for the rest of the fn body:
        // that guard is `!Send` because it relies on thread-local state,
        // which would make every async fn that uses this macro return a
        // `!Send` future and break Axum/tokio handler bounds.
        //
        // The trade-off: the per-fn span only encloses the enter log line
        // (not the rest of the body). For OTel purposes that's still a
        // real, exportable span tagged with `routine_id`, which is what
        // the user-facing telemetry needs. Logs emitted later in the fn
        // body that want `routine_id` can pull it from the local
        // `ROUTINE_ID` const — see e.g. `warn!(routine_id = ROUTINE_ID, …)`.
        let _: () = ::tracing::info_span!(
            "routine",
            routine_id = ROUTINE_ID,
            // `code.function` follows OTel semantic conventions for code
            // attributes — it makes the OTel UI show the source location
            // alongside the routine ID.
            code.function = ::std::module_path!(),
        )
        .in_scope(|| {
            ::tracing::info!(
                target: "lmx.routine",
                routine_id = ROUTINE_ID,
                code.function = ::std::module_path!(),
                "enter"
            );
        });
    };
}

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

use tracing_subscriber::filter::EnvFilter;
use tracing_subscriber::reload;
use tracing_subscriber::Registry;

/// Runtime kill-switch for OTel span/event export. Read by the
/// `FilterFn` wrapping the OTel layer (see [`otel::build_layer`]). When
/// `false` the OTel layer never sees an event; when `true` it forwards
/// everything as usual. Default `false`; set to `true` at the end of
/// [`init_tracing`] iff an OTLP exporter was successfully installed.
pub static OTEL_ENABLED: AtomicBool = AtomicBool::new(false);

/// Process-global handle to the reloadable `EnvFilter` layer installed
/// in [`init_tracing`]. Populated by the **first** call to that
/// function; subsequent `init_tracing` invocations are no-ops, matching
/// the existing idempotency contract relied on by tests that spawn
/// multiple brokers.
///
/// The handle's `S` type parameter is `Registry` — that's the
/// concrete subscriber the reload layer is composed onto in both the
/// default and the `otel` cargo-feature builds. Keeping the type
/// stable across feature flags is what lets [`set_log_level`] /
/// [`current_log_level`] live outside of any `#[cfg]` block.
static RELOAD_HANDLE: OnceLock<reload::Handle<EnvFilter, Registry>> = OnceLock::new();

/// Snapshot of the directive currently in effect on the reloadable
/// `EnvFilter`. Updated by [`set_log_level`]; seeded by
/// [`init_tracing`] from `RUST_LOG` (or the `info` default).
///
/// We mirror the directive into a `String` because `EnvFilter`'s
/// `Display` impl is the canonical render of "what the filter would
/// match", but only the **input** directive string is useful for
/// round-tripping through `set_log_level`. Storing the raw input
/// keeps the `GET /admin/log-level` payload meaningful for operators
/// who want to copy-paste it back.
static CURRENT_LOG_DIRECTIVE: OnceLock<parking_lot::RwLock<String>> = OnceLock::new();

fn current_directive_slot() -> &'static parking_lot::RwLock<String> {
    CURRENT_LOG_DIRECTIVE.get_or_init(|| parking_lot::RwLock::new(String::new()))
}

/// Read-only accessor for the current `EnvFilter` directive. Returns
/// the empty string before [`init_tracing`] has run.
pub fn current_log_level() -> String {
    crate::routine_id!("ddl-routine-current-log-level-Q9");
    current_directive_slot().read().clone()
}

/// Replace the `EnvFilter` directive at runtime. Returns the new
/// directive on success, or a parser error message on failure.
///
/// Keeps [`current_log_level`] in sync so a follow-up GET reflects
/// the change. Safe to call from any thread / async runtime — the
/// reload handle internally serializes modifications.
pub fn set_log_level(directive: &str) -> Result<String, String> {
    crate::routine_id!("ddl-routine-set-log-level-Wp4");
    let new = EnvFilter::try_new(directive).map_err(|err| err.to_string())?;
    let handle = RELOAD_HANDLE
        .get()
        .ok_or_else(|| "tracing reload handle not installed (init_tracing not run)".to_string())?;
    handle.modify(|f| *f = new).map_err(|err| err.to_string())?;
    *current_directive_slot().write() = directive.to_string();
    tracing::info!(
        target: "lmx.routine",
        directive = directive,
        "log-level reload applied"
    );
    Ok(directive.to_string())
}

/// Set the runtime kill-switch. Returns the previous value so callers
/// (e.g. the HTTP admin endpoint) can include it in an audit log.
///
/// Rebuilds the `tracing` callsite-interest cache so already-seen call
/// sites re-evaluate against the new flag value on their next event.
/// Without that, `tracing`'s per-callsite cache could serve stale
/// "enabled" decisions until the cache happens to expire.
pub fn set_otel_enabled(enabled: bool) -> bool {
    crate::routine_id!("ddl-routine-set-otel-enabled-Mp7");
    let previous = OTEL_ENABLED.swap(enabled, Ordering::Relaxed);
    tracing::callsite::rebuild_interest_cache();
    tracing::info!(
        target: "lmx.routine",
        routine_id = ROUTINE_ID,
        previous,
        next = enabled,
        "otel kill-switch toggled"
    );
    previous
}

/// Read-only accessor for the current runtime kill-switch state.
pub fn is_otel_enabled() -> bool {
    OTEL_ENABLED.load(Ordering::Relaxed)
}

/// Initialize the global `tracing` subscriber.
///
/// Reads the following environment variables:
///
/// | Variable                       | Default | Effect                                                                           |
/// | ------------------------------ | ------- | -------------------------------------------------------------------------------- |
/// | `LMX_LOG_FORMAT`               | `text`  | Set to `json` for newline-delimited JSON. Always applied to the stdout layer.    |
/// | `RUST_LOG`                     | `info`  | Standard `tracing` env-filter (e.g. `lmx=debug,info`).                           |
/// | `OTEL_EXPORTER_OTLP_ENDPOINT`  | unset   | When set, an OTLP/gRPC exporter is installed alongside the stdout layer.         |
/// | `OTEL_SERVICE_NAME`            | `dd-rust-network-mutex` | Service name attribute on exported spans/logs.                       |
/// | `OTEL_RESOURCE_ATTRIBUTES`     | unset   | Honored by `opentelemetry_sdk` resource detector for free-form k=v pairs.        |
///
/// Idempotent: calling more than once is a no-op (the underlying `try_init`
/// only succeeds for the first caller). This matters for tests that call
/// `init_tracing` from multiple `#[tokio::test]` cases.
pub fn init_tracing() {
    routine_id!("ddl-routine-init-tracing-Yk4nQ8aZv");

    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    let format = std::env::var("LMX_LOG_FORMAT").unwrap_or_else(|_| "text".into());
    // Capture the user's chosen directive verbatim so we can hand it
    // back from `current_log_level()` (and so `set_log_level` has a
    // sensible starting baseline). When no directive is set we fall
    // back to "info" — the same default `EnvFilter::try_new` would
    // produce.
    let initial_directive = std::env::var("RUST_LOG").unwrap_or_else(|_| "info".to_string());
    let env_filter =
        EnvFilter::try_new(&initial_directive).unwrap_or_else(|_| EnvFilter::new("info"));

    // Build the stdout fmt layer. `with_filter` would attach a per-layer
    // filter, but the global `EnvFilter` we register below is fine and
    // keeps both layers in sync — easier to reason about for operators.
    let fmt_layer: Box<dyn tracing_subscriber::Layer<_> + Send + Sync> = if format == "json" {
        Box::new(
            tracing_subscriber::fmt::layer()
                .json()
                .with_current_span(true)
                .with_span_list(false),
        )
    } else {
        Box::new(tracing_subscriber::fmt::layer())
    };

    // Wrap the EnvFilter in a `reload::Layer` so `POST /admin/log-level`
    // can swap directives at runtime without restarting the broker.
    // The handle's S type is `Registry` (the base subscriber); both
    // the default and the `otel` branch below compose onto the same
    // Registry so the type stays stable across feature flags.
    let (reload_layer, reload_handle) = reload::Layer::new(env_filter);

    let registry = tracing_subscriber::registry()
        .with(reload_layer)
        .with(fmt_layer);

    let install_handle = || {
        // OnceLock ensures only the first init populates the slot —
        // subsequent `init_tracing` calls (typically from tests that
        // spin up multiple brokers in one process) are no-ops as far
        // as the reload-handle goes. Mirror the same idempotency for
        // the directive snapshot.
        let _ = RELOAD_HANDLE.set(reload_handle);
        let mut slot = current_directive_slot().write();
        if slot.is_empty() {
            *slot = initial_directive.clone();
        }
    };

    #[cfg(feature = "otel")]
    {
        if let Some(layer) = otel::build_layer() {
            let success = registry.with(layer).try_init().is_ok();
            // Default the runtime kill-switch to ON now that an exporter
            // is wired. Operators can flip it via `POST /admin/otel`
            // without restarting the process.
            OTEL_ENABLED.store(true, Ordering::Relaxed);
            if success {
                install_handle();
            }
            ::tracing::info!(
                target: "lmx.routine",
                routine_id = ROUTINE_ID,
                "tracing initialised with OTLP exporter (runtime kill-switch=on)"
            );
            return;
        }
    }

    let success = registry.try_init().is_ok();
    if success {
        install_handle();
    }
    ::tracing::info!(
        target: "lmx.routine",
        routine_id = ROUTINE_ID,
        "tracing initialised (stdout only; set OTEL_EXPORTER_OTLP_ENDPOINT to enable OTLP export)"
    );
}

/// Gracefully shut down any OTel exporter that was installed by
/// [`init_tracing`]. Call this from the binary's signal handler **before**
/// the tokio runtime shuts down, so in-flight spans get flushed.
///
/// No-op when the `otel` feature is disabled or no exporter was wired up.
pub fn shutdown_tracing() {
    routine_id!("ddl-routine-shutdown-tracing-Pq3");

    #[cfg(feature = "otel")]
    {
        otel::shutdown();
    }
}

#[cfg(feature = "otel")]
mod otel {
    use opentelemetry::trace::TracerProvider as _;
    use opentelemetry::KeyValue;
    use opentelemetry_otlp::WithExportConfig;
    use opentelemetry_sdk::trace::{Config, TracerProvider};
    use opentelemetry_sdk::Resource;
    use std::sync::atomic::Ordering;
    use std::sync::OnceLock;
    use tracing_subscriber::filter::FilterFn;
    use tracing_subscriber::Layer;

    static PROVIDER: OnceLock<TracerProvider> = OnceLock::new();

    /// Build the `tracing-opentelemetry` layer, or `None` when no OTLP
    /// endpoint is configured. Safe to call exactly once per process.
    ///
    /// The returned layer is wrapped in a [`FilterFn`] that consults the
    /// process-wide [`super::OTEL_ENABLED`] atomic on every event. The
    /// filter check is essentially free (one relaxed atomic load) and
    /// allows operators to disable OTel export at runtime via
    /// [`super::set_otel_enabled`] without restarting the broker.
    pub(super) fn build_layer<S>() -> Option<Box<dyn Layer<S> + Send + Sync>>
    where
        S: tracing::Subscriber + Send + Sync + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
    {
        crate::routine_id!("ddl-routine-otel-build-layer-3kP");

        let endpoint = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").ok()?;
        let service_name = std::env::var("OTEL_SERVICE_NAME")
            .unwrap_or_else(|_| "dd-rust-network-mutex".to_string());
        let service_version = env!("CARGO_PKG_VERSION");

        let resource = Resource::new([
            KeyValue::new(
                opentelemetry_semantic_conventions::resource::SERVICE_NAME,
                service_name.clone(),
            ),
            KeyValue::new(
                opentelemetry_semantic_conventions::resource::SERVICE_VERSION,
                service_version.to_string(),
            ),
        ]);

        let exporter = opentelemetry_otlp::new_exporter()
            .tonic()
            .with_endpoint(endpoint.clone())
            .build_span_exporter()
            .ok()?;

        let provider = TracerProvider::builder()
            .with_config(Config::default().with_resource(resource))
            .with_batch_exporter(exporter, opentelemetry_sdk::runtime::Tokio)
            .build();

        let tracer = provider.tracer("dd-rust-network-mutex");
        let _ = PROVIDER.set(provider);

        let kill_switch = FilterFn::new(|_metadata| super::OTEL_ENABLED.load(Ordering::Relaxed));
        Some(Box::new(
            tracing_opentelemetry::layer()
                .with_tracer(tracer)
                .with_filter(kill_switch),
        ))
    }

    pub(super) fn shutdown() {
        crate::routine_id!("ddl-routine-otel-shutdown-Vw9");

        if let Some(provider) = PROVIDER.get() {
            for result in provider.force_flush() {
                if let Err(err) = result {
                    ::tracing::warn!(
                        target: "lmx.routine",
                        routine_id = ROUTINE_ID,
                        error = %err,
                        "OTel force_flush returned an error"
                    );
                }
            }
        }
    }
}
