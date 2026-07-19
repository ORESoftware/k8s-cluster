//! Environment contract for the separately deployable web process.

use std::net::SocketAddr;

use dd_nats_subject_defs::{FABRICATION_RESULTS_SUBJECT, RUNTIME_EVENTS_SUBJECT};

use crate::config::{env_u64, env_value, optional_env};

#[derive(Clone)]
pub(crate) struct WebConfig {
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) tcp_port: u16,
    pub(crate) event_buffer: usize,
    pub(crate) backend_ws_url: String,
    pub(crate) nats_result_subject: String,
    pub(crate) nats_event_subject: String,
    pub(crate) supabase: Option<SupabaseConfig>,
}

#[derive(Clone)]
pub(crate) struct SupabaseConfig {
    pub(crate) project_url: String,
    pub(crate) publishable_key: String,
    pub(crate) topic: String,
    pub(crate) schema: String,
    pub(crate) table: String,
}

impl WebConfig {
    pub(crate) fn from_env() -> Result<Self, std::num::ParseIntError> {
        Ok(Self {
            host: env_value("FABRICATION_WEB_HOST", "0.0.0.0"),
            port: env_value("FABRICATION_WEB_PORT", "8115").parse()?,
            tcp_port: env_value("FABRICATION_WEB_TCP_PORT", "8116").parse()?,
            event_buffer: env_u64("FABRICATION_WEB_EVENT_BUFFER", 256, 8, 4_096) as usize,
            backend_ws_url: env_value(
                "FABRICATION_BACKEND_WS_URL",
                "ws://dd-fabrication-server:8113/ws/json",
            ),
            nats_result_subject: env_value(
                "FABRICATION_WEB_NATS_SUBJECT",
                FABRICATION_RESULTS_SUBJECT,
            ),
            nats_event_subject: env_value("FABRICATION_WEB_EVENT_SUBJECT", RUNTIME_EVENTS_SUBJECT),
            supabase: SupabaseConfig::from_env(),
        })
    }

    pub(crate) fn http_address(&self) -> Result<SocketAddr, std::net::AddrParseError> {
        format!("{}:{}", self.host, self.port).parse()
    }

    pub(crate) fn tcp_address(&self) -> Result<SocketAddr, std::net::AddrParseError> {
        format!("{}:{}", self.host, self.tcp_port).parse()
    }
}

impl SupabaseConfig {
    fn from_env() -> Option<Self> {
        let project_url = optional_env("SUPABASE_URL")?;
        let publishable_key = optional_env("SUPABASE_PUBLISHABLE_KEY")
            .or_else(|| optional_env("SUPABASE_ANON_KEY"))?;
        Some(Self {
            project_url,
            publishable_key,
            topic: env_value("SUPABASE_REALTIME_TOPIC", "daedalus-fabrication"),
            schema: env_value("SUPABASE_REALTIME_SCHEMA", "public"),
            table: env_value("SUPABASE_REALTIME_TABLE", "fabrication_events"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_ports_are_separate_from_the_fabrication_api_defaults() {
        let http: SocketAddr = "0.0.0.0:8115".parse().expect("HTTP address");
        let tcp: SocketAddr = "0.0.0.0:8116".parse().expect("TCP address");

        assert_ne!(http.port(), 8113);
        assert_ne!(tcp.port(), http.port());
        assert_eq!(FABRICATION_RESULTS_SUBJECT, "dd.remote.fabrication.results");
    }
}
