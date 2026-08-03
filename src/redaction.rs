use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use utoipa::ToSchema;

use crate::contracts::PushTarget;

const FINGERPRINT_HEX_LENGTH: usize = 24;

/// Stable, non-reversible identifier for a capability target.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(transparent)]
pub struct TargetFingerprint(String);

impl TargetFingerprint {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for TargetFingerprint {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

pub fn fingerprint_target(target: &PushTarget) -> TargetFingerprint {
    let (provider, parts) = target.fingerprint_material();
    let mut hasher = Sha256::new();
    hasher.update(b"push-target-v1\0");
    hasher.update(provider.as_bytes());

    for part in parts {
        hasher.update(b"\0");
        hasher.update(part.as_bytes());
    }

    let digest = hex::encode(hasher.finalize());
    TargetFingerprint(format!("{provider}:{}", &digest[..FINGERPRINT_HEX_LENGTH]))
}

/// Truncate text on a UTF-8 boundary to keep provider errors bounded and safe.
pub fn truncate_utf8(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_owned();
    }

    let mut boundary = max_bytes;
    while boundary > 0 && !value.is_char_boundary(boundary) {
        boundary -= 1;
    }

    value[..boundary].to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contracts::PushTarget;

    #[test]
    fn fingerprints_are_stable_and_do_not_contain_target() {
        let target = PushTarget::Fcm {
            token: "sensitive-token".to_owned(),
        };

        let first = fingerprint_target(&target);
        let second = fingerprint_target(&target);

        assert_eq!(first, second);
        assert!(first.as_str().starts_with("fcm:"));
        assert!(!first.as_str().contains("sensitive-token"));
    }

    #[test]
    fn fingerprints_change_when_target_changes() {
        let first = fingerprint_target(&PushTarget::Fcm {
            token: "token-one".to_owned(),
        });
        let second = fingerprint_target(&PushTarget::Fcm {
            token: "token-two".to_owned(),
        });

        assert_ne!(first, second);
    }

    #[test]
    fn truncation_preserves_utf8_boundaries() {
        assert_eq!(truncate_utf8("hello", 5), "hello");
        assert_eq!(truncate_utf8("aébc", 2), "a");
        assert_eq!(truncate_utf8("aébc", 3), "aé");
        assert_eq!(truncate_utf8("abc", 0), "");
    }
}
