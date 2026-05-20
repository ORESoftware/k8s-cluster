pub mod connections;
pub mod customers;
pub mod health;
pub mod ledger;
pub mod locks;
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
use axum::routing::{get, post};
use std::time::Duration;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;

use crate::state::AppState;

pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(health::healthz))
        .route("/readyz", get(health::readyz))
        .route("/metrics", get(health::metrics))
        // Tenants
        .route("/v1/tenants", post(tenants::create))
        .route("/v1/tenants/{id}", get(tenants::get_by_id))
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
        .layer(TraceLayer::new_for_http())
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(30),
        ))
        .layer(RequestBodyLimitLayer::new(2 * 1024 * 1024))
        .with_state(state)
}
