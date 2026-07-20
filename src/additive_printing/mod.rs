//! Additive-manufacturing features isolated from the legacy planner core.

use axum::Router;

use crate::realtime::EventHub;

mod analysis;
mod http;
mod model;

pub(crate) const ADDITIVE_PREFLIGHT_SCHEMA: &str = "dd.fabrication.additive-preflight.v1";

pub(crate) fn router<S>(hub: EventHub) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    http::router(hub)
}
