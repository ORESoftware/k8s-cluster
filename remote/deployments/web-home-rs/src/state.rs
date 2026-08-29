use serde::Serialize;

#[derive(Clone)]
pub(crate) struct AppState {
    pub(crate) server_label: String,
    pub(crate) control_plane_label: String,
    pub(crate) workers_label: String,
    pub(crate) queue_consumer_label: String,
}

#[derive(Serialize)]
pub(crate) struct HealthResponse {
    pub(crate) ok: bool,
    pub(crate) service: String,
    pub(crate) mode: String,
}
