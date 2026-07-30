// fiducia-backend entrypoint: the axum app for fiducia.cloud's website tier.
// Serves the static Astro marketing site, the Maud/HTMX customer portal and its
// WS/SSE fragment streams, plus authenticated customer APIs. API-key lifecycle
// is delegated to fiducia-auth so there is exactly one credential authority.
mod auth;
mod billing;
mod cron;
mod entity;
mod metrics;
mod request_security;
mod store;
mod supabase_auth;
mod throttle;

use auth::{
    bearer_token, cookie_value, Authenticator, CustomerCtx, CUSTOMER_LOGIN_CSRF_COOKIE,
    CUSTOMER_SESSION_COOKIE,
};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::Request;
use axum::extract::{Form, Path, Query, State};
use axum::http::{header, HeaderMap, HeaderName, HeaderValue, Method, StatusCode, Uri};
use axum::middleware::{self, Next};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{Html, IntoResponse, Response};
use axum::{
    routing::{get, post},
    Json, Router,
};
use maud::{html, Markup, PreEscaped, DOCTYPE};
use request_security::{RequestSecurity, RequestSecurityError};
use sea_orm::{ConnectOptions, Database, DatabaseConnection};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::convert::Infallible;
use std::io;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use supabase_auth::{required_totp_factor, OtpChannel, SupabaseAuth, SupabaseAuthError};
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::cors::CorsLayer;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;
use uuid::Uuid;

const SERVICE: &str = "fiducia-backend";

/// Bound request handling time. The site is static; nothing legitimately runs long.
const REQUEST_TIMEOUT_SECS: u64 = 30;
/// Cap request bodies — this tier only serves GETs.
const MAX_BODY_BYTES: usize = 384 * 1024;
const STREAM_HEARTBEAT_SECS: u64 = 15;
const CUSTOMER_WS_PATH: &str = "/app/ws";
const CUSTOMER_EVENTS_PATH: &str = "/app/events";
const HTMX_JS: &str = include_str!("../assets/htmx.min.js");
const CUSTOMER_CSS: &str = include_str!("../assets/customer.css");
/// Carries the primary-factor (aal1) Supabase token between `/login/verify` and
/// `/login/mfa` while the user completes TOTP step-up. Short-lived and cleared
/// the instant the aal2 app-session cookie is issued. Distinct from the app
/// session cookie so a verified-TOTP user is never admitted on aal1 alone.
const CUSTOMER_MFA_PENDING_COOKIE: &str = if cfg!(debug_assertions) {
    "fiducia_customer_mfa_pending"
} else {
    "__Host-fiducia_customer_mfa_pending"
};
/// Step-up must complete promptly; the pending token self-expires.
const MFA_PENDING_MAX_AGE_SECS: u64 = 300;
const CUSTOMER_ORG_HEADER: &str = "x-fiducia-org-id";
const IDEMPOTENCY_KEY_HEADER: &str = "idempotency-key";
const CUSTOMER_CSRF_HEADER: &str = "x-fiducia-csrf";
const CORS_MAX_AGE_SECS: u64 = 10 * 60;
const MAX_API_KEY_NAME_CHARS: usize = 100;
const MAX_TIMEZONE_CHARS: usize = 64;
const MAX_SESSION_DEVICE_CHARS: usize = 200;
const DEFAULT_ACTIVITY_LIMIT: u64 = 50;
const MAX_ACTIVITY_LIMIT: u64 = 100;

const CUSTOMER_REGIONS: &[&str] = &["auto", "iad1", "sfo1", "ams1", "fra1", "sin1", "syd1"];

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Hold the guard for the whole of `main`: v0.2.1's `init` returns a
    // `#[must_use]` TelemetryGuard that shuts the OTLP exporters down on drop.
    let _telemetry = fiducia_telemetry::init(SERVICE);

    // Directory of the built Astro site. Defaults to the bundled `static/`
    // (populated from fiducia-marketing.web's `dist/` at build time), but can be
    // pointed straight at the frontend dist via STATIC_DIR for local dev.
    let static_dir: PathBuf = std::env::var("STATIC_DIR")
        .unwrap_or_else(|_| "static".to_string())
        .into();

    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8080);
    let customer_app_origin = customer_app_origin_from_env()?;
    let request_security = RequestSecurity::from_env(port)?;
    // Customer state is always durable. A missing/unreachable database is a
    // deployment error, not permission to serve invented customer data.
    let pool = connect_customer_db().await?;

    let config = AppConfig {
        static_dir: static_dir.clone(),
        customer_app_host: std::env::var("CUSTOMER_APP_HOST")
            .unwrap_or_else(|_| "app.fiducia.cloud".to_string()),
        customer_app_origin,
        customer_site_mode: std::env::var("FIDUCIA_SITE_MODE")
            .map(|v| v.eq_ignore_ascii_case("customer"))
            .unwrap_or(false),
        supabase_url: Some(required_env("SUPABASE_URL")?),
        supabase_publishable_key: Some(required_env("SUPABASE_PUBLISHABLE_KEY")?),
        auth_url: Some(required_env("FIDUCIA_AUTH_URL")?),
        pool: Some(pool),
        authenticator: Authenticator::from_env(),
        request_security,
        cron_services: CronServices::from_env(),
    };

    let app = build_router(config);

    let addr = SocketAddr::from(([0, 0, 0, 0], port));

    tracing::info!(
        "{SERVICE} listening on http://{addr} (marketing={})",
        static_dir.display()
    );
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

/// Resolve when the process is asked to stop, so in-flight work can finish.
///
/// Every k8s rollout sends SIGTERM. Without this the server is aborted the
/// instant the runtime unwinds, cutting the `/app/events` SSE streams and the
/// `/app/ws` sockets mid-write and killing any in-flight `/api/customer/*`
/// mutation. Waiting on SIGTERM (k8s) and Ctrl-C (local) lets connections drain.
async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut sig) => {
                sig.recv().await;
            }
            Err(error) => {
                tracing::error!(%error, "failed to install SIGTERM handler");
                std::future::pending::<()>().await;
            }
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }
    tracing::info!("shutdown signal received; draining in-flight requests");
}

/// Connect to the customer Postgres plane. Production startup fails closed when
/// `DATABASE_URL` is absent or unreachable.
///
/// The customer plane lives in the dedicated `fiducia` Postgres schema (declared
/// in k8s-cluster `remote/libs/pg-defs` and converged onto AWS RDS via dpm), so
/// the shared RDS instance can host many apps without table-name collisions. The
/// SeaORM entities reference bare table names, so we pin the connection's
/// `search_path` to that schema. `FIDUCIA_DB_SCHEMA` overrides it (e.g. `public`
/// for a legacy Supabase database); `pg_catalog` is always implicitly first, so
/// `gen_random_uuid()` and friends still resolve.
async fn connect_customer_db() -> Result<DatabaseConnection, Box<dyn std::error::Error>> {
    let url = required_env("DATABASE_URL")?;
    let schema = std::env::var("FIDUCIA_DB_SCHEMA")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "fiducia".to_string());
    let mut options = ConnectOptions::new(url);
    options.max_connections(5).sqlx_logging(false);
    options.set_schema_search_path(schema.clone());
    let pool = Database::connect(options).await?;
    pool.ping().await?;
    tracing::info!(%schema, "customer DB connected (search_path pinned) — customer state is durable");
    Ok(pool)
}

fn required_env(name: &str) -> Result<String, io::Error> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, format!("{name} must be set")))
}

fn customer_app_origin_from_env() -> Result<Option<HeaderValue>, io::Error> {
    match std::env::var("CUSTOMER_APP_ORIGIN") {
        Ok(value) if !value.trim().is_empty() => parse_customer_app_origin(&value).map(Some),
        Ok(_) | Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "CUSTOMER_APP_ORIGIN must be valid UTF-8",
        )),
    }
}

fn parse_customer_app_origin(value: &str) -> Result<HeaderValue, io::Error> {
    let value = value.trim();
    let uri = value.parse::<Uri>().map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "CUSTOMER_APP_ORIGIN must be an absolute http(s) origin",
        )
    })?;
    let scheme = uri
        .scheme_str()
        .filter(|scheme| matches!(*scheme, "http" | "https"));
    let authority = uri
        .authority()
        .filter(|authority| !authority.as_str().contains('@'));
    let root_only = uri
        .path_and_query()
        .map(|path| path.as_str() == "/")
        .unwrap_or(true);
    let (Some(scheme), Some(authority)) = (scheme, authority) else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "CUSTOMER_APP_ORIGIN must contain only an http(s) scheme and host",
        ));
    };
    if !root_only {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "CUSTOMER_APP_ORIGIN must not contain a path, query, or fragment",
        ));
    }
    let canonical = format!("{scheme}://{authority}");
    if value != canonical && value != format!("{canonical}/") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "CUSTOMER_APP_ORIGIN must be a single exact origin",
        ));
    }
    HeaderValue::from_str(&canonical).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "CUSTOMER_APP_ORIGIN is not a valid HTTP header origin",
        )
    })
}

fn customer_cors(origin: HeaderValue) -> CorsLayer {
    CorsLayer::new()
        .allow_origin(origin)
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .allow_headers([
            header::AUTHORIZATION,
            header::CONTENT_TYPE,
            HeaderName::from_static(CUSTOMER_ORG_HEADER),
            HeaderName::from_static(IDEMPOTENCY_KEY_HEADER),
            HeaderName::from_static(CUSTOMER_CSRF_HEADER),
        ])
        .max_age(Duration::from_secs(CORS_MAX_AGE_SECS))
}

/// Build the application router. Separated from `main` so tests can exercise the
/// routes without binding a socket or initializing telemetry.
fn build_router(config: AppConfig) -> Router {
    // Everything else is served from the static Astro build. Requests for
    // directories resolve to index.html, and unknown paths fall back to the
    // generated 404 page so client routing keeps working.
    let serve_dir = ServeDir::new(&config.static_dir)
        .append_index_html_on_directories(true)
        .fallback(ServeFile::new(config.static_dir.join("404.html")));
    let customer_app_origin = config.customer_app_origin.clone();
    let request_security = config.request_security.clone();
    let mfa_assurance_config = config.clone();
    let sensitive_header_context = SensitiveHeaderContext {
        customer_app_host: config.customer_app_host.clone(),
        customer_site_mode: config.customer_site_mode,
    };
    // Routes are declared as flat literals (not nested) so the shared API-docs
    // generator (remote/tools/generate-api-docs.mjs, which scans the router's
    // route declarations) records their true paths.
    let router = cron_routes(Router::new())
        // Liveness/readiness probe (matches the sibling canonical.cloud
        // convention); also available as /api/health.
        .route("/healthz", get(health))
        .route("/metrics", get(metrics::endpoint))
        .route("/api/health", get(health))
        .route("/api/info", get(info))
        .route("/assets/htmx.min.js", get(htmx_js))
        .route("/assets/customer.css", get(customer_css))
        .route("/login", get(customer_login).post(customer_login_submit))
        // Passwordless + MFA login flows (email OTP, phone OTP, and
        // TOTP step-up). All share the login-CSRF cookie contract.
        .route("/login/otp", post(customer_login_otp_submit))
        .route("/login/verify", post(customer_login_verify_submit))
        .route("/login/mfa", post(customer_login_mfa_submit))
        .route("/logout", axum::routing::post(customer_logout))
        .route("/api/customer/context", get(customer_context_json))
        .route(
            "/api/customer/api-keys",
            get(customer_api_keys_json).post(create_customer_api_key),
        )
        .route(
            "/api/customer/api-keys/rotate",
            axum::routing::post(rotate_customer_api_key),
        )
        .route(
            "/api/customer/api-keys/revoke",
            axum::routing::post(revoke_customer_api_key),
        )
        // Read-only authenticated catch-up for local browser hydration. Credential
        // mutations go through the explicit create/rotate endpoints above and are
        // owned by fiducia-auth; this BFF exposes no second write authority.
        .route("/api/customer/sync/:table", get(sync_catchup))
        // Inbound provider webhooks. Deliberately OUTSIDE the `/api/customer`
        // prefix so the session/CSRF/AAL gates do not apply — these are
        // unauthenticated by necessity (Stripe/PayPal POST directly) and the
        // provider SIGNATURE is the trust boundary, verified in `billing` before
        // anything is recorded. Idempotent on redelivery.
        .route("/api/billing/webhooks/:provider", post(billing::webhook))
        .route(
            "/api/customer/preferences",
            get(customer_preferences_json).put(update_customer_preferences),
        )
        .route(
            "/api/customer/security/sessions",
            get(customer_security_sessions_json),
        )
        .route(
            "/api/customer/security/sessions/revoke",
            axum::routing::post(revoke_customer_security_session),
        )
        .route("/api/customer/activity", get(customer_activity_json))
        .route("/", get(root))
        .route("/app", get(customer_home))
        .route("/app/", get(customer_home))
        .route("/app/dashboard", get(customer_home))
        .route("/app/auth", get(customer_auth))
        .route("/app/signup", get(customer_auth))
        .route(
            "/app/api-keys",
            get(customer_api_keys).post(create_customer_api_key_form),
        )
        .route(
            "/app/api-keys/rotate",
            axum::routing::post(rotate_customer_api_key_form),
        )
        .route(
            "/app/api-keys/revoke",
            axum::routing::post(revoke_customer_api_key_form),
        )
        .route("/app/security", get(customer_security))
        // Authenticator (TOTP) enrollment + lifecycle on the Security surface.
        .route("/app/security/mfa", get(customer_mfa_page))
        .route("/app/security/mfa/enroll", post(customer_mfa_enroll))
        .route("/app/security/mfa/activate", post(customer_mfa_activate))
        .route("/app/security/mfa/disable", post(customer_mfa_disable))
        .route("/app/activity", get(customer_activity))
        .route("/app/notifications", get(customer_notifications))
        .route(
            "/app/notifications/read",
            axum::routing::post(read_customer_notification_form),
        )
        .route(
            "/app/security/sessions/revoke",
            axum::routing::post(revoke_customer_session_form),
        )
        .route(
            "/app/settings",
            get(customer_settings).post(update_customer_preferences_form),
        )
        .route("/app/preferences", get(customer_settings))
        // Keep these route paths literal so the shared API-doc generator can
        // derive the complete surface; the constants remain the security and
        // response-metadata source of truth elsewhere in this module.
        .route("/app/ws", get(customer_ws))
        .route("/app/events", get(customer_events))
        .route("/app/fragments/summary", get(summary_fragment))
        .route("/app/fragments/api-keys", get(api_keys_fragment))
        .route(
            "/app/fragments/preferences",
            get(customer_preferences_fragment),
        )
        .route(
            "/app/fragments/security-sessions",
            get(customer_sessions_fragment),
        )
        .route("/app/fragments/activity", get(customer_activity_fragment))
        .route(
            "/app/fragments/notifications",
            get(customer_notifications_fragment),
        )
        // Generated API docs (AGENTS.md "API Docs Contract").
        .route("/docs/api", get(api_docs_html))
        .route("/api/docs", get(api_docs_html))
        .route("/api/docs.json", get(api_docs_json))
        // Mermaid architecture diagram (rendered client-side).
        .route("/docs/diagram", get(diagram_html))
        // Everything else: the static Astro site.
        .fallback_service(serve_dir)
        .with_state(config)
        // Security headers for the public site. CSP is intentionally just
        // `upgrade-insecure-requests` so the docs/diagram pages can still load
        // their Mermaid/marked CDN + inline init; tighten once those are vendored.
        .layer(SetResponseHeaderLayer::overriding(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            header::X_FRAME_OPTIONS,
            HeaderValue::from_static("DENY"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            header::REFERRER_POLICY,
            HeaderValue::from_static("strict-origin-when-cross-origin"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            HeaderName::from_static("permissions-policy"),
            HeaderValue::from_static("geolocation=(), microphone=(), camera=()"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static("upgrade-insecure-requests"),
        ))
        // The login-flow fix prevents a new password grant from becoming a
        // session before TOTP, but this is the token-level backstop: every
        // authenticated customer surface rejects an `aal1` token when Supabase
        // says that account has a verified factor. New routes inherit this gate.
        .layer(middleware::from_fn_with_state(
            mfa_assurance_config,
            customer_mfa_assurance_gate,
        ))
        // Keep host, origin, and CSRF checks outside the authentication/AAL
        // gate so rejected requests cannot trigger upstream auth calls.
        .layer(middleware::from_fn_with_state(
            request_security,
            request_security_gate,
        ))
        .layer(middleware::from_fn_with_state(
            sensitive_header_context,
            security_headers,
        ))
        // Hardening stack, applied LAST so it is genuinely OUTERMOST.
        //
        // Ordering is a correctness property, not style: `.layer()` wraps what
        // came before, so anything added after these runs outside them. When the
        // gates above were layered last they escaped both guards — the AAL gate
        // calls `fiducia-auth` over the network, so a hung upstream ignored
        // `REQUEST_TIMEOUT_SECS` entirely and a panic inside a gate bypassed
        // `CatchPanicLayer` and killed the connection. Keep these at the bottom.
        .layer(TraceLayer::new_for_http())
        .layer(CatchPanicLayer::new())
        .layer(TimeoutLayer::new(Duration::from_secs(REQUEST_TIMEOUT_SECS)))
        .layer(RequestBodyLimitLayer::new(MAX_BODY_BYTES))
        .layer(middleware::from_fn(metrics::record_http));

    match customer_app_origin {
        Some(origin) => router.layer(customer_cors(origin)),
        None => router,
    }
}

#[derive(Clone)]
struct AppConfig {
    static_dir: PathBuf,
    customer_app_host: String,
    /// Exact standalone customer origin allowed to call this service from a
    /// browser. `None` keeps the service same-origin-only.
    customer_app_origin: Option<HeaderValue>,
    customer_site_mode: bool,
    supabase_url: Option<String>,
    supabase_publishable_key: Option<String>,
    auth_url: Option<String>,
    /// Customer Postgres pool. `None` exists only in isolated route tests and
    /// always produces a service-unavailable response.
    pool: Option<DatabaseConnection>,
    /// Verifies the customer's Supabase session for `/api/customer/*` and scopes
    /// writes to their org. Fail-closed (`Deny`) when no auth backend is set.
    authenticator: Authenticator,
    request_security: RequestSecurity,
    /// Fail-closed trusted clients for the tenant-scoped scheduler and managed
    /// function runtime. Browser credentials are never forwarded.
    cron_services: CronServices,
}

impl AppConfig {
    /// A Supabase Auth client, when both the project URL and publishable key are
    /// configured. `None` means passwordless/MFA flows are unavailable and their
    /// handlers must fail closed with `customer_login_not_configured`.
    fn supabase_auth(&self) -> Option<SupabaseAuth> {
        match (
            self.supabase_url.as_deref(),
            self.supabase_publishable_key.as_deref(),
        ) {
            (Some(url), Some(key)) => Some(SupabaseAuth::new(url, key)),
            _ => None,
        }
    }

    /// Authenticate a customer session and enforce the MFA policy from the
    /// currently verified token plus Supabase's current factor state. A JWT can
    /// outlive factor enrollment, so `aal1` alone is insufficient: an enrolled
    /// factor makes `aal2` mandatory. The factor lookup is intentionally live
    /// and failures deny access rather than reintroducing a password-only path.
    async fn authenticate_mfa_aware(&self, headers: &HeaderMap) -> Result<CustomerCtx, Response> {
        let customer = self.authenticator.authenticate(headers).await?;
        if customer.is_aal2() {
            return Ok(customer);
        }

        let token = customer_access_token(headers)
            .ok_or_else(|| deny_json(StatusCode::UNAUTHORIZED, "missing_customer_session"))?;
        let supabase = self.supabase_auth().ok_or_else(|| {
            dependency_error(
                "supabase",
                "mfa_state_unavailable",
                "SUPABASE_URL and SUPABASE_PUBLISHABLE_KEY are required to verify aal1 sessions",
            )
        })?;
        let factors = supabase
            .list_factors(&token)
            .await
            .map_err(|error| dependency_error("supabase", "mfa_state_unavailable", error))?;
        if required_totp_factor(&factors).is_some() {
            return Err(deny_json(StatusCode::UNAUTHORIZED, "mfa_step_up_required"));
        }
        Ok(customer)
    }
}

mod security;
pub(crate) use cron::*;
pub(crate) use security::*;

mod login;
pub(crate) use login::*;

async fn health() -> Json<serde_json::Value> {
    Json(json!({ "status": "ok", "service": SERVICE }))
}

async fn info(State(config): State<AppConfig>) -> Json<serde_json::Value> {
    Json(json!({
        "service": SERVICE,
        "version": env!("CARGO_PKG_VERSION"),
        "domain": "fiducia.cloud",
        "role": "website",
        "customer_portal": {
            "host": config.customer_app_host,
            "path": "/app",
            "rendering": "maud+htmx",
            "streams": {
                "websocket": CUSTOMER_WS_PATH,
                "sse": CUSTOMER_EVENTS_PATH,
                "heartbeat_secs": STREAM_HEARTBEAT_SECS,
            },
            "regions": CUSTOMER_REGIONS,
            "supabase_login": config.supabase_url.is_some()
                && config.supabase_publishable_key.is_some(),
        },
        // The coordination API is not served here — it lives in the data-plane
        // and control-plane services.
        "components": {
            "data_plane": "fiducia-node",
            "control_plane": "fiducia-brain",
        },
    }))
}

#[derive(Debug, Deserialize)]
struct CreateCustomerApiKeyRequest {
    name: String,
    environment: String,
    scope: String,
    require_idempotency: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct CreateCustomerApiKeyForm {
    csrf_token: String,
    org_id: String,
    idempotency_key: String,
    name: String,
    environment: String,
    scope: String,
}

#[derive(Debug, Deserialize)]
struct RotateCustomerApiKeyForm {
    csrf_token: String,
    org_id: String,
    idempotency_key: String,
    prefix: String,
}

#[derive(Debug, Deserialize)]
struct RevokeCustomerApiKeyForm {
    csrf_token: String,
    org_id: String,
    idempotency_key: String,
    prefix: String,
}

#[derive(Debug, Deserialize)]
struct RotateCustomerApiKeyRequest {
    prefix: String,
}

#[derive(Debug, Deserialize)]
struct RevokeCustomerApiKeyRequest {
    prefix: String,
}

#[derive(Debug, Deserialize)]
struct AuthKeyMeta {
    key_id: String,
    org_id: String,
    name: String,
    scopes: Vec<String>,
    env: String,
    last_used_ms: Option<u64>,
    revoked: bool,
    version: u64,
    require_idempotency: bool,
}

#[derive(Debug, Deserialize)]
struct AuthKeyListResponse {
    keys: Vec<AuthKeyMeta>,
}

#[derive(Debug, Deserialize)]
struct AuthKeyCreateResponse {
    api_key: String,
    key: AuthKeyMeta,
}

#[derive(Debug, Deserialize)]
struct AuthKeyRotateResponse {
    api_key: String,
    key: AuthKeyMeta,
    overlap_seconds: u64,
}

#[derive(Debug, Deserialize)]
struct AuthKeyRevokeResponse {
    revoked: bool,
}

#[derive(Debug, Deserialize)]
struct RevokeCustomerSecuritySessionRequest {
    device: String,
}

#[derive(Debug, Deserialize)]
struct RevokeCustomerSessionForm {
    csrf_token: String,
    device: String,
}

#[derive(Debug, Deserialize)]
struct CustomerPreferencesForm {
    csrf_token: String,
    region: String,
    timezone: String,
    density: String,
    notify_lock_contention: Option<String>,
    notify_key_rotation: Option<String>,
    notify_mfa: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct CustomerOrgSelection {
    org_id: Option<String>,
}

/// Optional page size for the customer activity API. It is bounded before the
/// SeaORM query so a browser-controlled query string cannot create an unbounded
/// audit-log read.
#[derive(Debug, Default, Deserialize)]
struct CustomerActivityQuery {
    limit: Option<u16>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct CustomerPreferences {
    region: String,
    timezone: String,
    density: String,
    notify_lock_contention: bool,
    notify_key_rotation: bool,
    notify_mfa: bool,
}

/// Deliberately small customer-facing view of an audit record. In particular,
/// diagnostic metadata, source addresses, and user agents remain server-only.
#[derive(Clone, Debug, Serialize)]
struct CustomerAuditEvent {
    id: Uuid,
    actor: Option<String>,
    action: String,
    target: Option<String>,
    request_id: Option<String>,
    created_at: String,
}

#[allow(clippy::result_large_err)] // Axum handlers return the framework Response directly.
fn customer_pool(config: &AppConfig) -> Result<&DatabaseConnection, Response> {
    config.pool.as_ref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "ok": false,
                "error": "database_unavailable",
                "dependency": "postgres"
            })),
        )
            .into_response()
    })
}

fn dependency_error(dependency: &str, code: &str, error: impl std::fmt::Display) -> Response {
    tracing::error!(dependency, code, error = %error, "required dependency operation failed");
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({ "ok": false, "error": code, "dependency": dependency })),
    )
        .into_response()
}

fn no_store_json(status: StatusCode, body: serde_json::Value) -> Response {
    (
        status,
        [
            (header::CACHE_CONTROL, "no-store"),
            (header::PRAGMA, "no-cache"),
        ],
        Json(body),
    )
        .into_response()
}

fn valid_org_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && !value
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
}

#[allow(clippy::result_large_err)]
fn selected_customer_org_from(
    ctx: &CustomerCtx,
    headers: &HeaderMap,
    explicit: Option<&str>,
) -> Result<String, Response> {
    let requested = match explicit {
        Some(requested) => Some(requested),
        None => match headers.get(CUSTOMER_ORG_HEADER) {
            Some(requested) => Some(requested.to_str().map_err(|_| {
                (
                    StatusCode::BAD_REQUEST,
                    Json(json!({ "ok": false, "error": "invalid_org_selection" })),
                )
                    .into_response()
            })?),
            None => None,
        },
    };
    if let Some(requested) = requested {
        if !valid_org_id(requested) {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(json!({ "ok": false, "error": "invalid_org_selection" })),
            )
                .into_response());
        }
        if ctx.orgs.iter().any(|org_id| org_id == requested) {
            return Ok(requested.to_string());
        }
        return Err((
            StatusCode::FORBIDDEN,
            Json(json!({ "ok": false, "error": "forbidden_org" })),
        )
            .into_response());
    }

    match ctx.orgs.as_slice() {
        [] => Err((
            StatusCode::FORBIDDEN,
            Json(json!({ "ok": false, "error": "no_org_membership" })),
        )
            .into_response()),
        [org_id] => Ok(org_id.clone()),
        _ => Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "ok": false, "error": "org_selection_required" })),
        )
            .into_response()),
    }
}

#[allow(clippy::result_large_err)]
fn selected_customer_org(ctx: &CustomerCtx, headers: &HeaderMap) -> Result<String, Response> {
    selected_customer_org_from(ctx, headers, None)
}

#[allow(clippy::result_large_err)]
fn customer_page_org(
    ctx: &CustomerCtx,
    headers: &HeaderMap,
    explicit: Option<&str>,
) -> Result<String, Response> {
    if explicit.is_some() || headers.contains_key(CUSTOMER_ORG_HEADER) || ctx.orgs.len() <= 1 {
        selected_customer_org_from(ctx, headers, explicit)
    } else {
        ctx.orgs.first().cloned().ok_or_else(|| {
            (
                StatusCode::FORBIDDEN,
                Json(json!({ "ok": false, "error": "no_org_membership" })),
            )
                .into_response()
        })
    }
}

#[allow(clippy::result_large_err)]
fn require_idempotency_key(headers: &HeaderMap) -> Result<&HeaderValue, Response> {
    let value = headers.get(IDEMPOTENCY_KEY_HEADER).ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({ "ok": false, "error": "idempotency_key_required" })),
        )
            .into_response()
    })?;
    let valid = value.to_str().is_ok_and(|value| {
        !value.is_empty()
            && value.len() <= 200
            && value.bytes().all(|byte| matches!(byte, 0x21..=0x7e))
    });
    if !valid {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "ok": false, "error": "invalid_idempotency_key" })),
        )
            .into_response());
    }
    Ok(value)
}

async fn auth_json(
    config: &AppConfig,
    headers: &HeaderMap,
    method: reqwest::Method,
    path: &str,
    body: Option<serde_json::Value>,
) -> Result<(StatusCode, serde_json::Value), Response> {
    let Some(base) = config.auth_url.as_deref() else {
        return Err(dependency_error(
            "fiducia-auth",
            "customer_key_authority_not_configured",
            "FIDUCIA_AUTH_URL is unset",
        ));
    };
    let Some(token) = bearer_token(headers) else {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(json!({ "ok": false, "error": "missing_customer_session" })),
        )
            .into_response());
    };
    let mut request = reqwest::Client::new()
        .request(method, format!("{base}{path}"))
        .bearer_auth(token);
    if let Some(idempotency_key) = headers.get(IDEMPOTENCY_KEY_HEADER) {
        request = request.header(IDEMPOTENCY_KEY_HEADER, idempotency_key);
    }
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request.send().await.map_err(|error| {
        dependency_error("fiducia-auth", "customer_key_authority_unreachable", error)
    })?;
    let status =
        StatusCode::from_u16(response.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let body = response
        .json::<serde_json::Value>()
        .await
        .map_err(|error| {
            dependency_error("fiducia-auth", "customer_key_authority_bad_response", error)
        })?;
    Ok((status, body))
}

fn proxied_auth_error(status: StatusCode, body: serde_json::Value) -> Response {
    let error = body
        .get("error")
        .cloned()
        .unwrap_or_else(|| json!("credential_authority_rejected_request"));
    (status, Json(json!({ "ok": false, "error": error }))).into_response()
}

fn auth_key_to_display(
    key: &AuthKeyMeta,
    expected_org_id: &str,
) -> Result<serde_json::Value, &'static str> {
    if key.org_id != expected_org_id
        || !valid_key_id(&key.key_id)
        || !matches!(key.env.as_str(), "live" | "test")
        || key.version == 0
        || key
            .scopes
            .iter()
            .any(|scope| !allowed_api_key_scopes().contains(&scope.as_str()))
    {
        return Err("invalid key metadata");
    }
    Ok(json!({
        "id": key.key_id,
        "name": key.name,
        "prefix": format!("fdc_{}_{}", key.env, key.key_id),
        "scopes": key.scopes.join(", "),
        "last_used": if key.last_used_ms.is_some() { "recently" } else { "never" },
        "status": if key.revoked { "revoked" } else { "active" },
        "environment": key.env,
        "require_idempotency": key.require_idempotency,
        "version": key.version,
    }))
}

fn auth_key_id_from_prefix(prefix: &str) -> Option<&str> {
    let rest = prefix.strip_prefix("fdc_")?;
    let (environment, key_id) = rest.split_once('_')?;
    if matches!(environment, "live" | "test") && valid_key_id(key_id) {
        Some(key_id)
    } else {
        None
    }
}

fn valid_key_id(key_id: &str) -> bool {
    key_id.len() == 16
        && key_id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn raw_api_key_matches(raw: &str, key: &AuthKeyMeta) -> bool {
    let expected_prefix = format!("fdc_{}_{}.", key.env, key.key_id);
    raw.strip_prefix(&expected_prefix).is_some_and(|secret| {
        secret.len() == 64
            && secret
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    })
}

fn encode_query_value(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char)
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

async fn customer_api_keys_json(State(config): State<AppConfig>, headers: HeaderMap) -> Response {
    let ctx = match config.authenticator.authenticate(&headers).await {
        Ok(c) => c,
        Err(e) => return e,
    };
    let org_id = match selected_customer_org(&ctx, &headers) {
        Ok(org_id) => org_id,
        Err(response) => return response,
    };
    let path = format!("/v1/keys?org_id={}", encode_query_value(&org_id));
    let (status, body) = match auth_json(&config, &headers, reqwest::Method::GET, &path, None).await
    {
        Ok(result) => result,
        Err(response) => return response,
    };
    if !status.is_success() {
        return proxied_auth_error(status, body);
    }
    let response: AuthKeyListResponse = match serde_json::from_value(body) {
        Ok(response) => response,
        Err(error) => return dependency_error("fiducia-auth", "auth_key_list_bad_response", error),
    };
    let keys = match response
        .keys
        .iter()
        .map(|key| auth_key_to_display(key, &org_id))
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(keys) => keys,
        Err(error) => return dependency_error("fiducia-auth", "auth_key_list_bad_response", error),
    };

    no_store_json(
        StatusCode::OK,
        json!({
            "api_keys": keys,
            "default_require_idempotency": true,
            "allowed_environments": ["live", "test"],
            "allowed_scopes": allowed_api_key_scopes(),
        }),
    )
}

async fn customer_context_json(State(config): State<AppConfig>, headers: HeaderMap) -> Response {
    let ctx = match config.authenticator.authenticate(&headers).await {
        Ok(ctx) => ctx,
        Err(response) => return response,
    };
    let csrf_token = ctx
        .is_browser_session()
        .then(|| customer_csrf_token(&config, &ctx));
    no_store_json(
        StatusCode::OK,
        json!({
            "csrf_token": csrf_token,
            "user": {
                "user_id": ctx.user_id,
                "email": ctx.email,
                "orgs": ctx.orgs,
            }
        }),
    )
}

async fn create_customer_api_key(
    State(config): State<AppConfig>,
    headers: HeaderMap,
    Json(payload): Json<CreateCustomerApiKeyRequest>,
) -> Response {
    let ctx = match config.authenticator.authenticate(&headers).await {
        Ok(c) => c,
        Err(e) => return e,
    };
    if let Err(error) = require_api_write_security(&headers, &config, &ctx) {
        return request_security_error(error);
    }
    let (display, secret) = match issue_customer_api_key(&config, &headers, &ctx, &payload).await {
        Ok(issued) => issued,
        Err(response) => return response,
    };
    no_store_json(
        StatusCode::CREATED,
        json!({
            "ok": true,
            "api_key": display,
            "secret": secret,
            "secret_once": true,
        }),
    )
}

async fn issue_customer_api_key(
    config: &AppConfig,
    headers: &HeaderMap,
    ctx: &CustomerCtx,
    payload: &CreateCustomerApiKeyRequest,
) -> Result<(serde_json::Value, String), Response> {
    require_idempotency_key(headers)?;
    if let Some(error) = validate_api_key_request(payload) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": error, "ok": false })),
        )
            .into_response());
    }
    let org_id = selected_customer_org(ctx, headers)?;
    let request = json!({
        "name": payload.name.trim(),
        "org_id": &org_id,
        "scopes": [&payload.scope],
        "env": &payload.environment,
        "require_idempotency": payload.require_idempotency.unwrap_or(true),
    });
    let (status, body) = auth_json(
        config,
        headers,
        reqwest::Method::POST,
        "/v1/keys",
        Some(request),
    )
    .await?;
    if !status.is_success() {
        return Err(proxied_auth_error(status, body));
    }
    let response: AuthKeyCreateResponse = serde_json::from_value(body)
        .map_err(|error| dependency_error("fiducia-auth", "auth_key_create_bad_response", error))?;
    if !raw_api_key_matches(&response.api_key, &response.key) {
        return Err(dependency_error(
            "fiducia-auth",
            "auth_key_create_bad_response",
            "raw key does not match metadata",
        ));
    }
    let display = auth_key_to_display(&response.key, &org_id)
        .map_err(|error| dependency_error("fiducia-auth", "auth_key_create_bad_response", error))?;
    Ok((display, response.api_key))
}

async fn rotate_customer_api_key(
    State(config): State<AppConfig>,
    headers: HeaderMap,
    Json(payload): Json<RotateCustomerApiKeyRequest>,
) -> Response {
    let ctx = match config.authenticator.authenticate(&headers).await {
        Ok(customer) => customer,
        Err(response) => return response,
    };
    if let Err(error) = require_api_write_security(&headers, &config, &ctx) {
        return request_security_error(error);
    }
    let prefix = payload.prefix.trim();
    let (display, replacement_secret, overlap_seconds) =
        match rotate_customer_api_key_authority(&config, &headers, &ctx, prefix).await {
            Ok(rotated) => rotated,
            Err(response) => return response,
        };

    no_store_json(
        StatusCode::OK,
        json!({
            "ok": true,
            "prefix": prefix,
            "rotated_at_ms": unix_epoch_ms(),
            "replacement_secret": replacement_secret,
            "api_key": display,
            "overlap_seconds": overlap_seconds,
        }),
    )
}

async fn rotate_customer_api_key_authority(
    config: &AppConfig,
    headers: &HeaderMap,
    ctx: &CustomerCtx,
    prefix: &str,
) -> Result<(serde_json::Value, String, u64), Response> {
    require_idempotency_key(headers)?;
    let Some(key_id) = auth_key_id_from_prefix(prefix) else {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "invalid_key_prefix", "ok": false })),
        )
            .into_response());
    };

    let org_id = match selected_customer_org(ctx, headers) {
        Ok(org_id) => org_id,
        Err(response) => return Err(response),
    };
    let path = format!(
        "/v1/keys/{}/rotate?org_id={}",
        encode_query_value(key_id),
        encode_query_value(&org_id)
    );
    let (status, body) = match auth_json(
        config,
        headers,
        reqwest::Method::POST,
        &path,
        Some(json!({})),
    )
    .await
    {
        Ok(result) => result,
        Err(response) => return Err(response),
    };
    if !status.is_success() {
        return Err(proxied_auth_error(status, body));
    }
    let response: AuthKeyRotateResponse = match serde_json::from_value(body) {
        Ok(response) => response,
        Err(error) => {
            return Err(dependency_error(
                "fiducia-auth",
                "auth_key_rotate_bad_response",
                error,
            ))
        }
    };
    if !raw_api_key_matches(&response.api_key, &response.key) {
        return Err(dependency_error(
            "fiducia-auth",
            "auth_key_rotate_bad_response",
            "raw key does not match metadata",
        ));
    }
    let display = match auth_key_to_display(&response.key, &org_id) {
        Ok(display) => display,
        Err(error) => {
            return Err(dependency_error(
                "fiducia-auth",
                "auth_key_rotate_bad_response",
                error,
            ))
        }
    };
    Ok((display, response.api_key, response.overlap_seconds))
}

async fn revoke_customer_api_key(
    State(config): State<AppConfig>,
    headers: HeaderMap,
    Json(payload): Json<RevokeCustomerApiKeyRequest>,
) -> Response {
    let ctx = match config.authenticator.authenticate(&headers).await {
        Ok(ctx) => ctx,
        Err(response) => return response,
    };
    if let Err(error) = require_api_write_security(&headers, &config, &ctx) {
        return request_security_error(error);
    }
    let prefix = payload.prefix.trim();
    if let Err(response) = revoke_customer_api_key_authority(&config, &headers, &ctx, prefix).await
    {
        return response;
    }
    no_store_json(
        StatusCode::OK,
        json!({ "ok": true, "prefix": prefix, "status": "revoked" }),
    )
}

async fn revoke_customer_api_key_authority(
    config: &AppConfig,
    headers: &HeaderMap,
    ctx: &CustomerCtx,
    prefix: &str,
) -> Result<(), Response> {
    require_idempotency_key(headers)?;
    let Some(key_id) = auth_key_id_from_prefix(prefix) else {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "invalid_key_prefix", "ok": false })),
        )
            .into_response());
    };
    let org_id = match selected_customer_org(ctx, headers) {
        Ok(org_id) => org_id,
        Err(response) => return Err(response),
    };
    let path = format!(
        "/v1/keys/{}?org_id={}",
        encode_query_value(key_id),
        encode_query_value(&org_id)
    );
    let (status, body) =
        match auth_json(config, headers, reqwest::Method::DELETE, &path, None).await {
            Ok(result) => result,
            Err(response) => return Err(response),
        };
    if !status.is_success() {
        return Err(proxied_auth_error(status, body));
    }
    let response: AuthKeyRevokeResponse = match serde_json::from_value(body) {
        Ok(response) => response,
        Err(error) => {
            return Err(dependency_error(
                "fiducia-auth",
                "auth_key_revoke_bad_response",
                error,
            ))
        }
    };
    if !response.revoked {
        return Err((
            StatusCode::NOT_FOUND,
            Json(json!({ "ok": false, "error": "key_not_found" })),
        )
            .into_response());
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
struct CatchupParams {
    /// Accepted for client compatibility and observability. API-key hydration is
    /// a full authoritative snapshot because fiducia-auth owns the key store.
    #[serde(default)]
    since: i64,
}

/// Catch-up hydration returns a complete, sanitized, org-scoped API-key snapshot
/// from fiducia-auth. The browser uses `hydrate(..., { prune: true })`, so rows
/// removed or revoked while it was offline reconcile without raw database CDC.
async fn sync_catchup(
    State(config): State<AppConfig>,
    Path(table): Path<String>,
    headers: HeaderMap,
    Query(params): Query<CatchupParams>,
) -> Response {
    let ctx = match config.authenticator.authenticate(&headers).await {
        Ok(c) => c,
        Err(e) => return e,
    };
    let rows: Vec<serde_json::Value> = match table.as_str() {
        "api_keys" => {
            let org_id = match selected_customer_org(&ctx, &headers) {
                Ok(org_id) => org_id,
                Err(response) => return response,
            };
            let path = format!("/v1/keys?org_id={}", encode_query_value(&org_id));
            let (status, body) =
                match auth_json(&config, &headers, reqwest::Method::GET, &path, None).await {
                    Ok(result) => result,
                    Err(response) => return response,
                };
            if !status.is_success() {
                return proxied_auth_error(status, body);
            }
            let response: AuthKeyListResponse = match serde_json::from_value(body) {
                Ok(response) => response,
                Err(error) => {
                    return dependency_error("fiducia-auth", "auth_key_list_bad_response", error)
                }
            };
            match response
                .keys
                .iter()
                .map(|key| auth_key_to_display(key, &org_id))
                .collect::<Result<Vec<_>, _>>()
            {
                Ok(keys) => keys,
                Err(error) => {
                    return dependency_error("fiducia-auth", "auth_key_list_bad_response", error)
                }
            }
        }
        _ => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "ok": false, "error": "unsupported_sync_table", "table": table })),
            )
                .into_response()
        }
    };
    no_store_json(
        StatusCode::OK,
        json!({
            "table": table,
            "snapshot": true,
            "requested_since": params.since,
            "rows": rows,
        }),
    )
}

/// Resolve the caller's local `users.id`, provisioning the row on first access.
/// Identity fields come from the verified Supabase session and are never
/// synthesized when the upstream identity is incomplete.
async fn caller_user_id(config: &AppConfig, ctx: &CustomerCtx) -> Result<Uuid, Response> {
    let pool = customer_pool(config)?;
    let sub = Uuid::parse_str(&ctx.user_id).map_err(|_| {
        (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({ "ok": false, "error": "invalid_user_subject" })),
        )
            .into_response()
    })?;
    let email = ctx
        .email
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(json!({ "ok": false, "error": "user_email_required" })),
            )
                .into_response()
        })?;
    match store::ensure_user(pool, sub, email).await {
        Ok(id) => Ok(id),
        Err(err) => Err(dependency_error("postgres", "ensure_user_failed", err)),
    }
}

fn prefs_from_row(
    row: &fiducia_interfaces_db::customer::CustomerPreferencesRow,
) -> CustomerPreferences {
    CustomerPreferences {
        region: row.region.clone(),
        timezone: row.timezone.clone(),
        density: row.density.clone(),
        notify_lock_contention: row.notify_lock_contention,
        notify_key_rotation: row.notify_key_rotation,
        notify_mfa: row.notify_mfa,
    }
}

fn session_model_json(
    row: &fiducia_interfaces_db::customer::CustomerSessionsRow,
) -> serde_json::Value {
    json!({
        "device": row.device,
        "location": row.location,
        "last_seen": row.last_seen.to_rfc3339(),
        "status": row.status,
    })
}

async fn customer_preferences_json(
    State(config): State<AppConfig>,
    headers: HeaderMap,
) -> Response {
    let ctx = match config.authenticator.authenticate(&headers).await {
        Ok(c) => c,
        Err(e) => return e,
    };
    let uid = match caller_user_id(&config, &ctx).await {
        Ok(uid) => uid,
        Err(response) => return response,
    };
    let pool = match customer_pool(&config) {
        Ok(pool) => pool,
        Err(response) => return response,
    };
    let prefs = match store::get_preferences(pool, uid).await {
        Ok(Some(row)) => prefs_from_row(&row),
        Ok(None) => default_customer_preferences(),
        Err(err) => return dependency_error("postgres", "preferences_read_failed", err),
    };
    Json(prefs).into_response()
}

async fn update_customer_preferences(
    State(config): State<AppConfig>,
    headers: HeaderMap,
    Json(payload): Json<CustomerPreferences>,
) -> Response {
    let ctx = match config.authenticator.authenticate(&headers).await {
        Ok(c) => c,
        Err(e) => return e,
    };
    if let Err(error) = require_api_write_security(&headers, &config, &ctx) {
        return request_security_error(error);
    }
    if !CUSTOMER_REGIONS.contains(&payload.region.as_str()) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "invalid_region", "ok": false })),
        )
            .into_response();
    }
    if !["comfortable", "compact"].contains(&payload.density.as_str()) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "invalid_density", "ok": false })),
        )
            .into_response();
    }
    let timezone = payload.timezone.trim();
    if timezone.is_empty() || timezone.chars().count() > MAX_TIMEZONE_CHARS {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "invalid_timezone", "ok": false })),
        )
            .into_response();
    }

    let uid = match caller_user_id(&config, &ctx).await {
        Ok(uid) => uid,
        Err(response) => return response,
    };
    let pool = match customer_pool(&config) {
        Ok(pool) => pool,
        Err(response) => return response,
    };
    match store::upsert_preferences(
        pool,
        uid,
        payload.region,
        timezone.to_string(),
        payload.density,
        payload.notify_key_rotation,
        payload.notify_lock_contention,
        payload.notify_mfa,
    )
    .await
    {
        Ok(row) => (
            StatusCode::OK,
            Json(json!({
                "ok": true,
                "preferences": prefs_from_row(&row),
                "saved_at_ms": unix_epoch_ms(),
            })),
        )
            .into_response(),
        Err(err) => dependency_error("postgres", "preferences_write_failed", err),
    }
}

async fn customer_security_sessions_json(
    State(config): State<AppConfig>,
    headers: HeaderMap,
) -> Response {
    let ctx = match config.authenticator.authenticate(&headers).await {
        Ok(c) => c,
        Err(e) => return e,
    };
    let uid = match caller_user_id(&config, &ctx).await {
        Ok(uid) => uid,
        Err(response) => return response,
    };
    let pool = match customer_pool(&config) {
        Ok(pool) => pool,
        Err(response) => return response,
    };
    let sessions_json = match store::list_sessions(pool, uid).await {
        Ok(rows) => rows.iter().map(session_model_json).collect::<Vec<_>>(),
        Err(err) => return dependency_error("postgres", "sessions_list_failed", err),
    };
    Json(json!({ "sessions": sessions_json, "revoke_supported": true })).into_response()
}

fn customer_activity_limit(requested: Option<u16>) -> u64 {
    requested
        .map(u64::from)
        .unwrap_or(DEFAULT_ACTIVITY_LIMIT)
        .clamp(1, MAX_ACTIVITY_LIMIT)
}

fn customer_audit_event(row: crate::entity::audit_log::Model) -> CustomerAuditEvent {
    CustomerAuditEvent {
        id: row.id,
        actor: row.actor,
        action: row.action,
        target: row.target,
        request_id: row.request_id,
        created_at: row.created_at.to_rfc3339(),
    }
}

/// Load activity only after the authenticated Supabase identity has selected an
/// organization it is actually a member of. The canonical schema's indexed
/// `org_id` predicate is the second tenant boundary below the auth claim.
async fn customer_activity_events(
    config: &AppConfig,
    headers: &HeaderMap,
    customer: &CustomerCtx,
    explicit_org: Option<&str>,
    limit: u64,
) -> Result<Vec<CustomerAuditEvent>, Response> {
    let org_id = selected_customer_org_from(customer, headers, explicit_org)?;
    let org_id = Uuid::parse_str(&org_id).map_err(|_| {
        dependency_error(
            "fiducia-auth",
            "invalid_verified_org_id",
            "verified organization membership was not a UUID",
        )
    })?;
    let pool = customer_pool(config)?;
    let rows = store::list_audit_events(pool, org_id, limit)
        .await
        .map_err(|error| dependency_error("postgres", "activity_list_failed", error))?;
    Ok(rows.into_iter().map(customer_audit_event).collect())
}

async fn customer_activity_json(
    State(config): State<AppConfig>,
    headers: HeaderMap,
    Query(query): Query<CustomerActivityQuery>,
) -> Response {
    let customer = match config.authenticator.authenticate(&headers).await {
        Ok(customer) => customer,
        Err(response) => return response,
    };
    match customer_activity_events(
        &config,
        &headers,
        &customer,
        None,
        customer_activity_limit(query.limit),
    )
    .await
    {
        Ok(events) => no_store_json(StatusCode::OK, json!({ "events": events })),
        Err(response) => response,
    }
}

async fn revoke_customer_security_session(
    State(config): State<AppConfig>,
    headers: HeaderMap,
    Json(payload): Json<RevokeCustomerSecuritySessionRequest>,
) -> Response {
    let ctx = match config.authenticator.authenticate(&headers).await {
        Ok(ctx) => ctx,
        Err(response) => return response,
    };
    if let Err(error) = require_api_write_security(&headers, &config, &ctx) {
        return request_security_error(error);
    }
    let device = payload.device.trim();
    if device.is_empty() || device.chars().count() > MAX_SESSION_DEVICE_CHARS {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "invalid_device", "ok": false })),
        )
            .into_response();
    }

    let uid = match caller_user_id(&config, &ctx).await {
        Ok(uid) => uid,
        Err(response) => return response,
    };
    let pool = match customer_pool(&config) {
        Ok(pool) => pool,
        Err(response) => return response,
    };
    match store::revoke_session(pool, uid, device).await {
        Ok(revoked) => {
            Json(json!({ "ok": true, "device": device, "revoked": revoked })).into_response()
        }
        Err(err) => dependency_error("postgres", "session_revoke_failed", err),
    }
}

async fn customer_preferences_fragment(
    State(config): State<AppConfig>,
    headers: HeaderMap,
) -> Response {
    let customer = match config.authenticator.authenticate(&headers).await {
        Ok(customer) => customer,
        Err(response) => return response,
    };
    preferences_fragment_markup(&config, &customer, false).await
}

async fn update_customer_preferences_form(
    State(config): State<AppConfig>,
    headers: HeaderMap,
    Form(form): Form<CustomerPreferencesForm>,
) -> Response {
    let customer = match config.authenticator.authenticate(&headers).await {
        Ok(customer) => customer,
        Err(response) => return response,
    };
    if let Err(error) = require_form_security(&headers, &config, &customer, &form.csrf_token) {
        return request_security_error(error);
    }
    if !CUSTOMER_REGIONS.contains(&form.region.as_str()) {
        return (StatusCode::BAD_REQUEST, "invalid_region").into_response();
    }
    if !["comfortable", "compact"].contains(&form.density.as_str()) {
        return (StatusCode::BAD_REQUEST, "invalid_density").into_response();
    }
    let timezone = form.timezone.trim();
    if timezone.is_empty() || timezone.chars().count() > MAX_TIMEZONE_CHARS {
        return (StatusCode::BAD_REQUEST, "invalid_timezone").into_response();
    }
    let user_id = match caller_user_id(&config, &customer).await {
        Ok(user_id) => user_id,
        Err(response) => return response,
    };
    let pool = match customer_pool(&config) {
        Ok(pool) => pool,
        Err(response) => return response,
    };
    let row = match store::upsert_preferences(
        pool,
        user_id,
        form.region,
        timezone.to_string(),
        form.density,
        form.notify_key_rotation.is_some(),
        form.notify_lock_contention.is_some(),
        form.notify_mfa.is_some(),
    )
    .await
    {
        Ok(row) => row,
        Err(error) => return dependency_error("postgres", "preferences_write_failed", error),
    };
    preferences_form_markup(
        &prefs_from_row(&row),
        true,
        &customer_csrf_token(&config, &customer),
    )
    .into_response()
}

async fn preferences_fragment_markup(
    config: &AppConfig,
    customer: &CustomerCtx,
    saved: bool,
) -> Response {
    let user_id = match caller_user_id(config, customer).await {
        Ok(user_id) => user_id,
        Err(response) => return response,
    };
    let pool = match customer_pool(config) {
        Ok(pool) => pool,
        Err(response) => return response,
    };
    let preferences = match store::get_preferences(pool, user_id).await {
        Ok(Some(row)) => prefs_from_row(&row),
        Ok(None) => default_customer_preferences(),
        Err(error) => return dependency_error("postgres", "preferences_read_failed", error),
    };
    preferences_form_markup(&preferences, saved, &customer_csrf_token(config, customer))
        .into_response()
}

async fn customer_sessions_fragment(
    State(config): State<AppConfig>,
    headers: HeaderMap,
) -> Response {
    let customer = match config.authenticator.authenticate(&headers).await {
        Ok(customer) => customer,
        Err(response) => return response,
    };
    sessions_fragment_markup(&config, &customer, None).await
}

async fn customer_activity_fragment(
    State(config): State<AppConfig>,
    headers: HeaderMap,
    Query(selection): Query<CustomerOrgSelection>,
) -> Response {
    let customer = match config.authenticator.authenticate(&headers).await {
        Ok(customer) => customer,
        Err(response) => return response,
    };
    match customer_activity_events(
        &config,
        &headers,
        &customer,
        selection.org_id.as_deref(),
        DEFAULT_ACTIVITY_LIMIT,
    )
    .await
    {
        Ok(events) => customer_activity_table_markup(&events).into_response(),
        Err(response) => response,
    }
}

async fn revoke_customer_session_form(
    State(config): State<AppConfig>,
    headers: HeaderMap,
    Form(form): Form<RevokeCustomerSessionForm>,
) -> Response {
    let customer = match config.authenticator.authenticate(&headers).await {
        Ok(customer) => customer,
        Err(response) => return response,
    };
    if let Err(error) = require_form_security(&headers, &config, &customer, &form.csrf_token) {
        return request_security_error(error);
    }
    let device = form.device.trim();
    if device.is_empty() || device.chars().count() > MAX_SESSION_DEVICE_CHARS {
        return (StatusCode::BAD_REQUEST, "invalid_device").into_response();
    }
    let user_id = match caller_user_id(&config, &customer).await {
        Ok(user_id) => user_id,
        Err(response) => return response,
    };
    let pool = match customer_pool(&config) {
        Ok(pool) => pool,
        Err(response) => return response,
    };
    let message = match store::revoke_session(pool, user_id, device).await {
        Ok(true) => Some("Session revoked."),
        Ok(false) => Some("Session was already revoked or no longer exists."),
        Err(error) => return dependency_error("postgres", "session_revoke_failed", error),
    };
    sessions_fragment_markup(&config, &customer, message).await
}

async fn sessions_fragment_markup(
    config: &AppConfig,
    customer: &CustomerCtx,
    message: Option<&str>,
) -> Response {
    let user_id = match caller_user_id(config, customer).await {
        Ok(user_id) => user_id,
        Err(response) => return response,
    };
    let pool = match customer_pool(config) {
        Ok(pool) => pool,
        Err(response) => return response,
    };
    let sessions = match store::list_sessions(pool, user_id).await {
        Ok(sessions) => sessions,
        Err(error) => return dependency_error("postgres", "sessions_list_failed", error),
    };
    sessions_table_markup(&sessions, message, &customer_csrf_token(config, customer))
        .into_response()
}

fn validate_api_key_request(payload: &CreateCustomerApiKeyRequest) -> Option<&'static str> {
    if payload.name.trim().is_empty() {
        return Some("name_required");
    }
    if payload.name.trim().chars().count() > MAX_API_KEY_NAME_CHARS {
        return Some("name_too_long");
    }
    if !["live", "test"].contains(&payload.environment.as_str()) {
        return Some("invalid_environment");
    }
    if !allowed_api_key_scopes().contains(&payload.scope.as_str()) {
        return Some("invalid_scope");
    }

    None
}

fn allowed_api_key_scopes() -> &'static [&'static str] {
    &[
        "requests:read",
        "requests:write",
        "locks:read",
        "locks:write",
        "kv:read",
        "kv:write",
        "services:read",
        "services:write",
        "elections:read",
        "elections:write",
        "cron:read",
        "cron:write",
        "rate-limit:read",
        "rate-limit:write",
    ]
}

fn default_customer_preferences() -> CustomerPreferences {
    CustomerPreferences {
        region: "auto".to_string(),
        timezone: "browser".to_string(),
        density: "comfortable".to_string(),
        notify_lock_contention: true,
        notify_key_rotation: true,
        notify_mfa: true,
    }
}

async fn root(
    State(config): State<AppConfig>,
    headers: HeaderMap,
    Query(selection): Query<CustomerOrgSelection>,
) -> Response {
    if should_serve_customer_app(&config, &headers) {
        return customer_page_response(
            &config,
            &headers,
            CustomerTab::Dashboard,
            selection.org_id.as_deref(),
        )
        .await;
    }

    match tokio::fs::read_to_string(config.static_dir.join("index.html")).await {
        Ok(body) => Html(body).into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "static index not found").into_response(),
    }
}

async fn customer_home(
    State(config): State<AppConfig>,
    headers: HeaderMap,
    Query(selection): Query<CustomerOrgSelection>,
) -> Response {
    customer_page_response(
        &config,
        &headers,
        CustomerTab::Dashboard,
        selection.org_id.as_deref(),
    )
    .await
}

async fn customer_auth(
    State(config): State<AppConfig>,
    headers: HeaderMap,
    Query(selection): Query<CustomerOrgSelection>,
) -> Response {
    customer_page_response(
        &config,
        &headers,
        CustomerTab::Auth,
        selection.org_id.as_deref(),
    )
    .await
}

async fn customer_api_keys(
    State(config): State<AppConfig>,
    headers: HeaderMap,
    Query(selection): Query<CustomerOrgSelection>,
) -> Response {
    customer_page_response(
        &config,
        &headers,
        CustomerTab::ApiKeys,
        selection.org_id.as_deref(),
    )
    .await
}

async fn customer_security(
    State(config): State<AppConfig>,
    headers: HeaderMap,
    Query(selection): Query<CustomerOrgSelection>,
) -> Response {
    customer_page_response(
        &config,
        &headers,
        CustomerTab::Security,
        selection.org_id.as_deref(),
    )
    .await
}

async fn customer_activity(
    State(config): State<AppConfig>,
    headers: HeaderMap,
    Query(selection): Query<CustomerOrgSelection>,
) -> Response {
    customer_page_response(
        &config,
        &headers,
        CustomerTab::Activity,
        selection.org_id.as_deref(),
    )
    .await
}

async fn customer_notifications(
    State(config): State<AppConfig>,
    headers: HeaderMap,
    Query(selection): Query<CustomerOrgSelection>,
) -> Response {
    customer_page_response(
        &config,
        &headers,
        CustomerTab::Notifications,
        selection.org_id.as_deref(),
    )
    .await
}

async fn customer_settings(
    State(config): State<AppConfig>,
    headers: HeaderMap,
    Query(selection): Query<CustomerOrgSelection>,
) -> Response {
    customer_page_response(
        &config,
        &headers,
        CustomerTab::Settings,
        selection.org_id.as_deref(),
    )
    .await
}

/// Fragment: the signed-in user's notification feed. Reads are scoped to the
/// verified caller's `user_id` at the database, so a forged `org_id` can never
/// surface another user's notifications.
async fn customer_notifications_fragment(
    State(config): State<AppConfig>,
    headers: HeaderMap,
) -> Response {
    let customer = match config.authenticator.authenticate(&headers).await {
        Ok(customer) => customer,
        Err(response) => return response,
    };
    notifications_fragment_markup(&config, &customer, None).await
}

#[derive(Debug, Deserialize)]
struct ReadNotificationForm {
    csrf_token: String,
    id: String,
}

/// Mark one notification read. CSRF-protected like every other browser
/// mutation, and scoped to the caller's `user_id` in the store, so a user can
/// only ever clear their own notifications. Returns the refreshed fragment.
async fn read_customer_notification_form(
    State(config): State<AppConfig>,
    headers: HeaderMap,
    Form(form): Form<ReadNotificationForm>,
) -> Response {
    let customer = match config.authenticator.authenticate(&headers).await {
        Ok(customer) => customer,
        Err(response) => return response,
    };
    if let Err(error) = require_form_security(&headers, &config, &customer, &form.csrf_token) {
        return request_security_error(error);
    }
    let Ok(id) = Uuid::parse_str(form.id.trim()) else {
        return (StatusCode::BAD_REQUEST, "invalid_notification_id").into_response();
    };
    let user_id = match caller_user_id(&config, &customer).await {
        Ok(user_id) => user_id,
        Err(response) => return response,
    };
    let pool = match customer_pool(&config) {
        Ok(pool) => pool,
        Err(response) => return response,
    };
    let message = match store::mark_notification_read(pool, user_id, id).await {
        Ok(true) => Some("Notification marked read."),
        Ok(false) => Some("Notification was already read or no longer exists."),
        Err(error) => return dependency_error("postgres", "notification_read_failed", error),
    };
    notifications_fragment_markup(&config, &customer, message).await
}

async fn notifications_fragment_markup(
    config: &AppConfig,
    customer: &CustomerCtx,
    message: Option<&str>,
) -> Response {
    let user_id = match caller_user_id(config, customer).await {
        Ok(user_id) => user_id,
        Err(response) => return response,
    };
    let pool = match customer_pool(config) {
        Ok(pool) => pool,
        Err(response) => return response,
    };
    let notifications = match store::list_notifications(pool, user_id, DEFAULT_ACTIVITY_LIMIT).await
    {
        Ok(rows) => rows,
        Err(error) => return dependency_error("postgres", "notifications_list_failed", error),
    };
    // True unread total (not just within the shown page) for an accurate badge.
    let unread = match store::unread_notification_count(pool, user_id).await {
        Ok(count) => count,
        Err(error) => return dependency_error("postgres", "notifications_count_failed", error),
    };
    notifications_table_markup(
        &notifications,
        unread,
        message,
        &customer_csrf_token(config, customer),
    )
    .into_response()
}

async fn create_customer_api_key_form(
    State(config): State<AppConfig>,
    headers: HeaderMap,
    Form(form): Form<CreateCustomerApiKeyForm>,
) -> Response {
    let customer = match config.authenticator.authenticate(&headers).await {
        Ok(customer) => customer,
        Err(response) => return response,
    };
    if let Err(error) = require_form_security(&headers, &config, &customer, &form.csrf_token) {
        return request_security_error(error);
    }
    let headers =
        match form_mutation_headers(headers, &customer, &form.org_id, &form.idempotency_key) {
            Ok(headers) => headers,
            Err(response) => return response,
        };
    let payload = CreateCustomerApiKeyRequest {
        name: form.name,
        environment: form.environment,
        scope: form.scope,
        require_idempotency: Some(true),
    };
    let (_display, secret) =
        match issue_customer_api_key(&config, &headers, &customer, &payload).await {
            Ok(issued) => issued,
            Err(response) => return response,
        };
    api_keys_fragment_markup(&config, &headers, &customer, Some(&secret)).await
}

#[allow(clippy::result_large_err)]
fn form_mutation_headers(
    mut headers: HeaderMap,
    customer: &CustomerCtx,
    explicit_org: &str,
    idempotency_key: &str,
) -> Result<HeaderMap, Response> {
    let org_id = selected_customer_org_from(customer, &headers, Some(explicit_org))?;
    let org_header = HeaderValue::from_str(&org_id).map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({ "ok": false, "error": "invalid_org_selection" })),
        )
            .into_response()
    })?;
    let idempotency_header = HeaderValue::from_str(idempotency_key).map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({ "ok": false, "error": "invalid_idempotency_key" })),
        )
            .into_response()
    })?;
    headers.insert(HeaderName::from_static(CUSTOMER_ORG_HEADER), org_header);
    headers.insert(
        HeaderName::from_static(IDEMPOTENCY_KEY_HEADER),
        idempotency_header,
    );
    require_idempotency_key(&headers)?;
    Ok(headers)
}

async fn rotate_customer_api_key_form(
    State(config): State<AppConfig>,
    headers: HeaderMap,
    Form(form): Form<RotateCustomerApiKeyForm>,
) -> Response {
    let customer = match config.authenticator.authenticate(&headers).await {
        Ok(customer) => customer,
        Err(response) => return response,
    };
    if let Err(error) = require_form_security(&headers, &config, &customer, &form.csrf_token) {
        return request_security_error(error);
    }
    let headers =
        match form_mutation_headers(headers, &customer, &form.org_id, &form.idempotency_key) {
            Ok(headers) => headers,
            Err(response) => return response,
        };
    let (_, replacement_secret, _) =
        match rotate_customer_api_key_authority(&config, &headers, &customer, form.prefix.trim())
            .await
        {
            Ok(rotated) => rotated,
            Err(response) => return response,
        };
    api_keys_fragment_markup(&config, &headers, &customer, Some(&replacement_secret)).await
}

async fn revoke_customer_api_key_form(
    State(config): State<AppConfig>,
    headers: HeaderMap,
    Form(form): Form<RevokeCustomerApiKeyForm>,
) -> Response {
    let customer = match config.authenticator.authenticate(&headers).await {
        Ok(customer) => customer,
        Err(response) => return response,
    };
    if let Err(error) = require_form_security(&headers, &config, &customer, &form.csrf_token) {
        return request_security_error(error);
    }
    let headers =
        match form_mutation_headers(headers, &customer, &form.org_id, &form.idempotency_key) {
            Ok(headers) => headers,
            Err(response) => return response,
        };
    if let Err(response) =
        revoke_customer_api_key_authority(&config, &headers, &customer, form.prefix.trim()).await
    {
        return response;
    }
    api_keys_fragment_markup(&config, &headers, &customer, None).await
}

async fn api_keys_fragment(
    State(config): State<AppConfig>,
    headers: HeaderMap,
    Query(selection): Query<CustomerOrgSelection>,
) -> Response {
    let customer = match config.authenticator.authenticate(&headers).await {
        Ok(customer) => customer,
        Err(response) => return response,
    };
    api_keys_fragment_markup_for_org(
        &config,
        &headers,
        &customer,
        None,
        selection.org_id.as_deref(),
    )
    .await
}

async fn api_keys_fragment_markup(
    config: &AppConfig,
    headers: &HeaderMap,
    customer: &CustomerCtx,
    secret: Option<&str>,
) -> Response {
    api_keys_fragment_markup_for_org(config, headers, customer, secret, None).await
}

async fn api_keys_fragment_markup_for_org(
    config: &AppConfig,
    headers: &HeaderMap,
    customer: &CustomerCtx,
    secret: Option<&str>,
    explicit_org: Option<&str>,
) -> Response {
    let org_id = match selected_customer_org_from(customer, headers, explicit_org) {
        Ok(org_id) => org_id,
        Err(response) => return response,
    };
    let path = format!("/v1/keys?org_id={}", encode_query_value(&org_id));
    let (status, body) = match auth_json(config, headers, reqwest::Method::GET, &path, None).await {
        Ok(result) => result,
        Err(response) => return response,
    };
    if !status.is_success() {
        return proxied_auth_error(status, body);
    }
    let response: AuthKeyListResponse = match serde_json::from_value(body) {
        Ok(response) => response,
        Err(error) => return dependency_error("fiducia-auth", "auth_key_list_bad_response", error),
    };
    let keys = match response
        .keys
        .iter()
        .map(|key| auth_key_to_display(key, &org_id))
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(keys) => keys,
        Err(error) => return dependency_error("fiducia-auth", "auth_key_list_bad_response", error),
    };
    api_keys_table_markup(
        &keys,
        secret,
        &customer_csrf_token(config, customer),
        &org_id,
    )
    .into_response()
}

fn api_keys_table_markup(
    keys: &[serde_json::Value],
    secret: Option<&str>,
    csrf_token: &str,
    org_id: &str,
) -> Markup {
    html! {
        @if let Some(secret) = secret {
            section class="panel secret-once" role="status" {
                h2 { "Copy this secret now" }
                code { (secret) }
                p class="muted" { "The plaintext is returned only by the authoritative auth service for this replay-safe request." }
            }
        }
        section class="panel" aria-labelledby="api-keys-heading" {
            div class="panel__header" {
                h2 id="api-keys-heading" { "Customer API keys" }
                span { (keys.len()) " total" }
            }
            div class="table-wrap" {
                table {
                    thead {
                        tr {
                            th { "Name" }
                            th { "Prefix" }
                            th { "Environment" }
                            th { "Scopes" }
                            th { "State" }
                            th { "Actions" }
                        }
                    }
                    tbody {
                        @if keys.is_empty() {
                            tr { td colspan="6" class="muted" { "No API keys yet." } }
                        } @else {
                            @for key in keys {
                                @let prefix = key.get("prefix").and_then(|value| value.as_str()).unwrap_or("");
                                @let active = key.get("status").and_then(|value| value.as_str()) == Some("active");
                                tr {
                                    td { (key.get("name").and_then(|value| value.as_str()).unwrap_or("")) }
                                    td { code { (key.get("prefix").and_then(|value| value.as_str()).unwrap_or("")) } }
                                    td { (key.get("environment").and_then(|value| value.as_str()).unwrap_or("")) }
                                    td { code { (key.get("scopes").and_then(|value| value.as_str()).unwrap_or("")) } }
                                    td { (key.get("status").and_then(|value| value.as_str()).unwrap_or("")) }
                                    td {
                                        @if active {
                                            form method="post" action="/app/api-keys/rotate"
                                                hx-post="/app/api-keys/rotate" hx-target="#api-key-results" hx-swap="innerHTML" {
                                                input type="hidden" name="csrf_token" value=(csrf_token);
                                                input type="hidden" name="org_id" value=(org_id);
                                                input type="hidden" name="idempotency_key" value=(Uuid::new_v4().to_string());
                                                input type="hidden" name="prefix" value=(prefix);
                                                button type="submit" { "Rotate" }
                                            }
                                            form method="post" action="/app/api-keys/revoke"
                                                hx-post="/app/api-keys/revoke" hx-target="#api-key-results" hx-swap="innerHTML" {
                                                input type="hidden" name="csrf_token" value=(csrf_token);
                                                input type="hidden" name="org_id" value=(org_id);
                                                input type="hidden" name="idempotency_key" value=(Uuid::new_v4().to_string());
                                                input type="hidden" name="prefix" value=(prefix);
                                                button type="submit" { "Revoke" }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

async fn customer_page_response(
    config: &AppConfig,
    headers: &HeaderMap,
    active: CustomerTab,
    explicit_org: Option<&str>,
) -> Response {
    match config.authenticator.authenticate(headers).await {
        Ok(customer) => {
            if let Err(error) = config.request_security.require_api_host(headers) {
                return request_security_error(error);
            }
            let org_id = match customer_page_org(&customer, headers, explicit_org) {
                Ok(org_id) => org_id,
                Err(response) => return response,
            };
            customer_page(
                config,
                &customer,
                active,
                &org_id,
                &customer_csrf_token(config, &customer),
            )
            .into_response()
        }
        Err(response) if response.status() == StatusCode::UNAUTHORIZED => {
            (StatusCode::SEE_OTHER, [(header::LOCATION, "/login")]).into_response()
        }
        Err(response) => response,
    }
}

async fn summary_fragment(State(config): State<AppConfig>, headers: HeaderMap) -> Response {
    protected_fragment(&config, &headers, summary_markup()).await
}

async fn protected_fragment(config: &AppConfig, headers: &HeaderMap, fragment: Markup) -> Response {
    match config.authenticator.authenticate(headers).await {
        Ok(_) => fragment.into_response(),
        Err(response) => response,
    }
}

mod streaming;
pub(crate) use streaming::*;

fn host_serves_customer_app(
    headers: &HeaderMap,
    customer_app_host: &str,
    customer_site_mode: bool,
) -> bool {
    if customer_site_mode {
        return true;
    }

    let Some(host) = headers.get(header::HOST).and_then(|v| v.to_str().ok()) else {
        return false;
    };
    let host = host.split(':').next().unwrap_or(host);
    host.eq_ignore_ascii_case(customer_app_host)
}

fn should_serve_customer_app(config: &AppConfig, headers: &HeaderMap) -> bool {
    host_serves_customer_app(
        headers,
        &config.customer_app_host,
        config.customer_site_mode,
    )
}

mod views;
pub(crate) use views::*;

// Generated API docs (see AGENTS.md "API Docs Contract"). Artifacts are produced
// by remote/tools/generate-api-docs.mjs from the route declarations above and
// committed under generated/; do not hand-edit them.
async fn api_docs_html() -> axum::response::Html<&'static str> {
    axum::response::Html(include_str!("../generated/api-docs.html"))
}

async fn api_docs_json() -> impl axum::response::IntoResponse {
    (
        [("content-type", "application/json; charset=utf-8")],
        include_str!("../generated/api-docs.json"),
    )
}

// Mermaid architecture diagram page (rendered client-side via the Mermaid CDN).
async fn diagram_html() -> axum::response::Html<&'static str> {
    axum::response::Html(include_str!("../docs/diagram.html"))
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod interface_contract_tests;
