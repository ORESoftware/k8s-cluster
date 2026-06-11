//! Multi-signature coordinator. Tracks M-of-N proposals: clients create a
//! proposal (chain, threshold, signer set, payload digest), then each signer
//! submits its own signature. The coordinator verifies signer membership and
//! de-duplicates, reports when the threshold is met, and surfaces the collected
//! signatures for the caller to assemble + broadcast externally. It is keyless:
//! it never holds a key and never signs.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::atomic::Ordering;

use super::{
    evict_oldest, gen_id, json_err, json_ok, parse_chain, record_request, require_enabled,
    validate_chain_address, ChainKind, MAX_PROPOSALS,
};
use crate::AppState;

const MAX_SIGNERS: usize = 32;
const MAX_DIGEST_LEN: usize = 256;
const MAX_SIGNATURE_LEN: usize = 256;

pub(crate) struct Proposal {
    pub id: String,
    pub chain: ChainKind,
    pub threshold: usize,
    pub signers: Vec<String>,
    pub payload_digest: String,
    pub signatures: BTreeMap<String, String>,
    pub created_ms: u128,
}

impl Proposal {
    fn met(&self) -> bool {
        self.signatures.len() >= self.threshold
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateRequest {
    chain: String,
    threshold: usize,
    signers: Vec<String>,
    payload_digest: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApproveRequest {
    signer: String,
    signature: String,
}

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route("/multisig/proposal", post(create_http))
        .route("/multisig/proposal/:id", get(status_http))
        .route("/multisig/proposal/:id/approve", post(approve_http))
}

async fn create_http(
    State(state): State<AppState>,
    Json(body): Json<CreateRequest>,
) -> axum::response::Response {
    let bc = &state.blockchain;
    record_request(bc);
    if let Err(resp) = require_enabled(bc, bc.config().multisig_enabled, "BLOCKCHAIN_MULTISIG_ENABLED")
    {
        return resp;
    }
    let chain = match parse_chain(&body.chain) {
        Ok(kind) => kind,
        Err(error) => return json_err(StatusCode::BAD_REQUEST, &error),
    };
    if body.signers.is_empty() || body.signers.len() > MAX_SIGNERS {
        return json_err(StatusCode::BAD_REQUEST, "signers must be 1..=32 entries");
    }
    if body.threshold == 0 || body.threshold > body.signers.len() {
        return json_err(StatusCode::BAD_REQUEST, "threshold must be 1..=signers.len()");
    }
    let digest = body.payload_digest.trim();
    if digest.is_empty() || digest.len() > MAX_DIGEST_LEN {
        return json_err(StatusCode::BAD_REQUEST, "payloadDigest must be 1..=256 characters");
    }
    // Validate + canonicalize each signer address; reject duplicates.
    let mut signers: Vec<String> = Vec::with_capacity(body.signers.len());
    for raw in &body.signers {
        match validate_chain_address(chain, raw) {
            Ok(value) => {
                if signers.contains(&value) {
                    return json_err(StatusCode::BAD_REQUEST, "duplicate signer in set");
                }
                signers.push(value);
            }
            Err(error) => return json_err(StatusCode::BAD_REQUEST, &error),
        }
    }

    let id = gen_id("multisig");
    let now = crate::now_ms();
    {
        let mut proposals = match bc.inner().proposals.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        evict_oldest(&mut proposals, MAX_PROPOSALS, |p| p.created_ms);
        proposals.insert(
            id.clone(),
            Proposal {
                id: id.clone(),
                chain,
                threshold: body.threshold,
                signers: signers.clone(),
                payload_digest: digest.to_string(),
                signatures: BTreeMap::new(),
                created_ms: now,
            },
        );
    }
    bc.metrics()
        .multisig_proposals_total
        .fetch_add(1, Ordering::Relaxed);
    json_ok(json!({
        "ok": true,
        "id": id,
        "chain": super::chain_label(chain),
        "threshold": body.threshold,
        "signers": signers,
        "custody": "keyless-coordinator",
    }))
}

async fn approve_http(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<ApproveRequest>,
) -> axum::response::Response {
    let bc = &state.blockchain;
    record_request(bc);
    if let Err(resp) = require_enabled(bc, bc.config().multisig_enabled, "BLOCKCHAIN_MULTISIG_ENABLED")
    {
        return resp;
    }
    let signature = body.signature.trim();
    if signature.is_empty() || signature.len() > MAX_SIGNATURE_LEN {
        return json_err(StatusCode::BAD_REQUEST, "signature must be 1..=256 characters");
    }

    let mut proposals = match bc.inner().proposals.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    let Some(proposal) = proposals.get_mut(&id) else {
        return json_err(StatusCode::NOT_FOUND, "proposal not found");
    };
    let signer = match validate_chain_address(proposal.chain, &body.signer) {
        Ok(value) => value,
        Err(error) => return json_err(StatusCode::BAD_REQUEST, &error),
    };
    if !proposal.signers.contains(&signer) {
        return json_err(StatusCode::FORBIDDEN, "signer is not part of this proposal");
    }
    let already = proposal.signatures.insert(signer.clone(), signature.to_string());
    if already.is_none() {
        bc.metrics()
            .multisig_approvals_total
            .fetch_add(1, Ordering::Relaxed);
    }
    json_ok(json!({
        "ok": true,
        "id": proposal.id,
        "signer": signer,
        "collected": proposal.signatures.len(),
        "threshold": proposal.threshold,
        "thresholdMet": proposal.met(),
        "replacedExisting": already.is_some(),
    }))
}

async fn status_http(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> axum::response::Response {
    let bc = &state.blockchain;
    record_request(bc);
    if let Err(resp) = require_enabled(bc, bc.config().multisig_enabled, "BLOCKCHAIN_MULTISIG_ENABLED")
    {
        return resp;
    }
    let proposals = match bc.inner().proposals.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    match proposals.get(&id) {
        Some(p) => json_ok(json!({
            "ok": true,
            "id": p.id,
            "chain": super::chain_label(p.chain),
            "threshold": p.threshold,
            "signers": p.signers,
            "payloadDigest": p.payload_digest,
            "collected": p.signatures.len(),
            "thresholdMet": p.met(),
            "signatures": p.signatures,
            "createdMs": p.created_ms.to_string(),
        })),
        None => json_err(StatusCode::NOT_FOUND, "proposal not found"),
    }
}
