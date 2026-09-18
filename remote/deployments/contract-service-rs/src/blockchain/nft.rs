//! NFT / media storage. Validates NFT metadata (Metaplex for Solana, ERC-721/1155
//! for EVM), and stores media content-addressed by sha256 into a pluggable
//! [`super::MediaStore`] (today a bounded in-memory map). Returns a deterministic
//! digest/URI. Read-only with respect to chains — it does not mint or sign.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use base64::{engine::general_purpose, Engine as _};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::atomic::Ordering;

use super::{
    evict_oldest, json_err, json_ok, parse_chain, record_request, require_enabled, ChainKind,
    MediaObject, MediaStore, MAX_MEDIA_BYTES, MAX_MEDIA_OBJECTS,
};
use crate::AppState;

const MAX_CONTENT_TYPE_LEN: usize = 128;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MetadataRequest {
    chain: String,
    metadata: Value,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MediaRequest {
    content_type: String,
    /// base64-encoded media bytes.
    data_base64: String,
}

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route("/nft/metadata/validate", post(validate_metadata_http))
        .route("/nft/media", post(store_media_http))
        .route("/nft/media/:digest", get(get_media_http))
}

async fn validate_metadata_http(
    State(state): State<AppState>,
    Json(body): Json<MetadataRequest>,
) -> axum::response::Response {
    let bc = &state.blockchain;
    record_request(bc);
    if let Err(resp) = require_enabled(bc, bc.config().nft_enabled, "BLOCKCHAIN_NFT_ENABLED") {
        return resp;
    }
    let chain = match parse_chain(&body.chain) {
        Ok(kind) => kind,
        Err(error) => return json_err(StatusCode::BAD_REQUEST, &error),
    };
    if let Err(error) = validate_metadata(chain, &body.metadata) {
        return json_err(StatusCode::BAD_REQUEST, &error);
    }
    let digest = sha256_hex(body.metadata.to_string().as_bytes());
    json_ok(json!({
        "ok": true,
        "chain": super::chain_label(chain),
        "standard": standard_for(chain),
        "metadataDigest": format!("sha256:{digest}"),
    }))
}

async fn store_media_http(
    State(state): State<AppState>,
    Json(body): Json<MediaRequest>,
) -> axum::response::Response {
    let bc = &state.blockchain;
    record_request(bc);
    if let Err(resp) = require_enabled(bc, bc.config().nft_enabled, "BLOCKCHAIN_NFT_ENABLED") {
        return resp;
    }
    let content_type = body.content_type.trim();
    if content_type.is_empty() || content_type.len() > MAX_CONTENT_TYPE_LEN {
        return json_err(StatusCode::BAD_REQUEST, "contentType must be 1..=128 characters");
    }
    let bytes = match general_purpose::STANDARD.decode(body.data_base64.trim()) {
        Ok(bytes) => bytes,
        Err(error) => {
            return json_err(StatusCode::BAD_REQUEST, &format!("dataBase64 invalid: {error}"))
        }
    };
    if bytes.is_empty() {
        return json_err(StatusCode::BAD_REQUEST, "media must not be empty");
    }
    if bytes.len() > MAX_MEDIA_BYTES {
        return json_err(StatusCode::PAYLOAD_TOO_LARGE, "media exceeds the size limit");
    }
    let digest = sha256_hex(&bytes);
    let size = bytes.len();
    {
        let mut store = match bc.inner().media.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        let MediaStore::InMemory(map) = &mut *store;
        evict_oldest(map, MAX_MEDIA_OBJECTS, |obj| obj.created_ms);
        map.entry(digest.clone()).or_insert_with(|| MediaObject {
            content_type: content_type.to_string(),
            bytes,
            created_ms: crate::now_ms(),
        });
    }
    bc.metrics()
        .nft_media_stored_total
        .fetch_add(1, Ordering::Relaxed);
    json_ok(json!({
        "ok": true,
        "digest": format!("sha256:{digest}"),
        "uri": format!("dd-media://sha256/{digest}"),
        "bytes": size,
        "backend": "in-memory",
    }))
}

async fn get_media_http(
    State(state): State<AppState>,
    Path(digest): Path<String>,
) -> axum::response::Response {
    let bc = &state.blockchain;
    record_request(bc);
    if let Err(resp) = require_enabled(bc, bc.config().nft_enabled, "BLOCKCHAIN_NFT_ENABLED") {
        return resp;
    }
    let key = digest.trim().strip_prefix("sha256:").unwrap_or(digest.trim()).to_string();
    let store = match bc.inner().media.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    let MediaStore::InMemory(map) = &*store;
    match map.get(&key) {
        Some(obj) => json_ok(json!({
            "ok": true,
            "digest": format!("sha256:{key}"),
            "contentType": obj.content_type,
            "bytes": obj.bytes.len(),
            "dataBase64": general_purpose::STANDARD.encode(&obj.bytes),
        })),
        None => json_err(StatusCode::NOT_FOUND, "media not found"),
    }
}

fn standard_for(chain: ChainKind) -> &'static str {
    match chain {
        ChainKind::Solana => "metaplex",
        ChainKind::Evm => "erc-721/1155",
    }
}

/// Minimal but real metadata checks per standard.
fn validate_metadata(chain: ChainKind, metadata: &Value) -> Result<(), String> {
    let object = metadata
        .as_object()
        .ok_or_else(|| "metadata must be a JSON object".to_string())?;
    let name = object
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "metadata.name is required".to_string())?;
    if name.chars().count() > 256 {
        return Err("metadata.name is too long".to_string());
    }
    match chain {
        ChainKind::Solana => {
            // Metaplex requires a symbol and at least one creator-or-image hint.
            object
                .get("symbol")
                .and_then(Value::as_str)
                .ok_or_else(|| "metaplex metadata requires `symbol`".to_string())?;
            if object.get("image").is_none() && object.get("uri").is_none() {
                return Err("metaplex metadata requires `image` or `uri`".to_string());
            }
        }
        ChainKind::Evm => {
            // ERC-721/1155 metadata requires an image URI.
            object
                .get("image")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| "erc-721/1155 metadata requires `image`".to_string())?;
        }
    }
    Ok(())
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn metaplex_requires_symbol_and_image() {
        assert!(validate_metadata(ChainKind::Solana, &json!({ "name": "x", "symbol": "X", "image": "u" })).is_ok());
        assert!(validate_metadata(ChainKind::Solana, &json!({ "name": "x", "symbol": "X" })).is_err());
        assert!(validate_metadata(ChainKind::Solana, &json!({ "name": "x" })).is_err());
    }

    #[test]
    fn erc_requires_image() {
        assert!(validate_metadata(ChainKind::Evm, &json!({ "name": "x", "image": "ipfs://y" })).is_ok());
        assert!(validate_metadata(ChainKind::Evm, &json!({ "name": "x" })).is_err());
    }
}
