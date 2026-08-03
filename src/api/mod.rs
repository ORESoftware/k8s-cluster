pub mod auth;
pub mod connections;
pub mod customers;
pub mod health;
pub mod ledger;
pub mod locks;
pub mod memberships;
pub mod notifications;
pub mod oauth;
pub mod scheduler;
pub mod tenants;
pub mod users;
pub mod vendors;
pub mod verify;
pub mod webhooks;

use axum::Router;
use axum::http::StatusCode;
use axum::middleware;
use axum::response::{Html, IntoResponse};
use axum::routing::{get, post};
use std::time::Duration;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;

use crate::state::AppState;

async fn api_docs_html() -> Html<&'static str> {
    Html(include_str!("../../generated/api-docs.html"))
}

async fn api_docs_json() -> impl IntoResponse {
    (
        [("content-type", "application/json; charset=utf-8")],
        include_str!("../../generated/api-docs.json"),
    )
}

pub fn build_router(state: AppState) -> anyhow::Result<Router> {
    let admin_enabled = state.cfg.admin_ui_enabled;
    let api_auth = auth::ApiAuth::from_state(&state)?;
    if api_auth.bearer.is_none() {
        tracing::warn!(
            "BILLING_API_AUTH_BEARER is unset — legacy service authentication is disabled"
        );
    } else {
        tracing::info!(
            "legacy service bearer enabled for migration-only, non-user control-plane calls"
        );
    }
    if api_auth.shared_auth.is_some() {
        tracing::info!(
            "Shared Auth revocation-aware introspection enabled; tenant authorization is database-backed"
        );
    } else {
        tracing::warn!(
            "Shared Auth is disabled. This is permitted only while \
             BILLING_TENANT_ROUTES_REQUIRE_USER_JWT=false or in explicit insecure development."
        );
    }
    if api_auth.require_user_jwt {
        tracing::info!(
            "tenant routes require an active Shared Auth session and exact tenant membership"
        );
    } else {
        tracing::warn!(
            "BILLING_TENANT_ROUTES_REQUIRE_USER_JWT=false — legacy service credentials can read \
             tenant routes. Keep this migration window short."
        );
    }
    if api_auth.require_step_up {
        tracing::info!(
            "tenant mutations require fresh Shared Auth LOA2 plus tenant-local billing:write"
        );
    } else {
        tracing::warn!(
            "tenant mutation step-up is disabled; enable the production hardening flag after rollout"
        );
    }

    let mut router = Router::new()
        .route("/healthz", get(health::healthz))
        .route("/readyz", get(health::readyz))
        .route("/metrics", get(health::metrics))
        .route("/docs/api", get(api_docs_html))
        .route("/api/docs", get(api_docs_html))
        .route("/api/docs.json", get(api_docs_json))
        // Tenants
        .route("/v1/tenants", post(tenants::create))
        .route("/v1/tenants/{id}", get(tenants::get_by_id))
        // Shared Auth principal grants (Quaestor-owned authorization)
        .route(
            "/v1/tenants/{tenant_id}/memberships",
            get(memberships::list),
        )
        .route(
            "/v1/tenants/{tenant_id}/memberships/{shared_user_id}",
            axum::routing::put(memberships::upsert),
        )
        .route(
            "/v1/tenants/{tenant_id}/memberships/{shared_user_id}",
            axum::routing::delete(memberships::revoke),
        )
        // Users (per-tenant customers/vendors)
        .route("/v1/tenants/{tenant_id}/users", post(users::upsert))
        .route(
            "/v1/tenants/{tenant_id}/users/by-email/{email}",
            get(users::get_by_email),
        )
        // Customer billing state (Q1)
        .route(
            "/v1/tenants/{tenant_id}/customers/by-email/{email}/billing-state",
            get(customers::billing_state),
        )
        // Vendor payable state (Q2)
        .route(
            "/v1/tenants/{tenant_id}/vendors/by-email/{email}/payable-state",
            get(vendors::payable_state),
        )
        // Ledger primitives
        .route(
            "/v1/tenants/{tenant_id}/accounts",
            post(ledger::ensure_account),
        )
        .route(
            "/v1/tenants/{tenant_id}/transactions",
            post(ledger::post_transaction),
        )
        .route(
            "/v1/tenants/{tenant_id}/accounts/{code}/balance",
            get(ledger::account_balance),
        )
        // Provider connections
        .route(
            "/v1/tenants/{tenant_id}/connections",
            get(connections::list),
        )
        .route(
            "/v1/tenants/{tenant_id}/connections",
            post(connections::create),
        )
        // On-demand sync (the primary poll path — backstop poller defaults to 5x/day)
        .route(
            "/v1/tenants/{tenant_id}/connections/{connection_id}/sync",
            post(connections::sync_now),
        )
        // API-key attach (for non-OAuth providers: Coinflow, Coinbase, Wise, ...)
        .route(
            "/v1/tenants/{tenant_id}/connections/{connection_id}/attach-api-key",
            post(connections::attach_api_key),
        )
        // OAuth handshake
        .route("/v1/oauth/{provider}/start", get(oauth::start))
        .route("/v1/oauth/{provider}/callback", get(oauth::callback))
        // Plaid Link (not OAuth — frontend exchanges public_token via this route)
        .route("/v1/plaid/link-token", post(oauth::plaid_link_token))
        .route("/v1/plaid/exchange", post(oauth::plaid_exchange))
        // Webhooks (one endpoint per provider)
        .route("/v1/webhooks/stripe", post(webhooks::stripe))
        .route("/v1/webhooks/paypal", post(webhooks::paypal))
        .route("/v1/webhooks/coinbase", post(webhooks::coinbase))
        .route("/v1/webhooks/plaid", post(webhooks::plaid))
        .route("/v1/webhooks/coinflow", post(webhooks::coinflow))
        .route("/v1/webhooks/revolut", post(webhooks::revolut))
        .route("/v1/webhooks/gocardless", post(webhooks::gocardless))
        .route("/v1/webhooks/mercury", post(webhooks::mercury))
        .route("/v1/webhooks/bridge", post(webhooks::bridge))
        .route("/v1/webhooks/fireblocks", post(webhooks::fireblocks))
        .route("/v1/webhooks/circle", post(webhooks::circle))
        .route("/v1/webhooks/adyen", post(webhooks::adyen))
        .route("/v1/webhooks/square", post(webhooks::square))
        .route(
            "/v1/webhooks/modern_treasury",
            post(webhooks::modern_treasury),
        )
        .route("/v1/webhooks/dwolla", post(webhooks::dwolla))
        // Public verification (no auth required — that's the point)
        .route(
            "/v1/verify/tenants/{tenant_id}/postings/{id}",
            get(verify::verify_posting),
        )
        // Tenant-scoped leases ("locks")
        .route("/v1/tenants/{tenant_id}/locks", post(locks::acquire))
        .route("/v1/tenants/{tenant_id}/locks", get(locks::list))
        .route(
            "/v1/tenants/{tenant_id}/locks/{resource}/renew",
            post(locks::renew),
        )
        .route(
            "/v1/tenants/{tenant_id}/locks/{resource}",
            axum::routing::delete(locks::release),
        )
        // Scheduled jobs
        .route(
            "/v1/tenants/{tenant_id}/scheduled-jobs",
            post(scheduler::create),
        )
        .route(
            "/v1/tenants/{tenant_id}/scheduled-jobs",
            get(scheduler::list),
        )
        .route(
            "/v1/tenants/{tenant_id}/scheduled-jobs/{id}",
            get(scheduler::get_one),
        )
        .route(
            "/v1/tenants/{tenant_id}/scheduled-jobs/{id}/runs",
            get(scheduler::list_runs),
        )
        .route(
            "/v1/tenants/{tenant_id}/scheduled-jobs/{id}/run-now",
            post(scheduler::run_now),
        )
        .route(
            "/v1/tenants/{tenant_id}/scheduled-jobs/{id}/disable",
            post(scheduler::disable),
        )
        .route(
            "/v1/tenants/{tenant_id}/scheduled-jobs/{id}/enable",
            post(scheduler::enable),
        )
        // Notifications
        .route(
            "/v1/tenants/{tenant_id}/notification-rules",
            post(notifications::create_rule),
        )
        .route(
            "/v1/tenants/{tenant_id}/notification-rules",
            get(notifications::list_rules),
        )
        .route(
            "/v1/tenants/{tenant_id}/notification-dispatches",
            get(notifications::list_dispatches),
        )
        .layer(middleware::from_fn_with_state(
            api_auth.clone(),
            auth::require_api_auth,
        ))
        .layer(TraceLayer::new_for_http())
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(30),
        ))
        .layer(RequestBodyLimitLayer::new(2 * 1024 * 1024));

    if admin_enabled {
        router = router.nest("/admin", crate::admin::build_router(state.clone()));
    }

    Ok(router.with_state(state))
}
