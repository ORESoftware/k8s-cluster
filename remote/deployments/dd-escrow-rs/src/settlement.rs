use std::collections::HashSet;

use axum::http::{HeaderMap, StatusCode};
use base64::{engine::general_purpose, Engine as _};

use crate::config::{
    MAX_ESCROW_ID_LEN, MAX_MEMO_BYTES, MAX_PARTIES, MAX_SEND_RETRIES,
    MAX_SIGNED_TRANSACTION_BYTES, SCHEMA_VERSION, SETTLEMENT_AUTH_HEADER,
};
use crate::domain::{
    kind_spec, KindSpec, PartyRole, ReleaseMode, ResolutionOutcome, SettlementAction,
};
use crate::state::AppState;
use crate::types::{
    EscrowParty, EscrowResolution, EscrowSettlementRequest, ResolutionAllocation,
    ValidatedSettlement,
};
use crate::util::sensitive_eq;
use crate::validation::{
    normalize_commitment, normalize_encoding, normalize_request_cluster, request_id,
    transaction_digest, validate_escrow_intent, validate_pubkey, validate_request_id,
    validate_token,
};

pub(crate) fn validate_signed_transaction(transaction: &str, encoding: &str) -> Result<Vec<u8>, String> {
    let value = transaction.trim();
    if value.is_empty() {
        return Err("transaction must not be empty".to_string());
    }
    let bytes = match encoding {
        "base64" => general_purpose::STANDARD
            .decode(value)
            .map_err(|error| format!("transaction is not valid base64: {error}"))?,
        "base58" => bs58::decode(value)
            .into_vec()
            .map_err(|error| format!("transaction is not valid base58: {error}"))?,
        other => return Err(format!("unsupported transaction encoding: {other}")),
    };
    if bytes.len() > MAX_SIGNED_TRANSACTION_BYTES {
        return Err(format!(
            "transaction must be at most {MAX_SIGNED_TRANSACTION_BYTES} bytes, got {}",
            bytes.len()
        ));
    }
    Ok(bytes)
}

pub(crate) fn is_refundable_role(role: PartyRole) -> bool {
    matches!(
        role,
        PartyRole::Buyer
            | PartyRole::Payer
            | PartyRole::Depositor
            | PartyRole::Client
            | PartyRole::Tenant
            | PartyRole::Contributor
            | PartyRole::Creator
    )
}

/// Cross-checks a proposed resolution against the escrow parties, the chosen settlement action,
/// and the release mode. Pure policy validation; never touches Solana. Errors and warnings are
/// appended to the shared accumulators so callers can fold them into existing validation output.
pub(crate) fn validate_resolution(
    action: SettlementAction,
    resolution: &EscrowResolution,
    parties: &[EscrowParty],
    spec: &KindSpec,
    release_mode: ReleaseMode,
    errors: &mut Vec<String>,
    warnings: &mut Vec<String>,
) {
    if !resolution.outcome.matches_action(action) {
        errors.push(format!(
            "resolution.outcome {} is not consistent with settlement action {}",
            resolution.outcome.as_str(),
            action.as_str()
        ));
    }
    if !spec.settlement_actions.contains(&action) {
        errors.push(format!(
            "{} escrow does not allow settlement action {}",
            spec.kind.as_str(),
            action.as_str()
        ));
    }

    let roles: HashSet<PartyRole> = parties.iter().map(|party| party.role).collect();
    let arbiter_present = roles.contains(&PartyRole::Arbitrator);
    let arbiter_mode = matches!(
        release_mode,
        ReleaseMode::ArbiterDecision | ReleaseMode::MultiSig
    );

    match resolution.outcome {
        ResolutionOutcome::Refund => {
            if let Some(role) = resolution.refund_role {
                if !is_refundable_role(role) {
                    errors.push(format!(
                        "resolution.refundRole {} is not a refundable role",
                        role.as_str()
                    ));
                }
                if !roles.contains(&role) {
                    errors.push(format!(
                        "resolution.refundRole {} has no matching party",
                        role.as_str()
                    ));
                }
            } else if !roles.iter().copied().any(is_refundable_role) {
                errors.push(
                    "refund resolution requires a refundable party (buyer, payer, depositor, client, tenant, or contributor)"
                        .to_string(),
                );
            }
        }
        ResolutionOutcome::DisputeAward => {
            if arbiter_mode && !arbiter_present {
                errors.push(
                    "dispute-award resolution under arbiter-decision/multi-sig release requires an arbitrator party"
                        .to_string(),
                );
            }
            match resolution.winner_role {
                Some(role) => {
                    if !roles.contains(&role) {
                        errors.push(format!(
                            "resolution.winnerRole {} has no matching party",
                            role.as_str()
                        ));
                    }
                    if role == PartyRole::Arbitrator {
                        errors.push(
                            "resolution.winnerRole must be a disputing party, not the arbitrator"
                                .to_string(),
                        );
                    }
                }
                None => errors
                    .push("dispute-award resolution requires resolution.winnerRole".to_string()),
            }
        }
        ResolutionOutcome::Split => {
            if arbiter_mode && !arbiter_present {
                errors.push(
                    "split resolution under arbiter-decision/multi-sig release requires an arbitrator party"
                        .to_string(),
                );
            }
            match &resolution.allocations {
                Some(allocations) if !allocations.is_empty() => {
                    validate_resolution_allocations(allocations, &roles, errors);
                }
                _ => errors
                    .push("split resolution requires a non-empty resolution.allocations".to_string()),
            }
        }
        ResolutionOutcome::Release | ResolutionOutcome::Expire | ResolutionOutcome::Cancel => {}
    }

    if resolution.winner_role.is_some() && resolution.outcome != ResolutionOutcome::DisputeAward {
        warnings.push(format!(
            "resolution.winnerRole is ignored for {} outcomes",
            resolution.outcome.as_str()
        ));
    }
    if resolution.allocations.is_some() && resolution.outcome != ResolutionOutcome::Split {
        warnings.push(format!(
            "resolution.allocations is ignored for {} outcomes",
            resolution.outcome.as_str()
        ));
    }
    if let Some(rationale) = &resolution.rationale {
        if rationale.as_bytes().len() > MAX_MEMO_BYTES {
            errors.push(format!(
                "resolution.rationale must be at most {MAX_MEMO_BYTES} bytes"
            ));
        }
    }
}

pub(crate) fn validate_resolution_allocations(
    allocations: &[ResolutionAllocation],
    roles: &HashSet<PartyRole>,
    errors: &mut Vec<String>,
) {
    if allocations.len() > MAX_PARTIES {
        errors.push(format!(
            "resolution.allocations must include at most {MAX_PARTIES} entries"
        ));
    }
    let mut sum: u32 = 0;
    for (index, allocation) in allocations.iter().enumerate() {
        sum += u32::from(allocation.payout_bps);
        if allocation.payout_bps > 10_000 {
            errors.push(format!(
                "resolution.allocations[{index}].payoutBps must be at most 10000"
            ));
        }
        if !roles.contains(&allocation.role) {
            errors.push(format!(
                "resolution.allocations[{index}].role {} has no matching party",
                allocation.role.as_str()
            ));
        }
        if let Some(pubkey) = &allocation.pubkey {
            if let Err(error) =
                validate_pubkey(pubkey, &format!("resolution.allocations[{index}].pubkey"))
            {
                errors.push(error);
            }
        }
    }
    if sum != 10_000 {
        errors.push(
            "resolution.allocations payoutBps values must sum to exactly 10000".to_string(),
        );
    }
}

pub(crate) fn validate_settlement_request(
    request: &EscrowSettlementRequest,
    default_cluster: &str,
    allow_skip_preflight: bool,
    allowed_program_ids: &[String],
    require_intent: bool,
) -> Result<ValidatedSettlement, Vec<String>> {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    if request.schema_version != SCHEMA_VERSION {
        errors.push(format!(
            "schemaVersion must be {SCHEMA_VERSION}, got {}",
            request.schema_version
        ));
    }
    validate_request_id(request.request_id.as_ref(), &mut errors);
    validate_token(
        &request.escrow_id,
        "escrowId",
        MAX_ESCROW_ID_LEN,
        &mut errors,
    );
    let cluster = match normalize_request_cluster(request.cluster.as_deref(), default_cluster) {
        Ok(cluster) => cluster,
        Err(error) => {
            errors.push(error);
            default_cluster.to_string()
        }
    };
    let spec = kind_spec(request.kind);
    if !spec.settlement_actions.contains(&request.action) {
        errors.push(format!(
            "{} escrow does not allow settlement action {}",
            request.kind.as_str(),
            request.action.as_str()
        ));
    }
    let encoding = match normalize_encoding(request.encoding.as_deref()) {
        Ok(encoding) => encoding,
        Err(error) => {
            errors.push(error);
            "base64"
        }
    };
    if let Err(error) = normalize_commitment(request.commitment.as_deref()) {
        errors.push(error);
    }
    if request.skip_preflight == Some(true) && !allow_skip_preflight {
        errors.push(
            "skipPreflight is disabled by policy; set SOLANA_ALLOW_SKIP_PREFLIGHT=true to allow it"
                .to_string(),
        );
    }
    if let Some(max_retries) = request.max_retries {
        if max_retries > MAX_SEND_RETRIES {
            errors.push(format!(
                "maxRetries must be at most {MAX_SEND_RETRIES}, got {max_retries}"
            ));
        }
    }
    let transaction_bytes = match validate_signed_transaction(&request.transaction, encoding) {
        Ok(bytes) => bytes,
        Err(error) => {
            errors.push(error);
            Vec::new()
        }
    };
    if let Some(intent) = &request.intent {
        match validate_escrow_intent(intent, default_cluster, allowed_program_ids) {
            Ok(intent_response) => {
                if intent.kind != request.kind {
                    errors.push("intent.kind must match settlement kind".to_string());
                }
                if intent.escrow_id != request.escrow_id {
                    errors.push("intent.escrowId must match settlement escrowId".to_string());
                }
                if intent_response.cluster != cluster {
                    errors.push("intent.cluster must match settlement cluster".to_string());
                }
                warnings.extend(intent_response.warnings);
            }
            Err(intent_errors) => {
                errors.extend(
                    intent_errors
                        .into_iter()
                        .map(|error| format!("intent.{error}")),
                );
            }
        }
        if let Some(resolution) = &request.resolution {
            validate_resolution(
                request.action,
                resolution,
                &intent.parties,
                &spec,
                intent.terms.release_mode,
                &mut errors,
                &mut warnings,
            );
        }
    } else {
        if require_intent {
            errors.push(
                "intent is required for live settlement; set ESCROW_SETTLEMENT_REQUIRE_INTENT=false only for a reviewed operator exception".to_string(),
            );
        } else {
            warnings.push("no intent was attached; settlement action is validated only against kind and transaction policy".to_string());
        }
        if request.resolution.is_some() {
            errors.push(
                "resolution requires an attached intent so the proposed outcome can be checked against the escrow parties"
                    .to_string(),
            );
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    Ok(ValidatedSettlement {
        request_id: request_id(request.request_id.as_ref(), "escrow-settlement"),
        cluster,
        transaction_digest: transaction_digest(&transaction_bytes),
        transaction_bytes,
        warnings,
    })
}

pub(crate) fn authorize_settlement(
    headers: &HeaderMap,
    state: &AppState,
) -> Result<(), (StatusCode, &'static str)> {
    let Some(secret) = &state.settlement_auth_secret else {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "settlement sending is not configured with ESCROW_SETTLEMENT_AUTH_SECRET",
        ));
    };
    let Some(value) = headers
        .get(SETTLEMENT_AUTH_HEADER)
        .and_then(|value| value.to_str().ok())
    else {
        return Err((
            StatusCode::UNAUTHORIZED,
            "missing x-escrow-settlement-auth header",
        ));
    };
    if !sensitive_eq(value.trim(), secret) {
        return Err((
            StatusCode::UNAUTHORIZED,
            "invalid x-escrow-settlement-auth header",
        ));
    }
    Ok(())
}
