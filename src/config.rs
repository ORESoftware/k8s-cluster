use std::env;

use dd_nats_subject_defs::{
    FABRICATION_REQUESTS_QUEUE_GROUP, FABRICATION_REQUESTS_SUBJECT, FABRICATION_RESULTS_SUBJECT,
    MDP_OPTIMIZE_SUBJECT, RUNTIME_EVENTS_SUBJECT,
};

#[derive(Debug, Clone)]
pub(crate) struct ServiceConfig {
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) tcp_port: u16,
    pub(crate) request_subject: String,
    pub(crate) queue_group: String,
    pub(crate) result_subject: String,
    pub(crate) event_subject: String,
    pub(crate) mdp_subject: String,
    pub(crate) mdp_autopublish: bool,
    pub(crate) nats_max_inflight: usize,
    pub(crate) realtime_buffer: usize,
}

impl ServiceConfig {
    pub(crate) fn from_env() -> Result<Self, std::num::ParseIntError> {
        Ok(Self {
            host: env_value("HOST", "0.0.0.0"),
            port: env_value("PORT", "8113").parse::<u16>()?,
            tcp_port: env_value("FABRICATION_TCP_PORT", "8114").parse::<u16>()?,
            request_subject: env_value("FABRICATION_REQUEST_SUBJECT", FABRICATION_REQUESTS_SUBJECT),
            queue_group: env_value("FABRICATION_QUEUE_GROUP", FABRICATION_REQUESTS_QUEUE_GROUP),
            result_subject: env_value("FABRICATION_RESULT_SUBJECT", FABRICATION_RESULTS_SUBJECT),
            event_subject: env_value("FABRICATION_EVENT_SUBJECT", RUNTIME_EVENTS_SUBJECT),
            mdp_subject: env_value("FABRICATION_MDP_OPTIMIZE_SUBJECT", MDP_OPTIMIZE_SUBJECT),
            mdp_autopublish: env_bool("FABRICATION_MDP_AUTOPUBLISH", false),
            nats_max_inflight: env_u64("FABRICATION_NATS_MAX_INFLIGHT", 8, 1, 128) as usize,
            realtime_buffer: env_u64("FABRICATION_REALTIME_BUFFER", 256, 8, 4_096) as usize,
        })
    }
}

pub(crate) fn env_value(key: &str, fallback: &str) -> String {
    env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| fallback.to_string())
}

pub(crate) fn env_bool(key: &str, fallback: bool) -> bool {
    env::var(key)
        .ok()
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(fallback)
}

pub(crate) fn env_u64(key: &str, fallback: u64, min: u64, max: u64) -> u64 {
    env::var(key)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(fallback)
        .clamp(min, max)
}

pub(crate) fn optional_env(key: &str) -> Option<String> {
    env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_kubernetes_service_contract() {
        assert_eq!(
            env_value("DAEDALUS_MISSING_TEST_VALUE", "fallback"),
            "fallback"
        );
        assert!(!env_bool("DAEDALUS_MISSING_TEST_BOOL", false));
        assert_eq!(env_u64("DAEDALUS_MISSING_TEST_U64", 8, 1, 128), 8);
    }
}
