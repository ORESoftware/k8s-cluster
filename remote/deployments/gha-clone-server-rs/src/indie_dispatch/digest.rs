use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::model::DispatchRequest;

pub(super) fn stable_request_id(request: &DispatchRequest) -> Result<String, String> {
    let mut identity = request.clone();
    identity.request_id.clear();
    identity.request_digest.clear();
    let digest = canonical_digest(&identity)?;
    let component = digest
        .strip_prefix("sha256:")
        .ok_or_else(|| "internal request identity digest error".to_string())?
        .chars()
        .take(48)
        .collect::<String>();
    Ok(format!("gha:{component}"))
}

pub(super) fn request_digest(request: &DispatchRequest) -> Result<String, String> {
    let mut unsigned = request.clone();
    unsigned.request_digest.clear();
    canonical_digest(&unsigned)
}

fn canonical_digest<T: Serialize>(value: &T) -> Result<String, String> {
    let value = serde_json::to_value(value)
        .map_err(|error| format!("failed to canonicalize dispatch JSON: {error}"))?;
    let bytes = serde_json::to_vec(&canonicalize_json(value))
        .map_err(|error| format!("failed to serialize canonical dispatch JSON: {error}"))?;
    Ok(format!("sha256:{}", hex::encode(Sha256::digest(bytes))))
}

fn canonicalize_json(value: Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.into_iter().map(canonicalize_json).collect()),
        Value::Object(values) => {
            let sorted = values
                .into_iter()
                .map(|(key, value)| (key, canonicalize_json(value)))
                .collect::<BTreeMap<_, _>>();
            Value::Object(sorted.into_iter().collect())
        }
        scalar => scalar,
    }
}
