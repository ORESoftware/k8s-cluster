//! Optional HTMX admin UI mounted at `/admin`.
//!
//! Posture:
//!   - **Read-mostly.** Writes are limited to small, idempotent actions that
//!     also exist as JSON endpoints (run-now, enable/disable, sync-now).
//!   - **No regressions.** The JSON API in [`crate::api`] is untouched. The
//!     admin surface is a parallel router gated by `BILLING_ADMIN_UI_ENABLED`
//!     (default on).
//!   - **No new client toolchain.** HTML is rendered with [`maud`] (compile-
//!     time templates) and the only client-side dependency is `htmx.min.js`
//!     served from a CDN with SRI integrity.
//!
//! The route surface mirrors the JSON API:
//!   - `GET  /admin/`                         dashboard
//!   - `GET  /admin/tenants`                  list + create form
//!   - `POST /admin/tenants`                  create (HTMX form, returns row)
//!   - `GET  /admin/tenants/{id}`             tenant detail (tabs land here)
//!   - `GET  /admin/tenants/{id}/connections` connections table fragment
//!   - `GET  /admin/tenants/{id}/jobs`        scheduled-jobs table fragment
//!   - `GET  /admin/tenants/{id}/locks`       leases table fragment
//!   - `GET  /admin/tenants/{id}/notifications`  rules + dispatches fragment
//!   - `POST /admin/jobs/{job_id}/run-now`    HTMX action, returns row markup
//!   - `POST /admin/jobs/{job_id}/toggle`     HTMX action, returns row markup
//!   - `POST /admin/connections/{conn_id}/sync` HTMX action, returns row markup
//!   - `GET  /admin/status`                   auto-refresh status pill fragment
//!
//! Each handler returns either a full-page response (for direct navigation)
//! or a fragment (for HTMX swaps) based on the `HX-Request` header.

mod connections;
mod dashboard;
mod jobs;
mod layout;
mod locks;
mod notifications;
mod tenants;
mod time;

use axum::Router;
use axum::routing::{get, post};

use crate::state::AppState;

/// Build the admin router. Mount under `/admin` (handled by the caller).
pub fn build_router() -> Router<AppState> {
    Router::new()
        .route("/", get(dashboard::page))
        .route("/status", get(dashboard::status_fragment))
        .route("/tenants", get(tenants::list_page).post(tenants::create))
        .route("/tenants/{id}", get(tenants::detail_page))
        .route(
            "/tenants/{id}/connections",
            get(connections::table_fragment),
        )
        .route("/tenants/{id}/jobs", get(jobs::table_fragment))
        .route("/tenants/{id}/locks", get(locks::table_fragment))
        .route(
            "/tenants/{id}/notifications",
            get(notifications::page_fragment),
        )
        .route("/jobs/{job_id}/run-now", post(jobs::run_now))
        .route("/jobs/{job_id}/toggle", post(jobs::toggle))
        .route("/connections/{conn_id}/sync", post(connections::sync_now))
}
