//! Sharding key derivation.
//!
//! Every tenant-scoped row carries a `shard_key: i64` derived from
//! `(tenant_id, region)`. For the MVP we run a single physical Postgres, but
//! the shard key is computed and stored from day 1, so adding partitions or
//! splitting tenants across databases later requires no schema change — only
//! a routing-layer change.
//!
//! Region is intentionally a regulatory boundary (country + US state) rather
//! than purely a load-balancing hash, because data residency requirements
//! (GDPR, CCPA, state money-transmission rules) often demand that a tenant's
//! ledger never leaves a given jurisdiction.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum Region {
    /// US tenant; state is the regulatory residency state (often the state of
    /// incorporation, not the tenant's mailing address).
    #[serde(rename = "us")]
    Us { state: [u8; 2] },
    /// EU tenant. country_code is the ISO-3166-1 alpha-2 of the EU member.
    #[serde(rename = "eu")]
    Eu { country: [u8; 2] },
    /// Anywhere else.
    #[serde(rename = "other")]
    Other { country: [u8; 2] },
}

impl Region {
    pub fn from_codes(country: &str, us_state: Option<&str>) -> anyhow::Result<Self> {
        let cc = parse_cc(country)?;
        if cc == *b"US" {
            let state = us_state
                .ok_or_else(|| anyhow::anyhow!("us_state is required when country is US"))?;
            return Ok(Self::Us { state: parse_cc(state)? });
        }
        const EU: &[[u8; 2]] = &[
            *b"AT", *b"BE", *b"BG", *b"HR", *b"CY", *b"CZ", *b"DK", *b"EE",
            *b"FI", *b"FR", *b"DE", *b"GR", *b"HU", *b"IE", *b"IT", *b"LV",
            *b"LT", *b"LU", *b"MT", *b"NL", *b"PL", *b"PT", *b"RO", *b"SK",
            *b"SI", *b"ES", *b"SE",
        ];
        if EU.contains(&cc) {
            Ok(Self::Eu { country: cc })
        } else {
            Ok(Self::Other { country: cc })
        }
    }

    pub fn tag(&self) -> &'static str {
        match self {
            Self::Us { .. } => "us",
            Self::Eu { .. } => "eu",
            Self::Other { .. } => "ot",
        }
    }
}

fn parse_cc(s: &str) -> anyhow::Result<[u8; 2]> {
    if s.len() != 2 {
        anyhow::bail!("country/state code must be 2 chars: {s:?}");
    }
    let b = s.as_bytes();
    if !b.iter().all(|c| c.is_ascii_alphabetic()) {
        anyhow::bail!("country/state code must be alphabetic: {s:?}");
    }
    Ok([b[0].to_ascii_uppercase(), b[1].to_ascii_uppercase()])
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShardKey(pub i64);

impl ShardKey {
    /// Deterministic shard key. The high 16 bits encode the region (so
    /// breakouts by region are a simple range scan), the low 48 bits are a
    /// hash of (tenant_id, region) for spread within the region.
    pub fn derive(tenant_id: Uuid, region: Region) -> Self {
        let region_prefix: u16 = match region {
            Region::Us { state } => {
                // 0x1000 | state code packed
                0x1000 | ((state[0] as u16 & 0x3F) << 6) | (state[1] as u16 & 0x3F)
            }
            Region::Eu { country } => {
                0x2000 | ((country[0] as u16 & 0x3F) << 6) | (country[1] as u16 & 0x3F)
            }
            Region::Other { country } => {
                0x3000 | ((country[0] as u16 & 0x3F) << 6) | (country[1] as u16 & 0x3F)
            }
        };

        let mut hasher = Sha256::new();
        hasher.update(tenant_id.as_bytes());
        match region {
            Region::Us { state } => { hasher.update(b"us"); hasher.update(state); }
            Region::Eu { country } => { hasher.update(b"eu"); hasher.update(country); }
            Region::Other { country } => { hasher.update(b"ot"); hasher.update(country); }
        }
        let digest = hasher.finalize();
        // Low 48 bits from the digest
        let lo48 = u64::from_be_bytes([
            0, 0, digest[0], digest[1], digest[2], digest[3], digest[4], digest[5],
        ]) & 0x0000_FFFF_FFFF_FFFF;

        // Pack: top 16 bits region, bottom 48 bits hash
        let unsigned = ((region_prefix as u64) << 48) | lo48;
        // Reinterpret as i64 (Postgres BIGINT). The bit pattern is what we
        // index on; the signedness is a Postgres storage quirk only.
        ShardKey(unsigned as i64)
    }
}
