use std::{collections::HashSet, env, net::IpAddr};

use crate::validation::validate_pubkey;

pub(crate) const SCHEMA_VERSION: &str = "solana.escrow.v1";
pub(crate) const SERVICE_NAME: &str = "dd-escrow-rs";
pub(crate) const SERVICE_NAMESPACE: &str = "remote-dev";
pub(crate) const LOG_SCHEMA: &str = "dd.log.v1";
pub(crate) const LOG_SCOPE: &str = "dd-escrow-rs";
pub(crate) const DEFAULT_COMMITMENT: &str = "confirmed";
pub(crate) const SETTLEMENT_AUTH_HEADER: &str = "x-escrow-settlement-auth";
pub(crate) const CONTRACT_SEND_AUTH_HEADER: &str = "x-contract-send-auth";
pub(crate) const DEFAULT_CONTRACT_SERVICE_TIMEOUT_SECONDS: u64 = 20;
pub(crate) const MAX_HTTP_BODY_BYTES: usize = 512 * 1024;
pub(crate) const MAX_NATS_PAYLOAD_BYTES: usize = 512 * 1024;
pub(crate) const MAX_SIGNED_TRANSACTION_BYTES: usize = 256 * 1024;
pub(crate) const MAX_REQUEST_ID_LEN: usize = 128;
pub(crate) const MAX_ESCROW_ID_LEN: usize = 128;
pub(crate) const MAX_LABEL_LEN: usize = 80;
pub(crate) const MAX_MEMO_BYTES: usize = 1024;
pub(crate) const MAX_METADATA_BYTES: usize = 4096;
pub(crate) const MAX_PARTIES: usize = 12;
pub(crate) const MAX_MILESTONES: usize = 24;
pub(crate) const MAX_TOKEN_AMOUNT_LEN: usize = 80;
pub(crate) const MAX_DISPUTE_WINDOW_SECONDS: u64 = 90 * 24 * 60 * 60;
pub(crate) const MAX_INSPECTION_SECONDS: u64 = 30 * 24 * 60 * 60;
pub(crate) const MAX_SEND_RETRIES: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettlementBackend {
    SolanaRpc,
    ContractService,
}

impl SettlementBackend {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            SettlementBackend::SolanaRpc => "solana-rpc",
            SettlementBackend::ContractService => "contract-service",
        }
    }

    pub(crate) fn parse(input: &str) -> Result<Self, String> {
        match input.trim().to_ascii_lowercase().as_str() {
            "solana-rpc" | "solana_rpc" | "rpc" => Ok(SettlementBackend::SolanaRpc),
            "contract-service" | "contract_service" | "contract" => {
                Ok(SettlementBackend::ContractService)
            }
            other => Err(format!(
                "ESCROW_SETTLEMENT_BACKEND must be solana-rpc or contract-service, got {other}"
            )),
        }
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
                "1" | "true" | "yes"
            )
        })
        .unwrap_or(fallback)
}

pub(crate) fn env_u64(key: &str, fallback: u64) -> u64 {
    env::var(key)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(fallback)
}

pub(crate) fn env_secret(key: &str) -> Option<String> {
    env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub(crate) fn env_pubkey_list(key: &str) -> Result<Vec<String>, String> {
    let Some(raw) = env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    else {
        return Ok(Vec::new());
    };
    let mut values = Vec::new();
    let mut seen = HashSet::new();
    for item in raw.split(',') {
        let value = item.trim();
        if value.is_empty() {
            continue;
        }
        validate_pubkey(value, key)?;
        if seen.insert(value.to_string()) {
            values.push(value.to_string());
        }
    }
    Ok(values)
}

pub(crate) fn config_error(message: impl Into<String>) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidInput, message.into())
}

/// Validates the in-cluster `dd-contract-service` base URL. Unlike `SOLANA_RPC_URL`, this is an
/// internal service address, so cluster-local `http://*.svc.cluster.local` hosts are allowed; we
/// still reject embedded credentials and require a host.
pub(crate) fn validate_contract_service_url(raw: &str) -> Result<String, String> {
    let parsed = reqwest::Url::parse(raw)
        .map_err(|error| format!("CONTRACT_SERVICE_URL must be an absolute URL: {error}"))?;
    match parsed.scheme() {
        "http" | "https" => {}
        scheme => {
            return Err(format!(
                "CONTRACT_SERVICE_URL scheme must be http or https, got {scheme}"
            ))
        }
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err("CONTRACT_SERVICE_URL must not include credentials".to_string());
    }
    if parsed.host_str().is_none() {
        return Err("CONTRACT_SERVICE_URL must include a host".to_string());
    }
    Ok(parsed.to_string().trim_end_matches('/').to_string())
}

pub(crate) fn validate_solana_rpc_url(raw: &str, allow_private_rpc: bool) -> Result<String, String> {
    let parsed = reqwest::Url::parse(raw)
        .map_err(|error| format!("SOLANA_RPC_URL must be an absolute URL: {error}"))?;
    match parsed.scheme() {
        "https" => {}
        "http" if allow_private_rpc => {}
        "http" => {
            return Err(
                "SOLANA_RPC_URL must use https unless SOLANA_ALLOW_PRIVATE_RPC=true".to_string(),
            )
        }
        scheme => {
            return Err(format!(
                "SOLANA_RPC_URL scheme must be https or http, got {scheme}"
            ))
        }
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err("SOLANA_RPC_URL must not include credentials".to_string());
    }
    let Some(host) = parsed.host_str() else {
        return Err("SOLANA_RPC_URL must include a host".to_string());
    };
    if !allow_private_rpc {
        let host_lower = host.to_ascii_lowercase();
        if matches!(
            host_lower.as_str(),
            "localhost" | "metadata.google.internal"
        ) || host_lower.ends_with(".local")
            || host_lower.ends_with(".cluster.local")
        {
            return Err(
                "SOLANA_RPC_URL points at a private host; set SOLANA_ALLOW_PRIVATE_RPC=true to allow it"
                    .to_string(),
            );
        }
        if let Ok(ip) = host.parse::<IpAddr>() {
            let private_ip = match ip {
                IpAddr::V4(address) => {
                    address.is_private()
                        || address.is_loopback()
                        || address.is_link_local()
                        || address.is_broadcast()
                        || address.is_unspecified()
                }
                IpAddr::V6(address) => {
                    address.is_loopback()
                        || address.is_unspecified()
                        || address.is_unique_local()
                        || address.is_unicast_link_local()
                }
            };
            if private_ip {
                return Err(
                    "SOLANA_RPC_URL points at a private IP; set SOLANA_ALLOW_PRIVATE_RPC=true to allow it"
                        .to_string(),
                );
            }
        }
    }
    Ok(parsed.to_string())
}
