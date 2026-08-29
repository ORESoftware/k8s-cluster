use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::domain::{
    AssetType, EscrowKind, PartyRole, ReleaseMode, ResolutionOutcome, SettlementAction,
};

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EscrowIntentRequest {
    pub(crate) schema_version: String,
    pub(crate) request_id: Option<String>,
    pub(crate) cluster: Option<String>,
    pub(crate) kind: EscrowKind,
    pub(crate) escrow_id: String,
    pub(crate) parties: Vec<EscrowParty>,
    pub(crate) asset: EscrowAsset,
    pub(crate) terms: EscrowTerms,
    pub(crate) settlement_plan: Option<SettlementPlan>,
    pub(crate) memo: Option<String>,
    pub(crate) metadata: Option<Value>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EscrowParty {
    pub(crate) role: PartyRole,
    pub(crate) pubkey: String,
    pub(crate) label: Option<String>,
    pub(crate) required_signer: Option<bool>,
    pub(crate) payout_bps: Option<u16>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EscrowAsset {
    pub(crate) asset_type: AssetType,
    pub(crate) mint: Option<String>,
    pub(crate) amount_lamports: Option<u64>,
    pub(crate) token_amount: Option<String>,
    pub(crate) decimals: Option<u8>,
    pub(crate) collection: Option<String>,
    pub(crate) escrow_vault: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EscrowTerms {
    pub(crate) release_mode: ReleaseMode,
    pub(crate) settlement_actions: Option<Vec<SettlementAction>>,
    pub(crate) dispute_window_seconds: Option<u64>,
    pub(crate) inspection_period_seconds: Option<u64>,
    pub(crate) timeout_unix_seconds: Option<u64>,
    pub(crate) milestones: Option<Vec<EscrowMilestone>>,
    pub(crate) required_approvals: Option<Vec<PartyRole>>,
    pub(crate) max_partial_releases: Option<u8>,
    pub(crate) delivery_required: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EscrowMilestone {
    pub(crate) id: String,
    pub(crate) label: Option<String>,
    pub(crate) amount_bps: Option<u16>,
    pub(crate) due_unix_seconds: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SettlementPlan {
    pub(crate) program_id: String,
    pub(crate) vault_pubkey: Option<String>,
    pub(crate) fee_bps: Option<u16>,
    pub(crate) memo_required: Option<bool>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EscrowValidationResponse {
    pub(crate) ok: bool,
    pub(crate) request_id: String,
    pub(crate) schema_version: &'static str,
    pub(crate) cluster: String,
    pub(crate) escrow_id: String,
    pub(crate) kind: EscrowKind,
    pub(crate) asset_type: AssetType,
    pub(crate) release_mode: ReleaseMode,
    pub(crate) party_count: usize,
    pub(crate) milestone_count: usize,
    pub(crate) required_roles: Vec<&'static str>,
    pub(crate) allowed_settlement_actions: Vec<&'static str>,
    pub(crate) on_chain_settlement_ready: bool,
    pub(crate) readiness: EscrowReadiness,
    pub(crate) checks: Vec<EscrowPolicyCheck>,
    pub(crate) digest: String,
    pub(crate) warnings: Vec<String>,
    pub(crate) generated_at_ms: u128,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EscrowReadiness {
    pub(crate) risk_tier: &'static str,
    pub(crate) risk_score: u8,
    pub(crate) required_signer_count: usize,
    pub(crate) required_approval_count: usize,
    pub(crate) on_chain_settlement_ready: bool,
    pub(crate) recommended_next_actions: Vec<&'static str>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EscrowPolicyCheck {
    pub(crate) name: &'static str,
    pub(crate) ok: bool,
    pub(crate) severity: &'static str,
    pub(crate) detail: String,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EscrowAuditResponse {
    pub(crate) ok: bool,
    pub(crate) request_id: String,
    pub(crate) schema_version: &'static str,
    pub(crate) cluster: String,
    pub(crate) escrow_id: String,
    pub(crate) kind: EscrowKind,
    pub(crate) validation: Option<EscrowValidationResponse>,
    pub(crate) errors: Vec<String>,
    pub(crate) warnings: Vec<String>,
    pub(crate) generated_at_ms: u128,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EscrowSettlementRequest {
    pub(crate) schema_version: String,
    pub(crate) request_id: Option<String>,
    pub(crate) cluster: Option<String>,
    pub(crate) kind: EscrowKind,
    pub(crate) escrow_id: String,
    pub(crate) action: SettlementAction,
    pub(crate) transaction: String,
    pub(crate) encoding: Option<String>,
    pub(crate) commitment: Option<String>,
    pub(crate) skip_preflight: Option<bool>,
    pub(crate) max_retries: Option<usize>,
    pub(crate) min_context_slot: Option<u64>,
    pub(crate) intent: Option<EscrowIntentRequest>,
    pub(crate) resolution: Option<EscrowResolution>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EscrowResolution {
    pub(crate) outcome: ResolutionOutcome,
    pub(crate) winner_role: Option<PartyRole>,
    pub(crate) refund_role: Option<PartyRole>,
    pub(crate) allocations: Option<Vec<ResolutionAllocation>>,
    pub(crate) rationale: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ResolutionAllocation {
    pub(crate) role: PartyRole,
    pub(crate) pubkey: Option<String>,
    pub(crate) payout_bps: u16,
}

/// Standalone body for `POST /resolve`: validate an intent plus its proposed resolution
/// without touching Solana.
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ResolutionRequest {
    pub(crate) schema_version: String,
    pub(crate) request_id: Option<String>,
    pub(crate) cluster: Option<String>,
    pub(crate) action: SettlementAction,
    pub(crate) intent: EscrowIntentRequest,
    pub(crate) resolution: EscrowResolution,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ResolutionResponse {
    pub(crate) ok: bool,
    pub(crate) request_id: String,
    pub(crate) schema_version: &'static str,
    pub(crate) cluster: String,
    pub(crate) escrow_id: String,
    pub(crate) kind: EscrowKind,
    pub(crate) action: SettlementAction,
    pub(crate) outcome: &'static str,
    pub(crate) errors: Vec<String>,
    pub(crate) warnings: Vec<String>,
    pub(crate) generated_at_ms: u128,
}

#[derive(Debug)]
pub(crate) struct ValidatedSettlement {
    pub(crate) request_id: String,
    pub(crate) cluster: String,
    pub(crate) transaction_bytes: Vec<u8>,
    pub(crate) transaction_digest: String,
    pub(crate) warnings: Vec<String>,
}
