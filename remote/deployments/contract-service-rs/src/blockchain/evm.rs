//! Minimal, dependency-light EVM JSON-RPC client plus address helpers.
//!
//! This is deliberately small: the blockchain scaffold is keyless and read/relay
//! oriented, so we only need JSON-RPC plumbing, EIP-55 address validation, and
//! keccak256 (for checksums and log-topic hashing). Full ABI encoding is out of
//! scope — callers pass pre-encoded `data` — so we do not pull in `alloy`/`ethers`.

use serde_json::{json, Value};
use sha3::{Digest, Keccak256};

/// keccak256 over arbitrary bytes.
pub(super) fn keccak256(bytes: &[u8]) -> [u8; 32] {
    let mut hasher = Keccak256::new();
    hasher.update(bytes);
    let out = hasher.finalize();
    let mut buf = [0u8; 32];
    buf.copy_from_slice(&out);
    buf
}

/// Returns the EIP-55 mixed-case checksum form of a hex address (`0x` + 40 hex).
pub(super) fn to_checksum_address(addr: &str) -> Result<String, String> {
    let raw = strip_hex_prefix(addr);
    if raw.len() != 40 || !raw.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("EVM address must be 0x followed by 40 hex characters".to_string());
    }
    let lower = raw.to_ascii_lowercase();
    let hash = keccak256(lower.as_bytes());
    let mut out = String::with_capacity(42);
    out.push_str("0x");
    for (index, ch) in lower.chars().enumerate() {
        let nibble = if index % 2 == 0 {
            hash[index / 2] >> 4
        } else {
            hash[index / 2] & 0x0f
        };
        if ch.is_ascii_alphabetic() && nibble >= 8 {
            out.push(ch.to_ascii_uppercase());
        } else {
            out.push(ch);
        }
    }
    Ok(out)
}

/// Validates an EVM address and returns its canonical EIP-55 checksum form. A
/// mixed-case input must already be a valid checksum; an all-lower/all-upper
/// input is accepted and normalized.
pub(super) fn validate_evm_address(addr: &str) -> Result<String, String> {
    let trimmed = addr.trim();
    let checksum = to_checksum_address(trimmed)?;
    let raw = strip_hex_prefix(trimmed);
    let has_lower = raw.chars().any(|c| c.is_ascii_lowercase());
    let has_upper = raw.chars().any(|c| c.is_ascii_uppercase());
    if has_lower && has_upper && trimmed != checksum {
        return Err("EVM address fails the EIP-55 checksum".to_string());
    }
    Ok(checksum)
}

/// Validates a `0x`-prefixed hex blob of arbitrary even length (calldata, raw tx).
pub(super) fn validate_hex_blob(value: &str, label: &str, max_bytes: usize) -> Result<String, String> {
    let trimmed = value.trim();
    let raw = strip_hex_prefix(trimmed);
    if raw.is_empty() {
        return Err(format!("{label} must not be empty"));
    }
    if raw.len() % 2 != 0 {
        return Err(format!("{label} must have an even number of hex digits"));
    }
    if !raw.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!("{label} must be 0x-prefixed hex"));
    }
    if raw.len() / 2 > max_bytes {
        return Err(format!("{label} exceeds {max_bytes} bytes"));
    }
    Ok(format!("0x{}", raw.to_ascii_lowercase()))
}

/// True for EVM mainnet (chain id 1, or a network label that names it).
pub(super) fn is_evm_mainnet(chain_id: Option<u64>, network: &str) -> bool {
    chain_id == Some(1)
        || matches!(network.to_ascii_lowercase().as_str(), "mainnet" | "ethereum" | "homestead")
}

fn strip_hex_prefix(value: &str) -> &str {
    value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .unwrap_or(value)
}

/// Issues a single JSON-RPC call against an EVM node and returns the `result`
/// value, surfacing JSON-RPC `error` objects as `Err`.
pub(super) async fn evm_rpc(
    client: &reqwest::Client,
    url: &str,
    method: &str,
    params: Value,
) -> Result<Value, String> {
    let payload = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": method,
        "params": params,
    });
    let response = client
        .post(url)
        .json(&payload)
        .send()
        .await
        .map_err(|error| format!("EVM RPC request failed: {error}"))?;
    let status = response.status();
    let body: Value = response
        .json()
        .await
        .map_err(|error| format!("EVM RPC returned a non-JSON body: {error}"))?;
    if !status.is_success() {
        return Err(format!("EVM RPC returned HTTP {status}"));
    }
    if let Some(error) = body.get("error") {
        return Err(format!("EVM RPC error: {error}"));
    }
    Ok(body.get("result").cloned().unwrap_or(Value::Null))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksums_known_addresses() {
        // Canonical EIP-55 vectors.
        for addr in [
            "0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed",
            "0xfB6916095ca1df60bB79Ce92cE3Ea74c37c5d359",
            "0xdbF03B407c01E7cD3CBea99509d93f8DDDC8C6FB",
            "0xD1220A0cf47c7B9Be7A2E6BA89F429762e7b9aDb",
        ] {
            assert_eq!(to_checksum_address(&addr.to_lowercase()).unwrap(), addr);
            assert_eq!(validate_evm_address(addr).unwrap(), addr);
        }
    }

    #[test]
    fn rejects_bad_checksum_and_shape() {
        // Mixed case that is not valid EIP-55.
        assert!(validate_evm_address("0x5aaeb6053f3E94C9b9A09f33669435E7Ef1BeAed").is_err());
        assert!(validate_evm_address("0x123").is_err());
        assert!(validate_evm_address("5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAez").is_err());
    }

    #[test]
    fn hex_blob_validation() {
        assert_eq!(validate_hex_blob("0xAB", "data", 16).unwrap(), "0xab");
        assert!(validate_hex_blob("0xA", "data", 16).is_err());
        assert!(validate_hex_blob("0x", "data", 16).is_err());
        assert!(validate_hex_blob("0xabcd", "data", 1).is_err());
    }

    #[test]
    fn mainnet_detection() {
        assert!(is_evm_mainnet(Some(1), "sepolia"));
        assert!(is_evm_mainnet(None, "mainnet"));
        assert!(!is_evm_mainnet(Some(11155111), "sepolia"));
    }
}
