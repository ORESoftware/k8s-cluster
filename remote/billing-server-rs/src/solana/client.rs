//! Minimal Solana JSON-RPC client.
//!
//! We deliberately avoid the heavy `solana-sdk` / `solana-client` crates here
//! to keep the build small. For the operations the billing server actually
//! needs — submit a memo transaction, fetch a signature for verification,
//! query signatures-for-address — JSON-RPC over reqwest is plenty.

use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::error::{AppError, AppResult};

#[derive(Clone)]
pub struct SolanaClient {
    pub rpc_url: String,
    http: reqwest::Client,
}

#[derive(Serialize)]
struct RpcRequest<'a, P: Serialize> {
    jsonrpc: &'static str,
    id: u64,
    method: &'a str,
    params: P,
}

#[derive(Deserialize)]
struct RpcResponse<R> {
    #[allow(dead_code)]
    jsonrpc: Option<String>,
    #[allow(dead_code)]
    id: u64,
    result: Option<R>,
    error: Option<RpcErr>,
}

#[derive(Deserialize, Debug)]
struct RpcErr {
    code: i64,
    message: String,
}

impl SolanaClient {
    pub fn new(cfg: &Config) -> Self {
        Self {
            rpc_url: cfg.solana_rpc_url.clone(),
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .expect("reqwest client"),
        }
    }

    pub async fn get_slot(&self) -> AppResult<u64> {
        let resp: RpcResponse<u64> = self.call("getSlot", &serde_json::json!([])).await?;
        resp.result.ok_or_else(|| AppError::Provider {
            provider: "solana".into(),
            message: "getSlot returned no result".into(),
        })
    }

    /// Fetch a confirmed/finalized transaction by signature. Returns the raw
    /// JSON value so callers can extract whatever they need (memo data, block
    /// time, slot) without us defining an exhaustive schema here.
    pub async fn get_transaction(
        &self,
        signature: &str,
    ) -> AppResult<Option<serde_json::Value>> {
        let resp: RpcResponse<serde_json::Value> = self
            .call(
                "getTransaction",
                &serde_json::json!([
                    signature,
                    { "encoding": "jsonParsed", "commitment": "finalized", "maxSupportedTransactionVersion": 0 }
                ]),
            )
            .await?;
        Ok(resp.result)
    }

    async fn call<P: Serialize, R: for<'de> Deserialize<'de>>(
        &self,
        method: &str,
        params: &P,
    ) -> AppResult<RpcResponse<R>> {
        let req = RpcRequest { jsonrpc: "2.0", id: 1, method, params };
        let resp = self
            .http
            .post(&self.rpc_url)
            .json(&req)
            .send()
            .await
            .map_err(|e| AppError::Provider {
                provider: "solana".into(),
                message: format!("rpc transport: {e}"),
            })?
            .json::<RpcResponse<R>>()
            .await
            .map_err(|e| AppError::Provider {
                provider: "solana".into(),
                message: format!("rpc decode: {e}"),
            })?;

        if let Some(err) = &resp.error {
            return Err(AppError::Provider {
                provider: "solana".into(),
                message: format!("rpc error {}: {}", err.code, err.message),
            });
        }
        Ok(resp)
    }
}
