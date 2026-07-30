//! Versioned, deterministic ledger-intent fingerprints.
//!
//! An idempotency key identifies one business intent, not merely one database
//! row. The fingerprint is persisted beside the key so a replay with different
//! accounting content fails closed instead of silently returning the original
//! transaction.

use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::fmt::Write;
use uuid::Uuid;

pub const LEGACY_INTENT_FINGERPRINT: &str = "legacy:v0";
const DOMAIN_SEPARATOR: &[u8] = b"quaestor-ledger:intent:v1\0";

#[derive(Clone, Debug)]
pub struct LedgerIntent {
    pub tenant_id: Uuid,
    pub kind: String,
    pub description: Option<String>,
    pub metadata: Value,
    pub postings: Vec<LedgerPostingIntent>,
}

#[derive(Clone, Debug)]
pub struct LedgerPostingIntent {
    pub account_code: String,
    pub direction: String,
    pub amount_minor: i128,
    pub currency: String,
    pub source: String,
    pub source_event_id: String,
    pub metadata: Value,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReplayFingerprint {
    Exact,
    Legacy,
    Mismatch,
}

pub fn classify_replay(existing: &str, incoming: &str) -> ReplayFingerprint {
    if existing == LEGACY_INTENT_FINGERPRINT {
        ReplayFingerprint::Legacy
    } else if existing == incoming {
        ReplayFingerprint::Exact
    } else {
        ReplayFingerprint::Mismatch
    }
}

/// Return `sha256:v1:<lowercase hex>` for a canonical ledger intent.
///
/// JSON object-key order and posting order are deliberately ignored. JSON
/// array order and duplicate postings are retained because both can carry
/// meaning. Amounts are encoded as decimal strings so the full signed `i128`
/// domain is stable across JSON implementations.
pub fn intent_fingerprint(intent: &LedgerIntent) -> String {
    let mut postings = intent
        .postings
        .iter()
        .map(|posting| {
            canonicalize(json!({
                "accountCode": posting.account_code,
                "direction": posting.direction,
                "amountMinor": posting.amount_minor.to_string(),
                "currency": posting.currency,
                "source": posting.source,
                "sourceEventId": posting.source_event_id,
                "metadata": posting.metadata,
            }))
        })
        .collect::<Vec<_>>();

    postings.sort_by_cached_key(canonical_json);

    let canonical = canonicalize(json!({
        "tenantId": intent.tenant_id.to_string(),
        "kind": intent.kind,
        "description": intent.description,
        "metadata": intent.metadata,
        "postings": postings,
    }));
    let encoded = canonical_json(&canonical);

    let mut hasher = Sha256::new();
    hasher.update(DOMAIN_SEPARATOR);
    hasher.update(encoded.as_bytes());
    let mut fingerprint = String::from("sha256:v1:");
    for byte in hasher.finalize() {
        write!(&mut fingerprint, "{byte:02x}").expect("writing to a String is infallible");
    }
    fingerprint
}

fn canonical_json(value: &Value) -> String {
    serde_json::to_string(value).expect("serializing a JSON Value is infallible")
}

fn canonicalize(value: Value) -> Value {
    match value {
        Value::Object(object) => {
            let mut entries = object.into_iter().collect::<Vec<_>>();
            entries.sort_by(|(left, _), (right, _)| left.cmp(right));

            let mut canonical = Map::new();
            for (key, value) in entries {
                canonical.insert(key, canonicalize(value));
            }
            Value::Object(canonical)
        }
        Value::Array(values) => {
            Value::Array(values.into_iter().map(canonicalize).collect::<Vec<_>>())
        }
        scalar => scalar,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vector() -> LedgerIntent {
        LedgerIntent {
            tenant_id: Uuid::from_u128(1),
            kind: "invoice.payment".into(),
            description: Some("invoice 42".into()),
            metadata: json!({
                "z": 2,
                "nested": {"z": 2, "a": 1},
                "a": 1,
            }),
            postings: vec![
                LedgerPostingIntent {
                    account_code: "revenue/service".into(),
                    direction: "credit".into(),
                    amount_minor: 1_250,
                    currency: "USD".into(),
                    source: "stripe".into(),
                    source_event_id: "evt_42".into(),
                    metadata: json!({"z": 2, "a": 1}),
                },
                LedgerPostingIntent {
                    account_code: "cash/usd".into(),
                    direction: "debit".into(),
                    amount_minor: 1_250,
                    currency: "USD".into(),
                    source: "stripe".into(),
                    source_event_id: "evt_42".into(),
                    metadata: json!({"a": 1, "z": 2}),
                },
            ],
        }
    }

    #[test]
    fn fingerprint_matches_the_versioned_cross_language_vector() {
        assert_eq!(
            intent_fingerprint(&vector()),
            "sha256:v1:8704e4581065bfc21e13aee8680836c727f12a2aef83a7386b925174896c0afb"
        );
    }

    #[test]
    fn object_key_and_posting_order_are_transport_noise() {
        let original = vector();
        let mut reordered = vector();
        reordered.postings.reverse();
        reordered.metadata = json!({
            "a": 1,
            "nested": {"a": 1, "z": 2},
            "z": 2,
        });

        assert_eq!(
            intent_fingerprint(&original),
            intent_fingerprint(&reordered)
        );
    }

    #[test]
    fn every_accounting_field_and_duplicate_remains_significant() {
        let baseline = intent_fingerprint(&vector());

        let mut changed_amount = vector();
        changed_amount.postings[0].amount_minor += 1;
        assert_ne!(baseline, intent_fingerprint(&changed_amount));

        let mut changed_array_order = vector();
        changed_array_order.metadata = json!({"steps": ["authorize", "capture"]});
        let forward = intent_fingerprint(&changed_array_order);
        changed_array_order.metadata = json!({"steps": ["capture", "authorize"]});
        assert_ne!(forward, intent_fingerprint(&changed_array_order));

        let mut duplicated = vector();
        duplicated.postings.push(duplicated.postings[0].clone());
        assert_ne!(baseline, intent_fingerprint(&duplicated));
    }

    #[test]
    fn legacy_and_mismatched_replays_are_distinct_from_exact_replays() {
        let fingerprint = intent_fingerprint(&vector());
        assert_eq!(
            classify_replay(&fingerprint, &fingerprint),
            ReplayFingerprint::Exact
        );
        assert_eq!(
            classify_replay(LEGACY_INTENT_FINGERPRINT, &fingerprint),
            ReplayFingerprint::Legacy
        );
        assert_eq!(
            classify_replay("sha256:v1:different", &fingerprint),
            ReplayFingerprint::Mismatch
        );
    }
}
