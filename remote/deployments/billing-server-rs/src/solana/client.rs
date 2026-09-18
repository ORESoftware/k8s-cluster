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

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SolanaSignatureInfo {
    pub signature: String,
    pub slot: u64,
    pub err: Option<serde_json::Value>,
    pub memo: Option<String>,
    pub block_time: Option<i64>,
    pub confirmation_status: Option<String>,
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
    pub async fn get_transaction(&self, signature: &str) -> AppResult<Option<serde_json::Value>> {
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

    pub async fn get_signatures_for_address(
        &self,
        address_b58: &str,
        before: Option<&str>,
        until: Option<&str>,
        limit: usize,
    ) -> AppResult<Vec<SolanaSignatureInfo>> {
        let mut opts = serde_json::json!({
            "commitment": "finalized",
            "limit": limit.clamp(1, 1_000),
        });
        if let Some(before) = before {
            opts["before"] = serde_json::Value::String(before.to_string());
        }
        if let Some(until) = until {
            opts["until"] = serde_json::Value::String(until.to_string());
        }

        let resp: RpcResponse<Vec<SolanaSignatureInfo>> = self
            .call(
                "getSignaturesForAddress",
                &serde_json::json!([address_b58, opts]),
            )
            .await?;
        resp.result.ok_or_else(|| AppError::Provider {
            provider: "solana".into(),
            message: "getSignaturesForAddress returned no result".into(),
        })
    }

    async fn call<P: Serialize, R: for<'de> Deserialize<'de>>(
        &self,
        method: &str,
        params: &P,
    ) -> AppResult<RpcResponse<R>> {
        let mut last_error: Option<AppError> = None;

        for attempt in 0..3 {
            let req = RpcRequest {
                jsonrpc: "2.0",
                id: 1,
                method,
                params,
            };
            let response = match self.http.post(&self.rpc_url).json(&req).send().await {
                Ok(response) => response,
                Err(e) => {
                    last_error = Some(AppError::Provider {
                        provider: "solana".into(),
                        message: format!("rpc transport: {e}"),
                    });
                    sleep_before_retry(attempt).await;
                    continue;
                }
            };

            let status = response.status();
            let body = response.text().await.map_err(|e| AppError::Provider {
                provider: "solana".into(),
                message: format!("rpc response read: {e}"),
            })?;

            if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                let retry_after = retry_delay_seconds(attempt);
                last_error = Some(AppError::ProviderRateLimited {
                    provider: "solana".into(),
                    retry_after_seconds: retry_after,
                    message: format!("rpc {method} returned HTTP 429"),
                });
                sleep_before_retry(attempt).await;
                continue;
            }

            if status.is_server_error() {
                last_error = Some(AppError::Provider {
                    provider: "solana".into(),
                    message: format!("rpc {method} returned HTTP {status}: {body}"),
                });
                sleep_before_retry(attempt).await;
                continue;
            }

            if !status.is_success() {
                return Err(AppError::Provider {
                    provider: "solana".into(),
                    message: format!("rpc {method} returned HTTP {status}: {body}"),
                });
            }

            let resp =
                serde_json::from_str::<RpcResponse<R>>(&body).map_err(|e| AppError::Provider {
                    provider: "solana".into(),
                    message: format!("rpc decode: {e}"),
                })?;

            if let Some(err) = &resp.error {
                return Err(AppError::Provider {
                    provider: "solana".into(),
                    message: format!("rpc error {}: {}", err.code, err.message),
                });
            }
            return Ok(resp);
        }

        Err(last_error.unwrap_or_else(|| AppError::Provider {
            provider: "solana".into(),
            message: format!("rpc {method} failed after retries"),
        }))
    }
}

async fn sleep_before_retry(attempt: u32) {
    if attempt < 2 {
        tokio::time::sleep(std::time::Duration::from_secs(
            retry_delay_seconds(attempt) as u64
        ))
        .await;
    }
}

fn retry_delay_seconds(attempt: u32) -> i64 {
    match attempt {
        0 => 1,
        1 => 2,
        _ => 4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::mock_http::{ExpectedRequest, ProviderMock};

    #[tokio::test]
    async fn get_slot_posts_json_rpc_payload_to_mock() {
        let mock = ProviderMock::start(vec![
            ExpectedRequest::post("/")
                .json_body(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "getSlot",
                    "params": []
                }))
                .respond_json(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "result": 42
                })),
        ])
        .await;
        let client = SolanaClient {
            rpc_url: mock.base_url(),
            http: reqwest::Client::new(),
        };

        let slot = client.get_slot().await.unwrap();

        assert_eq!(slot, 42);
        mock.assert_finished().await;
    }

    #[tokio::test]
    async fn get_signatures_for_address_clamps_limit() {
        let mock = ProviderMock::start(vec![
            ExpectedRequest::post("/")
                .json_body(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "getSignaturesForAddress",
                    "params": [
                        "So11111111111111111111111111111111111111112",
                        { "commitment": "finalized", "limit": 1000 }
                    ]
                }))
                .respond_json(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "result": [{
                        "signature": "sig_1",
                        "slot": 42,
                        "err": null,
                        "memo": null,
                        "blockTime": 1716423000,
                        "confirmationStatus": "finalized"
                    }]
                })),
        ])
        .await;
        let client = SolanaClient {
            rpc_url: mock.base_url(),
            http: reqwest::Client::new(),
        };

        let signatures = client
            .get_signatures_for_address(
                "So11111111111111111111111111111111111111112",
                None,
                None,
                5000,
            )
            .await
            .unwrap();

        assert_eq!(signatures[0].signature, "sig_1");
        mock.assert_finished().await;
    }
}
