use std::collections::HashSet;

use sha2::{Digest, Sha256};

use crate::config::{
    DEFAULT_COMMITMENT, MAX_DISPUTE_WINDOW_SECONDS, MAX_ESCROW_ID_LEN, MAX_INSPECTION_SECONDS,
    MAX_LABEL_LEN, MAX_MEMO_BYTES, MAX_METADATA_BYTES, MAX_MILESTONES, MAX_PARTIES,
    MAX_REQUEST_ID_LEN, MAX_TOKEN_AMOUNT_LEN, SCHEMA_VERSION,
};
use crate::domain::{
    kind_spec, AssetType, EscrowKind, KindSpec, PartyRole, ReleaseMode, SettlementAction,
};
use crate::types::{
    EscrowAsset, EscrowIntentRequest, EscrowMilestone, EscrowParty, EscrowPolicyCheck,
    EscrowReadiness, EscrowValidationResponse, SettlementPlan,
};
use crate::util::{now_ms, now_unix_seconds};

pub(crate) fn request_id(input: Option<&String>, prefix: &str) -> String {
    input
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .unwrap_or(prefix)
        .to_string()
}

pub(crate) fn validate_request_id(input: Option<&String>, errors: &mut Vec<String>) {
    let Some(value) = input else {
        return;
    };
    validate_token(value, "requestId", MAX_REQUEST_ID_LEN, errors);
}

pub(crate) fn validate_token(value: &str, label: &str, max_len: usize, errors: &mut Vec<String>) {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        errors.push(format!("{label} must not be empty"));
        return;
    }
    if trimmed.len() != value.len() {
        errors.push(format!(
            "{label} must not contain leading or trailing whitespace"
        ));
    }
    if trimmed.len() > max_len {
        errors.push(format!("{label} must be at most {max_len} bytes"));
    }
    if !trimmed
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | ':'))
    {
        errors.push(format!(
            "{label} may contain only ASCII letters, numbers, '.', '_', '-', and ':'"
        ));
    }
}

pub(crate) fn validate_label(value: &str, label: &str, errors: &mut Vec<String>) {
    validate_token(value, label, MAX_LABEL_LEN, errors);
}

pub(crate) fn validate_pubkey(value: &str, label: &str) -> Result<(), String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(format!("{label} must not be empty"));
    }
    if trimmed.len() != value.len() {
        return Err(format!(
            "{label} must not contain leading or trailing whitespace"
        ));
    }
    let decoded = bs58::decode(trimmed)
        .into_vec()
        .map_err(|error| format!("{label} must be valid base58: {error}"))?;
    if decoded.len() != 32 {
        return Err(format!(
            "{label} must decode to a 32 byte Solana public key, got {} bytes",
            decoded.len()
        ));
    }
    Ok(())
}

pub(crate) fn normalize_cluster(input: Option<&str>, fallback: &str) -> Result<String, String> {
    let value = input
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(fallback);
    let normalized = value.to_ascii_lowercase();
    match normalized.as_str() {
        "mainnet-beta" | "devnet" | "testnet" | "localnet" | "custom" => Ok(normalized),
        _ => Err(format!(
            "cluster must be one of mainnet-beta, devnet, testnet, localnet, or custom: {value}"
        )),
    }
}

pub(crate) fn normalize_request_cluster(
    input: Option<&str>,
    configured_cluster: &str,
) -> Result<String, String> {
    let cluster = normalize_cluster(input, configured_cluster)?;
    if cluster != configured_cluster {
        return Err(format!(
            "cluster must match configured SOLANA_CLUSTER ({configured_cluster}), got {cluster}"
        ));
    }
    Ok(cluster)
}

pub(crate) fn normalize_commitment(input: Option<&str>) -> Result<String, String> {
    let value = input
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_COMMITMENT);
    let normalized = value.to_ascii_lowercase();
    match normalized.as_str() {
        "processed" | "confirmed" | "finalized" => Ok(normalized),
        _ => Err(format!(
            "commitment must be processed, confirmed, or finalized: {value}"
        )),
    }
}

pub(crate) fn normalize_encoding(input: Option<&str>) -> Result<&'static str, String> {
    let value = input
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("base64");
    match value.to_ascii_lowercase().as_str() {
        "base64" => Ok("base64"),
        "base58" => Ok("base58"),
        _ => Err(format!("encoding must be base64 or base58: {value}")),
    }
}

pub(crate) fn validate_token_amount(value: &str, label: &str, errors: &mut Vec<String>) {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        errors.push(format!("{label} must not be empty"));
        return;
    }
    if trimmed.len() != value.len() {
        errors.push(format!(
            "{label} must not contain leading or trailing whitespace"
        ));
    }
    if trimmed.len() > MAX_TOKEN_AMOUNT_LEN {
        errors.push(format!(
            "{label} must be at most {MAX_TOKEN_AMOUNT_LEN} digits"
        ));
    }
    if !trimmed.chars().all(|ch| ch.is_ascii_digit()) {
        errors.push(format!("{label} must be a positive integer string"));
    }
    if trimmed.chars().all(|ch| ch == '0') {
        errors.push(format!("{label} must be greater than zero"));
    }
}

pub(crate) fn validate_asset(
    asset: &EscrowAsset,
    request: &EscrowIntentRequest,
    errors: &mut Vec<String>,
    warnings: &mut Vec<String>,
) {
    if let Some(vault) = &asset.escrow_vault {
        if let Err(error) = validate_pubkey(vault, "asset.escrowVault") {
            errors.push(error);
        }
    }
    if let Some(collection) = &asset.collection {
        if let Err(error) = validate_pubkey(collection, "asset.collection") {
            errors.push(error);
        }
    }
    if let Some(decimals) = asset.decimals {
        if decimals > 12 {
            errors.push("asset.decimals must be at most 12".to_string());
        }
    }
    match asset.asset_type {
        AssetType::Sol => {
            match asset.amount_lamports {
                Some(amount) if amount > 0 => {}
                _ => errors.push(
                    "asset.amountLamports is required and must be greater than zero for SOL escrow"
                        .to_string(),
                ),
            }
            if asset.mint.is_some() {
                warnings.push("asset.mint is ignored for SOL escrow".to_string());
            }
        }
        AssetType::SplToken => {
            match &asset.mint {
                Some(mint) => {
                    if let Err(error) = validate_pubkey(mint, "asset.mint") {
                        errors.push(error);
                    }
                }
                None => errors.push("asset.mint is required for SPL token escrow".to_string()),
            }
            match &asset.token_amount {
                Some(amount) => validate_token_amount(amount, "asset.tokenAmount", errors),
                None => {
                    errors.push("asset.tokenAmount is required for SPL token escrow".to_string())
                }
            }
        }
        AssetType::Nft | AssetType::CompressedNft => {
            match &asset.mint {
                Some(mint) => {
                    if let Err(error) = validate_pubkey(mint, "asset.mint") {
                        errors.push(error);
                    }
                }
                None => errors.push("asset.mint is required for NFT escrow".to_string()),
            }
            if let Some(amount) = &asset.token_amount {
                let trimmed = amount.trim();
                if trimmed != "1" {
                    errors.push(
                        "asset.tokenAmount must be omitted or set to '1' for NFT escrow"
                            .to_string(),
                    );
                }
            }
        }
        AssetType::CustomProgram => {
            if request.settlement_plan.is_none() {
                errors.push(
                    "settlementPlan is required for custom-program escrow assets".to_string(),
                );
            }
        }
    }
}

pub(crate) fn validate_parties(
    request: &EscrowIntentRequest,
    spec: &KindSpec,
    errors: &mut Vec<String>,
    warnings: &mut Vec<String>,
) {
    if request.parties.len() < spec.min_parties {
        errors.push(format!(
            "{} escrow requires at least {} parties",
            spec.kind.as_str(),
            spec.min_parties
        ));
    }
    if request.parties.len() > MAX_PARTIES {
        errors.push(format!(
            "parties must include at most {MAX_PARTIES} entries"
        ));
    }
    let mut roles = HashSet::new();
    let mut labels = HashSet::new();
    let mut payout_sum: u32 = 0;
    let mut payout_count = 0;
    let mut required_signers = 0;
    for (index, party) in request.parties.iter().enumerate() {
        roles.insert(party.role);
        if let Err(error) = validate_pubkey(&party.pubkey, &format!("parties[{index}].pubkey")) {
            errors.push(error);
        }
        if let Some(label) = &party.label {
            validate_label(label, &format!("parties[{index}].label"), errors);
            if !labels.insert(label.trim().to_ascii_lowercase()) {
                errors.push(format!("parties[{index}].label must be unique"));
            }
        }
        if party.required_signer.unwrap_or(false) {
            required_signers += 1;
        }
        if let Some(payout_bps) = party.payout_bps {
            payout_count += 1;
            payout_sum += u32::from(payout_bps);
            if payout_bps > 10_000 {
                errors.push(format!("parties[{index}].payoutBps must be at most 10000"));
            }
        }
    }
    for role in &spec.required_roles {
        if !roles.contains(role) {
            errors.push(format!(
                "{} escrow requires a party with role {}",
                spec.kind.as_str(),
                role.as_str()
            ));
        }
    }
    if payout_count > 0 && payout_sum != 10_000 {
        errors.push(
            "party payoutBps values must sum to exactly 10000 when any payoutBps is provided"
                .to_string(),
        );
    }
    if required_signers == 0 {
        warnings.push("no parties are marked requiredSigner=true; settlement relies entirely on the submitted signed transaction".to_string());
    }
    if request.kind == EscrowKind::GroupBuy {
        let contributors = request
            .parties
            .iter()
            .filter(|party| party.role == PartyRole::Contributor)
            .count();
        if contributors < 2 {
            errors.push("group-buy escrow requires at least two contributor parties".to_string());
        }
    }
    if request.kind == EscrowKind::CollabShow {
        let creators: Vec<&EscrowParty> = request
            .parties
            .iter()
            .filter(|party| party.role == PartyRole::Creator)
            .collect();
        if creators.len() < 2 {
            errors.push("collab-show escrow requires at least two creator parties".to_string());
        }
        // The revenue split is the defining term of this product, so every creator
        // must carry payoutBps (the global check then enforces they sum to 10000).
        if creators.iter().any(|party| party.payout_bps.is_none()) {
            errors.push(
                "collab-show creator parties must each set payoutBps for the agreed revenue split"
                    .to_string(),
            );
        }
        // A neutral arbiter must not be a payout recipient in the revenue split.
        if request
            .parties
            .iter()
            .any(|party| party.role == PartyRole::Arbitrator && party.payout_bps.is_some())
        {
            errors.push(
                "collab-show arbiter must not carry payoutBps; the split is between creators"
                    .to_string(),
            );
        }
    }
}

pub(crate) fn validate_terms(
    request: &EscrowIntentRequest,
    spec: &KindSpec,
    errors: &mut Vec<String>,
    warnings: &mut Vec<String>,
) {
    if !spec.release_modes.contains(&request.terms.release_mode) {
        errors.push(format!(
            "{} escrow does not allow releaseMode {}",
            spec.kind.as_str(),
            request.terms.release_mode.as_str()
        ));
    }
    if let Some(actions) = &request.terms.settlement_actions {
        if actions.is_empty() {
            errors.push("terms.settlementActions must not be empty when provided".to_string());
        }
        for action in actions {
            if !spec.settlement_actions.contains(action) {
                errors.push(format!(
                    "{} escrow does not allow settlement action {}",
                    spec.kind.as_str(),
                    action.as_str()
                ));
            }
        }
    }
    if let Some(seconds) = request.terms.dispute_window_seconds {
        if seconds > MAX_DISPUTE_WINDOW_SECONDS {
            errors.push(format!(
                "terms.disputeWindowSeconds must be at most {MAX_DISPUTE_WINDOW_SECONDS}"
            ));
        }
    }
    if let Some(seconds) = request.terms.inspection_period_seconds {
        if seconds > MAX_INSPECTION_SECONDS {
            errors.push(format!(
                "terms.inspectionPeriodSeconds must be at most {MAX_INSPECTION_SECONDS}"
            ));
        }
    }
    if (matches!(
        request.terms.release_mode,
        ReleaseMode::TimeLocked | ReleaseMode::ExpiryRefund
    ) || request.kind == EscrowKind::SubscriptionRelease)
        && request.terms.timeout_unix_seconds.is_none()
    {
        errors.push(
            "terms.timeoutUnixSeconds is required for time-locked or expiry-refund escrow"
                .to_string(),
        );
    }
    if let Some(timeout) = request.terms.timeout_unix_seconds {
        if timeout <= now_unix_seconds() {
            errors.push("terms.timeoutUnixSeconds must be in the future".to_string());
        }
    }
    if request.kind == EscrowKind::CollabShow {
        // A collab show is inherently date-bound and breach-adjudicated, so pin both in
        // typed fields rather than opaque metadata: a future show deadline (so the escrow
        // can `expire`) and a contestation window for the arbiter to act within.
        if request.terms.timeout_unix_seconds.is_none() {
            errors.push(
                "collab-show escrow requires terms.timeoutUnixSeconds (the show date/deadline)"
                    .to_string(),
            );
        }
        if request.terms.dispute_window_seconds.unwrap_or(0) == 0 {
            errors.push(
                "collab-show escrow requires a non-zero terms.disputeWindowSeconds for breach adjudication"
                    .to_string(),
            );
        }
    }
    if request.terms.release_mode == ReleaseMode::MilestoneApproval {
        match &request.terms.milestones {
            Some(milestones) if !milestones.is_empty() => {}
            _ => errors
                .push("terms.milestones is required for milestone-approval escrow".to_string()),
        }
    }
    if let Some(max_partial) = request.terms.max_partial_releases {
        if usize::from(max_partial) > MAX_MILESTONES {
            errors.push(format!(
                "terms.maxPartialReleases must be at most {MAX_MILESTONES}"
            ));
        }
    }
    if let Some(approvals) = &request.terms.required_approvals {
        if approvals.is_empty() {
            errors.push("terms.requiredApprovals must not be empty when provided".to_string());
        }
        let party_roles: HashSet<PartyRole> =
            request.parties.iter().map(|party| party.role).collect();
        for role in approvals {
            if !party_roles.contains(role) {
                errors.push(format!(
                    "terms.requiredApprovals includes role {} but no party has that role",
                    role.as_str()
                ));
            }
        }
    }
    if request.kind == EscrowKind::DigitalDelivery && request.terms.delivery_required != Some(true)
    {
        warnings.push("digital-delivery escrow should set terms.deliveryRequired=true".to_string());
    }
    if request.kind == EscrowKind::OtcTrade
        && !matches!(
            request.asset.asset_type,
            AssetType::SplToken | AssetType::Nft | AssetType::CompressedNft
        )
    {
        warnings.push("otc-trade escrow usually uses an SPL token or NFT asset".to_string());
    }
}

pub(crate) fn validate_milestones(
    milestones: &Option<Vec<EscrowMilestone>>,
    errors: &mut Vec<String>,
) -> usize {
    let Some(milestones) = milestones else {
        return 0;
    };
    if milestones.len() > MAX_MILESTONES {
        errors.push(format!(
            "terms.milestones must include at most {MAX_MILESTONES} entries"
        ));
    }
    let mut ids = HashSet::new();
    let mut bps_sum = 0_u32;
    let mut bps_count = 0_usize;
    for (index, milestone) in milestones.iter().enumerate() {
        validate_token(
            &milestone.id,
            &format!("terms.milestones[{index}].id"),
            MAX_LABEL_LEN,
            errors,
        );
        if !ids.insert(milestone.id.trim().to_ascii_lowercase()) {
            errors.push(format!("terms.milestones[{index}].id must be unique"));
        }
        if let Some(label) = &milestone.label {
            validate_label(label, &format!("terms.milestones[{index}].label"), errors);
        }
        if let Some(amount_bps) = milestone.amount_bps {
            bps_count += 1;
            bps_sum += u32::from(amount_bps);
            if amount_bps > 10_000 {
                errors.push(format!(
                    "terms.milestones[{index}].amountBps must be at most 10000"
                ));
            }
        }
        if let Some(due) = milestone.due_unix_seconds {
            if due <= now_unix_seconds() {
                errors.push(format!(
                    "terms.milestones[{index}].dueUnixSeconds must be in the future"
                ));
            }
        }
    }
    if bps_count > 0 && bps_count == milestones.len() && bps_sum != 10_000 {
        errors.push("terms.milestones amountBps values must sum to exactly 10000 when every milestone has amountBps".to_string());
    }
    milestones.len()
}

pub(crate) fn validate_settlement_plan(
    plan: &Option<SettlementPlan>,
    allowed_program_ids: &[String],
    errors: &mut Vec<String>,
) {
    let Some(plan) = plan else {
        return;
    };
    if let Err(error) = validate_pubkey(&plan.program_id, "settlementPlan.programId") {
        errors.push(error);
    }
    if !allowed_program_ids.is_empty()
        && !allowed_program_ids
            .iter()
            .any(|program_id| program_id == &plan.program_id)
    {
        errors.push("settlementPlan.programId is not in ESCROW_ALLOWED_PROGRAM_IDS".to_string());
    }
    if let Some(vault) = &plan.vault_pubkey {
        if let Err(error) = validate_pubkey(vault, "settlementPlan.vaultPubkey") {
            errors.push(error);
        }
    }
    if let Some(fee_bps) = plan.fee_bps {
        if fee_bps > 1000 {
            errors.push("settlementPlan.feeBps must be at most 1000".to_string());
        }
    }
}

pub(crate) fn required_signer_count(request: &EscrowIntentRequest) -> usize {
    request
        .parties
        .iter()
        .filter(|party| party.required_signer.unwrap_or(false))
        .count()
}

pub(crate) fn required_approval_count(request: &EscrowIntentRequest) -> usize {
    request
        .terms
        .required_approvals
        .as_ref()
        .map(Vec::len)
        .unwrap_or(0)
}

pub(crate) fn configured_settlement_actions(request: &EscrowIntentRequest) -> usize {
    request
        .terms
        .settlement_actions
        .as_ref()
        .map(Vec::len)
        .unwrap_or(0)
}

pub(crate) fn policy_checks(request: &EscrowIntentRequest, spec: &KindSpec) -> Vec<EscrowPolicyCheck> {
    let signer_count = required_signer_count(request);
    let approval_count = required_approval_count(request);
    let has_dispute_window = request.terms.dispute_window_seconds.unwrap_or(0) > 0;
    let has_timeout = request.terms.timeout_unix_seconds.is_some();
    let has_settlement_plan = request.settlement_plan.is_some();
    let has_action_list = configured_settlement_actions(request) > 0;
    let has_required_roles = spec
        .required_roles
        .iter()
        .all(|role| request.parties.iter().any(|party| &party.role == role));
    vec![
        EscrowPolicyCheck {
            name: "required-roles",
            ok: has_required_roles,
            severity: "error",
            detail: format!("{} requires {:?}", spec.kind.as_str(), spec.required_roles),
        },
        EscrowPolicyCheck {
            name: "required-signers",
            ok: signer_count > 0,
            severity: "warn",
            detail: format!("{signer_count} party record(s) are marked requiredSigner=true"),
        },
        EscrowPolicyCheck {
            name: "approval-policy",
            ok: approval_count > 0
                || matches!(
                    request.terms.release_mode,
                    ReleaseMode::MultiSig | ReleaseMode::ArbiterDecision
                ),
            severity: "warn",
            detail: format!("{approval_count} explicit approval role(s) configured"),
        },
        EscrowPolicyCheck {
            name: "settlement-actions",
            ok: has_action_list,
            severity: "warn",
            detail: format!(
                "{} settlement action(s) explicitly configured",
                configured_settlement_actions(request)
            ),
        },
        EscrowPolicyCheck {
            name: "settlement-plan",
            ok: has_settlement_plan,
            severity: "warn",
            detail: if has_settlement_plan {
                "settlementPlan is present for on-chain readiness".to_string()
            } else {
                "settlementPlan is missing; validation can pass but live settlement will require stronger evidence".to_string()
            },
        },
        EscrowPolicyCheck {
            name: "dispute-window",
            ok: has_dispute_window
                || !matches!(request.terms.release_mode, ReleaseMode::ArbiterDecision),
            severity: "warn",
            detail: if has_dispute_window {
                "disputeWindowSeconds is configured".to_string()
            } else {
                "no disputeWindowSeconds configured".to_string()
            },
        },
        EscrowPolicyCheck {
            name: "timeout",
            ok: has_timeout
                || !matches!(
                    request.terms.release_mode,
                    ReleaseMode::TimeLocked | ReleaseMode::ExpiryRefund
                ),
            severity: "warn",
            detail: if has_timeout {
                "timeoutUnixSeconds is configured".to_string()
            } else {
                "no timeoutUnixSeconds configured".to_string()
            },
        },
    ]
}

pub(crate) fn readiness_for(
    request: &EscrowIntentRequest,
    on_chain_settlement_ready: bool,
) -> EscrowReadiness {
    let signer_count = required_signer_count(request);
    let approval_count = required_approval_count(request);
    let mut score = 10_u8;
    if on_chain_settlement_ready {
        score = score.saturating_add(25);
    }
    if signer_count > 0 {
        score = score.saturating_add(20);
    }
    if approval_count > 0 {
        score = score.saturating_add(15);
    }
    if request.terms.dispute_window_seconds.unwrap_or(0) > 0 {
        score = score.saturating_add(10);
    }
    if request.terms.timeout_unix_seconds.is_some() {
        score = score.saturating_add(10);
    }
    if configured_settlement_actions(request) > 0 {
        score = score.saturating_add(10);
    }
    let risk_tier = if score >= 80 {
        "low"
    } else if score >= 55 {
        "medium"
    } else {
        "high"
    };
    let mut recommended_next_actions = Vec::new();
    if !on_chain_settlement_ready {
        recommended_next_actions.push("attach-settlement-plan");
    }
    if signer_count == 0 {
        recommended_next_actions.push("mark-required-signers");
    }
    if approval_count == 0 {
        recommended_next_actions.push("configure-required-approvals");
    }
    if request.terms.dispute_window_seconds.unwrap_or(0) == 0 {
        recommended_next_actions.push("set-dispute-window");
    }
    if recommended_next_actions.is_empty() {
        recommended_next_actions.push("simulate-settlement");
    }
    EscrowReadiness {
        risk_tier,
        risk_score: score.min(100),
        required_signer_count: signer_count,
        required_approval_count: approval_count,
        on_chain_settlement_ready,
        recommended_next_actions,
    }
}

pub(crate) fn validate_memo_and_metadata(request: &EscrowIntentRequest, errors: &mut Vec<String>) {
    if let Some(memo) = &request.memo {
        if memo.as_bytes().len() > MAX_MEMO_BYTES {
            errors.push(format!("memo must be at most {MAX_MEMO_BYTES} bytes"));
        }
    }
    if let Some(metadata) = &request.metadata {
        match serde_json::to_vec(metadata) {
            Ok(encoded) if encoded.len() <= MAX_METADATA_BYTES => {}
            Ok(encoded) => errors.push(format!(
                "metadata must serialize to at most {MAX_METADATA_BYTES} bytes, got {}",
                encoded.len()
            )),
            Err(error) => errors.push(format!("metadata could not be serialized: {error}")),
        }
    }
}

pub(crate) fn escrow_digest(request: &EscrowIntentRequest) -> String {
    let canonical = serde_json::to_vec(request).unwrap_or_default();
    let digest = Sha256::digest(canonical);
    format!("solana-escrow:{}", hex::encode(&digest[..16]))
}

pub(crate) fn transaction_digest(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    format!("solana-tx:{}", hex::encode(&digest[..16]))
}

pub(crate) fn validate_escrow_intent(
    request: &EscrowIntentRequest,
    default_cluster: &str,
    allowed_program_ids: &[String],
) -> Result<EscrowValidationResponse, Vec<String>> {
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
    validate_parties(request, &spec, &mut errors, &mut warnings);
    validate_asset(&request.asset, request, &mut errors, &mut warnings);
    validate_terms(request, &spec, &mut errors, &mut warnings);
    let milestone_count = validate_milestones(&request.terms.milestones, &mut errors);
    validate_settlement_plan(&request.settlement_plan, allowed_program_ids, &mut errors);
    validate_memo_and_metadata(request, &mut errors);
    if !errors.is_empty() {
        return Err(errors);
    }
    let on_chain_settlement_ready = request.settlement_plan.is_some();
    let readiness = readiness_for(request, on_chain_settlement_ready);
    let checks = policy_checks(request, &spec);
    Ok(EscrowValidationResponse {
        ok: true,
        request_id: request_id(request.request_id.as_ref(), "escrow-validation"),
        schema_version: SCHEMA_VERSION,
        cluster,
        escrow_id: request.escrow_id.clone(),
        kind: request.kind,
        asset_type: request.asset.asset_type,
        release_mode: request.terms.release_mode,
        party_count: request.parties.len(),
        milestone_count,
        required_roles: spec
            .required_roles
            .iter()
            .copied()
            .map(PartyRole::as_str)
            .collect(),
        allowed_settlement_actions: spec
            .settlement_actions
            .iter()
            .copied()
            .map(SettlementAction::as_str)
            .collect(),
        on_chain_settlement_ready,
        readiness,
        checks,
        digest: escrow_digest(request),
        warnings,
        generated_at_ms: now_ms(),
    })
}
