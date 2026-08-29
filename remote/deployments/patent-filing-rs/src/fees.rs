use serde::{Deserialize, Serialize};

use crate::{FEE_EFFECTIVE_DATE, MAX_CLAIMS};

// ---------------------------------------------------------------------------
// USPTO fee estimation
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Entity {
    Large,
    Small,
    Micro,
}

impl Entity {
    pub(crate) fn parse(value: Option<&str>) -> Entity {
        match value.map(|item| item.trim().to_ascii_lowercase()).as_deref() {
            Some("small") => Entity::Small,
            Some("micro") => Entity::Micro,
            _ => Entity::Large,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Entity::Large => "large",
            Entity::Small => "small",
            Entity::Micro => "micro",
        }
    }

    /// Scale an undiscounted (large-entity) fee. Small = 40%, micro = 20%;
    /// the 2025 schedule values are exact multiples so integer math is exact.
    fn scale(self, large_cents: u64) -> u64 {
        match self {
            Entity::Large => large_cents,
            Entity::Small => large_cents * 2 / 5,
            Entity::Micro => large_cents / 5,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FeeLineItem {
    pub(crate) code: String,
    pub(crate) label: String,
    pub(crate) unit_usd: f64,
    pub(crate) quantity: u64,
    pub(crate) amount_usd: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FeeEstimate {
    pub(crate) entity: String,
    pub(crate) filing_track: String,
    pub(crate) currency: &'static str,
    pub(crate) effective_date: &'static str,
    pub(crate) line_items: Vec<FeeLineItem>,
    pub(crate) total_usd: f64,
    pub(crate) disclaimer: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FeeEstimateRequest {
    pub(crate) entity_status: Option<String>,
    pub(crate) filing_track: Option<String>,
    pub(crate) total_claims: Option<usize>,
    pub(crate) independent_claims: Option<usize>,
    pub(crate) has_multiple_dependent_claim: Option<bool>,
}

/// Large-entity (undiscounted) USPTO fee amounts in whole US dollars, effective
/// 2025-01-19. Source: USPTO fee schedule.
fn fee_line(
    entity: Entity,
    code: &str,
    label: &str,
    large_usd: u64,
    quantity: u64,
) -> Option<FeeLineItem> {
    if quantity == 0 {
        return None;
    }
    let unit = entity.scale(large_usd) as f64;
    Some(FeeLineItem {
        code: code.to_string(),
        label: label.to_string(),
        unit_usd: unit,
        quantity,
        amount_usd: unit * quantity as f64,
    })
}

pub(crate) fn estimate_fees(
    entity: Entity,
    track: &str,
    total_claims: usize,
    independent_claims: usize,
    has_multiple_dependent_claim: bool,
) -> FeeEstimate {
    let total_claims = total_claims.min(MAX_CLAIMS);
    let independent_claims = independent_claims.min(total_claims.max(1)).max(1);
    let mut items = Vec::new();

    if track == "provisional" {
        items.extend(fee_line(
            entity,
            "provisional-filing",
            "Provisional application filing fee",
            325,
            1,
        ));
    } else {
        items.extend(fee_line(
            entity,
            "basic-filing",
            "Utility nonprovisional basic filing fee",
            350,
            1,
        ));
        items.extend(fee_line(entity, "search", "Utility search fee", 770, 1));
        items.extend(fee_line(
            entity,
            "examination",
            "Utility examination fee",
            880,
            1,
        ));
        let excess_independent = independent_claims.saturating_sub(3) as u64;
        items.extend(fee_line(
            entity,
            "excess-independent-claims",
            "Each independent claim in excess of 3",
            600,
            excess_independent,
        ));
        let excess_total = total_claims.saturating_sub(20) as u64;
        items.extend(fee_line(
            entity,
            "excess-claims",
            "Each claim in excess of 20",
            200,
            excess_total,
        ));
        if has_multiple_dependent_claim {
            items.extend(fee_line(
                entity,
                "multiple-dependent-claim",
                "Multiple dependent claim fee (per application)",
                925,
                1,
            ));
        }
    }

    let total_usd = items.iter().map(|item| item.amount_usd).sum();
    FeeEstimate {
        entity: entity.label().to_string(),
        filing_track: track.to_string(),
        currency: "USD",
        effective_date: FEE_EFFECTIVE_DATE,
        line_items: items,
        total_usd,
        disclaimer:
            "Estimate of standard USPTO fees only (effective 2025-01-19). Excludes attorney fees, \
             extensions, petitions, IDS, issue/maintenance, and any fee changes after the effective date."
                .to_string(),
    }
}
