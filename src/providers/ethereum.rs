//! Ethereum / EVM wallet observer.
//!
//! Like the Solana wallet provider, this is strictly read-only. Tenants provide
//! an Ethereum address and an RPC endpoint. The server can read native balances,
//! ERC-20 balances, and receipts, but never stores private keys and never calls
//! `eth_sendTransaction`.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::time::Duration;

use crate::error::{AppError, AppResult};

const HTTP_TIMEOUT: Duration = Duration::from_secs(15);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Serialize, Deserialize)]
pub struct EthereumWalletCredential {
    pub address: String,
    pub rpc_url: String,
    #[serde(default = "default_chain_id")]
    pub chain_id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rpc_bearer_token: Option<String>,
    #[serde(default)]
    pub tracked_assets: Vec<EthereumTrackedAsset>,
}

impl fmt::Debug for EthereumWalletCredential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EthereumWalletCredential")
            .field("address", &self.address)
            .field("rpc_url", &self.rpc_url)
            .field("chain_id", &self.chain_id)
            .field(
                "rpc_bearer_token",
                &self.rpc_bearer_token.as_ref().map(|_| "<redacted>"),
            )
            .field("tracked_assets", &self.tracked_assets)
            .finish()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EthereumTrackedAsset {
    pub symbol: String,
    pub contract_address: Option<String>,
    #[serde(default = "default_erc20_decimals")]
    pub decimals: u8,
}

#[derive(Clone, Debug, Deserialize)]
pub struct EthereumRpcResponse<T> {
    #[serde(default)]
    pub jsonrpc: Option<String>,
    #[serde(default)]
    pub id: Option<serde_json::Value>,
    #[serde(default)]
    pub result: Option<T>,
    #[serde(default)]
    pub error: Option<EthereumRpcError>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct EthereumRpcError {
    pub code: i64,
    pub message: String,
    #[serde(flatten)]
    pub raw: serde_json::Value,
}

#[derive(Clone, Debug, Deserialize)]
pub struct EthereumReceipt {
    #[serde(default, rename = "transactionHash")]
    pub transaction_hash: Option<String>,
    #[serde(default, rename = "blockNumber")]
    pub block_number: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(flatten)]
    pub raw: serde_json::Value,
}

#[derive(Clone)]
pub struct EthereumWalletApi {
    cred: EthereumWalletCredential,
    http: reqwest::Client,
    rpc_url: String,
}

impl EthereumWalletApi {
    pub fn new(cred: EthereumWalletCredential) -> AppResult<Self> {
        let rpc_url = normalize_rpc_url(
            "ethereum_wallet.rpc_url",
            &cred.rpc_url,
            BaseUrlMode::Runtime,
        )?;
        validate_address("ethereum_wallet.address", &cred.address)?;
        Ok(Self {
            cred,
            http: http_client("ethereum_wallet")?,
            rpc_url,
        })
    }

    #[cfg(test)]
    pub fn with_rpc_url_for_tests(
        mut cred: EthereumWalletCredential,
        rpc_url: String,
    ) -> AppResult<Self> {
        validate_address("ethereum_wallet.address", &cred.address)?;
        let rpc_url = normalize_rpc_url("ethereum_wallet.rpc_url", &rpc_url, BaseUrlMode::Test)?;
        cred.rpc_url = rpc_url.clone();
        Ok(Self {
            cred,
            http: http_client("ethereum_wallet")?,
            rpc_url,
        })
    }

    pub async fn get_native_balance_wei(&self, block: Option<&str>) -> AppResult<String> {
        let block = block.unwrap_or("latest");
        let response: EthereumRpcResponse<String> = self
            .rpc_call(
                "eth_getBalance",
                serde_json::json!([self.cred.address, block]),
            )
            .await?;
        rpc_result("ethereum_wallet.eth_getBalance", response)
    }

    pub async fn get_chain_id(&self) -> AppResult<String> {
        let response: EthereumRpcResponse<String> =
            self.rpc_call("eth_chainId", serde_json::json!([])).await?;
        rpc_result("ethereum_wallet.eth_chainId", response)
    }

    pub async fn get_transaction_receipt(
        &self,
        tx_hash: &str,
    ) -> AppResult<Option<EthereumReceipt>> {
        validate_tx_hash("ethereum_wallet.tx_hash", tx_hash)?;
        let response: EthereumRpcResponse<Option<EthereumReceipt>> = self
            .rpc_call("eth_getTransactionReceipt", serde_json::json!([tx_hash]))
            .await?;
        rpc_result("ethereum_wallet.eth_getTransactionReceipt", response)
    }

    pub async fn get_erc20_balance(
        &self,
        contract_address: &str,
        block: Option<&str>,
    ) -> AppResult<String> {
        validate_address("ethereum_wallet.contract_address", contract_address)?;
        let data = erc20_balance_of_calldata(&self.cred.address)?;
        let response: EthereumRpcResponse<String> = self
            .rpc_call(
                "eth_call",
                serde_json::json!([{
                    "to": contract_address,
                    "data": data
                }, block.unwrap_or("latest")]),
            )
            .await?;
        rpc_result("ethereum_wallet.eth_call", response)
    }

    async fn rpc_call<T: for<'de> Deserialize<'de>>(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> AppResult<T> {
        let payload = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": method,
            "params": params
        });
        let mut req = self
            .http
            .post(&self.rpc_url)
            .header("Content-Type", "application/json")
            .json(&payload);
        if let Some(token) = self.cred.rpc_bearer_token.as_deref() {
            req = req.bearer_auth(token);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| provider_err("ethereum_wallet", format!("{method} HTTP: {e}")))?;
        decode_json_response("ethereum_wallet", resp, method).await
    }
}

fn default_chain_id() -> u64 {
    1
}

fn default_erc20_decimals() -> u8 {
    6
}

pub fn validate_ethereum_rpc_url(value: &str) -> AppResult<String> {
    normalize_rpc_url("ethereum_wallet.rpc_url", value, BaseUrlMode::Runtime)
}

pub fn validate_ethereum_address(value: &str) -> AppResult<()> {
    validate_address("ethereum_wallet.address", value).map(|_| ())
}

fn erc20_balance_of_calldata(address: &str) -> AppResult<String> {
    let lower = validate_address("ethereum_wallet.address", address)?;
    Ok(format!(
        "0x70a08231000000000000000000000000{}",
        lower.trim_start_matches("0x")
    ))
}

fn validate_address(field: &str, value: &str) -> AppResult<String> {
    let trimmed = value.trim();
    let Some(rest) = trimmed.strip_prefix("0x") else {
        return Err(AppError::BadRequest(format!("{field} must start with 0x")));
    };
    if rest.len() != 40 || !rest.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(AppError::BadRequest(format!(
            "{field} must be a 20-byte hex address"
        )));
    }
    Ok(format!("0x{}", rest.to_ascii_lowercase()))
}

fn validate_tx_hash(field: &str, value: &str) -> AppResult<()> {
    let trimmed = value.trim();
    let Some(rest) = trimmed.strip_prefix("0x") else {
        return Err(AppError::BadRequest(format!("{field} must start with 0x")));
    };
    if rest.len() != 64 || !rest.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(AppError::BadRequest(format!(
            "{field} must be a 32-byte hex transaction hash"
        )));
    }
    Ok(())
}

fn rpc_result<T>(label: &str, response: EthereumRpcResponse<T>) -> AppResult<T> {
    if let Some(error) = response.error {
        return Err(provider_err(
            "ethereum_wallet",
            format!("{label} rpc error {}: {}", error.code, error.message),
        ));
    }
    response
        .result
        .ok_or_else(|| provider_err("ethereum_wallet", format!("{label} missing result")))
}

fn http_client(provider: &str) -> AppResult<reqwest::Client> {
    reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(HTTP_TIMEOUT)
        .build()
        .map_err(|e| provider_err(provider, format!("HTTP client build: {e}")))
}

#[derive(Clone, Copy)]
enum BaseUrlMode {
    Runtime,
    Test,
}

fn normalize_rpc_url(field: &str, value: &str, mode: BaseUrlMode) -> AppResult<String> {
    let trimmed = value.trim();
    let parsed = url::Url::parse(trimmed)
        .map_err(|e| AppError::BadRequest(format!("{field} must be a valid URL: {e}")))?;
    let allow_http = matches!(mode, BaseUrlMode::Test);
    if parsed.scheme() != "https" && !(allow_http && parsed.scheme() == "http") {
        return Err(AppError::BadRequest(format!("{field} must use https")));
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(AppError::BadRequest(format!(
            "{field} must not include URL credentials"
        )));
    }
    if matches!(mode, BaseUrlMode::Runtime) {
        validate_runtime_host(field, &parsed)?;
    }
    Ok(trimmed.to_string())
}

fn validate_runtime_host(field: &str, parsed: &url::Url) -> AppResult<()> {
    let Some(host) = parsed.host() else {
        return Err(AppError::BadRequest(format!("{field} must include a host")));
    };
    match host {
        url::Host::Domain(domain) => {
            let host = domain.trim_end_matches('.').to_ascii_lowercase();
            if host == "localhost"
                || host.ends_with(".localhost")
                || host.ends_with(".local")
                || host.ends_with(".internal")
                || !host.contains('.')
            {
                return Err(AppError::BadRequest(format!(
                    "{field} must use a public RPC hostname"
                )));
            }
        }
        url::Host::Ipv4(addr) => {
            if addr.is_private()
                || addr.is_loopback()
                || addr.is_link_local()
                || addr.is_unspecified()
                || addr.is_broadcast()
                || addr.is_multicast()
            {
                return Err(AppError::BadRequest(format!(
                    "{field} must not target a private or local IP"
                )));
            }
        }
        url::Host::Ipv6(addr) => {
            if addr.is_loopback()
                || addr.is_unspecified()
                || addr.is_unique_local()
                || addr.is_unicast_link_local()
                || addr.is_multicast()
            {
                return Err(AppError::BadRequest(format!(
                    "{field} must not target a private or local IP"
                )));
            }
        }
    }
    Ok(())
}

async fn decode_json_response<T: for<'de> Deserialize<'de>>(
    provider: &str,
    resp: reqwest::Response,
    label: &str,
) -> AppResult<T> {
    let status = resp.status();
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| provider_err(provider, format!("{label} body: {e}")))?;
    if !status.is_success() {
        return Err(provider_err(
            provider,
            format!("{label} {status}: {}", String::from_utf8_lossy(&bytes)),
        ));
    }
    serde_json::from_slice(&bytes)
        .map_err(|e| provider_err(provider, format!("{label} decode: {e}")))
}

fn provider_err(provider: &str, message: String) -> AppError {
    AppError::Provider {
        provider: provider.into(),
        message,
    }
}
