use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum EscrowKind {
    MarketplaceOrder,
    Milestone,
    FreelanceContract,
    DigitalDelivery,
    OtcTrade,
    RentalDeposit,
    Bounty,
    SubscriptionRelease,
    GroupBuy,
    DisputeResolution,
    CollabShow,
}

impl EscrowKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            EscrowKind::MarketplaceOrder => "marketplace-order",
            EscrowKind::Milestone => "milestone",
            EscrowKind::FreelanceContract => "freelance-contract",
            EscrowKind::DigitalDelivery => "digital-delivery",
            EscrowKind::OtcTrade => "otc-trade",
            EscrowKind::RentalDeposit => "rental-deposit",
            EscrowKind::Bounty => "bounty",
            EscrowKind::SubscriptionRelease => "subscription-release",
            EscrowKind::GroupBuy => "group-buy",
            EscrowKind::DisputeResolution => "dispute-resolution",
            EscrowKind::CollabShow => "collab-show",
        }
    }
}

pub(crate) const ESCROW_KINDS: [EscrowKind; 11] = [
    EscrowKind::MarketplaceOrder,
    EscrowKind::Milestone,
    EscrowKind::FreelanceContract,
    EscrowKind::DigitalDelivery,
    EscrowKind::OtcTrade,
    EscrowKind::RentalDeposit,
    EscrowKind::Bounty,
    EscrowKind::SubscriptionRelease,
    EscrowKind::GroupBuy,
    EscrowKind::DisputeResolution,
    EscrowKind::CollabShow,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum PartyRole {
    Buyer,
    Seller,
    Payer,
    Payee,
    Client,
    Contractor,
    Depositor,
    Recipient,
    Arbitrator,
    Broker,
    Platform,
    Contributor,
    Maintainer,
    Fulfiller,
    Landlord,
    Tenant,
    Creator,
}

impl PartyRole {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            PartyRole::Buyer => "buyer",
            PartyRole::Seller => "seller",
            PartyRole::Payer => "payer",
            PartyRole::Payee => "payee",
            PartyRole::Client => "client",
            PartyRole::Contractor => "contractor",
            PartyRole::Depositor => "depositor",
            PartyRole::Recipient => "recipient",
            PartyRole::Arbitrator => "arbitrator",
            PartyRole::Broker => "broker",
            PartyRole::Platform => "platform",
            PartyRole::Contributor => "contributor",
            PartyRole::Maintainer => "maintainer",
            PartyRole::Fulfiller => "fulfiller",
            PartyRole::Landlord => "landlord",
            PartyRole::Tenant => "tenant",
            PartyRole::Creator => "creator",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum AssetType {
    Sol,
    SplToken,
    Nft,
    CompressedNft,
    CustomProgram,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ReleaseMode {
    BuyerApproval,
    MilestoneApproval,
    TimeLocked,
    OracleSignal,
    ArbiterDecision,
    MultiSig,
    DeliveryProof,
    ExpiryRefund,
    ManualOperator,
}

impl ReleaseMode {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            ReleaseMode::BuyerApproval => "buyer-approval",
            ReleaseMode::MilestoneApproval => "milestone-approval",
            ReleaseMode::TimeLocked => "time-locked",
            ReleaseMode::OracleSignal => "oracle-signal",
            ReleaseMode::ArbiterDecision => "arbiter-decision",
            ReleaseMode::MultiSig => "multi-sig",
            ReleaseMode::DeliveryProof => "delivery-proof",
            ReleaseMode::ExpiryRefund => "expiry-refund",
            ReleaseMode::ManualOperator => "manual-operator",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum SettlementAction {
    Fund,
    Release,
    Refund,
    PartialRelease,
    SplitRelease,
    DisputeAward,
    Expire,
    Cancel,
}

impl SettlementAction {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            SettlementAction::Fund => "fund",
            SettlementAction::Release => "release",
            SettlementAction::Refund => "refund",
            SettlementAction::PartialRelease => "partial-release",
            SettlementAction::SplitRelease => "split-release",
            SettlementAction::DisputeAward => "dispute-award",
            SettlementAction::Expire => "expire",
            SettlementAction::Cancel => "cancel",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ResolutionOutcome {
    Release,
    Refund,
    Split,
    DisputeAward,
    Expire,
    Cancel,
}

impl ResolutionOutcome {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            ResolutionOutcome::Release => "release",
            ResolutionOutcome::Refund => "refund",
            ResolutionOutcome::Split => "split",
            ResolutionOutcome::DisputeAward => "dispute-award",
            ResolutionOutcome::Expire => "expire",
            ResolutionOutcome::Cancel => "cancel",
        }
    }

    /// The settlement action that an outcome maps onto. `Split` is satisfied by either
    /// `SplitRelease` or `PartialRelease`, so it returns the canonical `SplitRelease`.
    pub(crate) fn expected_action(self) -> SettlementAction {
        match self {
            ResolutionOutcome::Release => SettlementAction::Release,
            ResolutionOutcome::Refund => SettlementAction::Refund,
            ResolutionOutcome::Split => SettlementAction::SplitRelease,
            ResolutionOutcome::DisputeAward => SettlementAction::DisputeAward,
            ResolutionOutcome::Expire => SettlementAction::Expire,
            ResolutionOutcome::Cancel => SettlementAction::Cancel,
        }
    }

    pub(crate) fn matches_action(self, action: SettlementAction) -> bool {
        match self {
            ResolutionOutcome::Split => matches!(
                action,
                SettlementAction::SplitRelease | SettlementAction::PartialRelease
            ),
            other => action == other.expected_action(),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct KindSpec {
    pub(crate) kind: EscrowKind,
    pub(crate) description: &'static str,
    pub(crate) min_parties: usize,
    pub(crate) required_roles: Vec<PartyRole>,
    pub(crate) release_modes: Vec<ReleaseMode>,
    pub(crate) settlement_actions: Vec<SettlementAction>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct KindCatalogEntry {
    pub(crate) kind: &'static str,
    pub(crate) description: &'static str,
    pub(crate) min_parties: usize,
    pub(crate) required_roles: Vec<&'static str>,
    pub(crate) release_modes: Vec<&'static str>,
    pub(crate) settlement_actions: Vec<&'static str>,
}

pub(crate) fn kind_spec(kind: EscrowKind) -> KindSpec {
    match kind {
        EscrowKind::MarketplaceOrder => KindSpec {
            kind,
            description: "Buyer/seller order escrow with approval, delivery proof, refund, or dispute settlement.",
            min_parties: 2,
            required_roles: vec![PartyRole::Buyer, PartyRole::Seller],
            release_modes: vec![
                ReleaseMode::BuyerApproval,
                ReleaseMode::DeliveryProof,
                ReleaseMode::ExpiryRefund,
                ReleaseMode::ArbiterDecision,
            ],
            settlement_actions: vec![
                SettlementAction::Fund,
                SettlementAction::Release,
                SettlementAction::Refund,
                SettlementAction::DisputeAward,
                SettlementAction::Expire,
                SettlementAction::Cancel,
            ],
        },
        EscrowKind::Milestone => KindSpec {
            kind,
            description: "Milestone escrow that can release partial payouts as approved work checkpoints complete.",
            min_parties: 2,
            required_roles: vec![PartyRole::Payer, PartyRole::Payee],
            release_modes: vec![
                ReleaseMode::MilestoneApproval,
                ReleaseMode::MultiSig,
                ReleaseMode::ArbiterDecision,
            ],
            settlement_actions: vec![
                SettlementAction::Fund,
                SettlementAction::PartialRelease,
                SettlementAction::Release,
                SettlementAction::Refund,
                SettlementAction::DisputeAward,
            ],
        },
        EscrowKind::FreelanceContract => KindSpec {
            kind,
            description: "Client/contractor escrow for scoped services, milestones, inspection, and dispute awards.",
            min_parties: 2,
            required_roles: vec![PartyRole::Client, PartyRole::Contractor],
            release_modes: vec![
                ReleaseMode::MilestoneApproval,
                ReleaseMode::BuyerApproval,
                ReleaseMode::ArbiterDecision,
            ],
            settlement_actions: vec![
                SettlementAction::Fund,
                SettlementAction::PartialRelease,
                SettlementAction::Release,
                SettlementAction::Refund,
                SettlementAction::DisputeAward,
                SettlementAction::Cancel,
            ],
        },
        EscrowKind::DigitalDelivery => KindSpec {
            kind,
            description: "Digital goods escrow that prefers delivery proof plus an inspection window before release.",
            min_parties: 2,
            required_roles: vec![PartyRole::Buyer, PartyRole::Seller],
            release_modes: vec![
                ReleaseMode::DeliveryProof,
                ReleaseMode::BuyerApproval,
                ReleaseMode::TimeLocked,
            ],
            settlement_actions: vec![
                SettlementAction::Fund,
                SettlementAction::Release,
                SettlementAction::Refund,
                SettlementAction::Expire,
                SettlementAction::Cancel,
            ],
        },
        EscrowKind::OtcTrade => KindSpec {
            kind,
            description: "OTC token/NFT trade escrow for brokered or multi-signature settlement.",
            min_parties: 2,
            required_roles: vec![PartyRole::Buyer, PartyRole::Seller],
            release_modes: vec![ReleaseMode::MultiSig, ReleaseMode::ArbiterDecision],
            settlement_actions: vec![
                SettlementAction::Fund,
                SettlementAction::Release,
                SettlementAction::Refund,
                SettlementAction::SplitRelease,
                SettlementAction::DisputeAward,
            ],
        },
        EscrowKind::RentalDeposit => KindSpec {
            kind,
            description: "Rental deposit escrow with time locks, inspection windows, refund, and damage awards.",
            min_parties: 2,
            required_roles: vec![PartyRole::Landlord, PartyRole::Tenant],
            release_modes: vec![
                ReleaseMode::TimeLocked,
                ReleaseMode::ExpiryRefund,
                ReleaseMode::ArbiterDecision,
            ],
            settlement_actions: vec![
                SettlementAction::Fund,
                SettlementAction::Refund,
                SettlementAction::SplitRelease,
                SettlementAction::DisputeAward,
                SettlementAction::Expire,
            ],
        },
        EscrowKind::Bounty => KindSpec {
            kind,
            description: "Bounty escrow for a payer and fulfiller, optionally reviewed by a maintainer.",
            min_parties: 2,
            required_roles: vec![PartyRole::Payer, PartyRole::Fulfiller],
            release_modes: vec![
                ReleaseMode::BuyerApproval,
                ReleaseMode::MilestoneApproval,
                ReleaseMode::ArbiterDecision,
            ],
            settlement_actions: vec![
                SettlementAction::Fund,
                SettlementAction::Release,
                SettlementAction::Refund,
                SettlementAction::PartialRelease,
                SettlementAction::DisputeAward,
                SettlementAction::Cancel,
            ],
        },
        EscrowKind::SubscriptionRelease => KindSpec {
            kind,
            description: "Recurring or streaming escrow for scheduled releases with optional oracle or operator approval.",
            min_parties: 2,
            required_roles: vec![PartyRole::Payer, PartyRole::Payee],
            release_modes: vec![
                ReleaseMode::TimeLocked,
                ReleaseMode::OracleSignal,
                ReleaseMode::ManualOperator,
            ],
            settlement_actions: vec![
                SettlementAction::Fund,
                SettlementAction::PartialRelease,
                SettlementAction::Release,
                SettlementAction::Refund,
                SettlementAction::Expire,
                SettlementAction::Cancel,
            ],
        },
        EscrowKind::GroupBuy => KindSpec {
            kind,
            description: "Group-buy escrow with multiple contributors and a seller or broker before final release/refund.",
            min_parties: 3,
            required_roles: vec![PartyRole::Contributor, PartyRole::Seller],
            release_modes: vec![
                ReleaseMode::MultiSig,
                ReleaseMode::TimeLocked,
                ReleaseMode::ManualOperator,
            ],
            settlement_actions: vec![
                SettlementAction::Fund,
                SettlementAction::Release,
                SettlementAction::Refund,
                SettlementAction::SplitRelease,
                SettlementAction::Expire,
                SettlementAction::Cancel,
            ],
        },
        EscrowKind::DisputeResolution => KindSpec {
            kind,
            description: "Dispute-first escrow that requires an arbitrator and settles by refund, split, or award.",
            min_parties: 3,
            required_roles: vec![PartyRole::Payer, PartyRole::Payee, PartyRole::Arbitrator],
            release_modes: vec![ReleaseMode::ArbiterDecision, ReleaseMode::MultiSig],
            settlement_actions: vec![
                SettlementAction::Fund,
                SettlementAction::Refund,
                SettlementAction::SplitRelease,
                SettlementAction::DisputeAward,
                SettlementAction::Cancel,
            ],
        },
        EscrowKind::CollabShow => KindSpec {
            kind,
            description: "Two-creator collaboration/show escrow: each creator funds a commitment stake plus a shared pool, split by revenue-share payoutBps on success, with a required arbiter awarding or splitting funds on a no-show or rule violation.",
            min_parties: 3,
            required_roles: vec![PartyRole::Creator, PartyRole::Arbitrator],
            release_modes: vec![
                ReleaseMode::ArbiterDecision,
                ReleaseMode::MultiSig,
                ReleaseMode::TimeLocked,
                ReleaseMode::DeliveryProof,
            ],
            settlement_actions: vec![
                SettlementAction::Fund,
                SettlementAction::SplitRelease,
                SettlementAction::Release,
                SettlementAction::Refund,
                SettlementAction::DisputeAward,
                SettlementAction::Expire,
                SettlementAction::Cancel,
            ],
        },
    }
}

pub(crate) fn kind_catalog() -> Vec<KindCatalogEntry> {
    ESCROW_KINDS
        .iter()
        .copied()
        .map(|kind| {
            let spec = kind_spec(kind);
            KindCatalogEntry {
                kind: spec.kind.as_str(),
                description: spec.description,
                min_parties: spec.min_parties,
                required_roles: spec
                    .required_roles
                    .iter()
                    .copied()
                    .map(PartyRole::as_str)
                    .collect(),
                release_modes: spec
                    .release_modes
                    .iter()
                    .copied()
                    .map(ReleaseMode::as_str)
                    .collect(),
                settlement_actions: spec
                    .settlement_actions
                    .iter()
                    .copied()
                    .map(SettlementAction::as_str)
                    .collect(),
            }
        })
        .collect()
}
