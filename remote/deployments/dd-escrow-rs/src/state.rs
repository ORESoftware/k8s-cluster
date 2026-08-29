use std::{sync::Arc, time::Duration};

use crate::config::SettlementBackend;
use crate::metrics::Metrics;

#[derive(Clone)]
pub(crate) struct AppState {
    pub(crate) rpc_client: reqwest::Client,
    pub(crate) solana_rpc_url: String,
    pub(crate) default_cluster: String,
    pub(crate) settlement_backend: SettlementBackend,
    pub(crate) contract_service_url: Option<String>,
    pub(crate) contract_service_send_secret: Option<String>,
    pub(crate) contract_service_timeout: Duration,
    pub(crate) settlement_enabled: bool,
    pub(crate) settlement_auth_secret: Option<String>,
    pub(crate) settlement_require_intent: bool,
    pub(crate) allowed_program_ids: Vec<String>,
    pub(crate) allow_skip_preflight: bool,
    pub(crate) nats: Option<async_nats::Client>,
    pub(crate) validate_subject: String,
    pub(crate) result_subject: String,
    pub(crate) event_subject: String,
    pub(crate) critical_event_subject: String,
    pub(crate) metrics: Arc<Metrics>,
}
