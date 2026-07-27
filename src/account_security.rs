use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

const MIN_PIN_KDF_MEMORY_KIB: u32 = 8 * 1024;
const MAX_PIN_KDF_MEMORY_KIB: u32 = 1024 * 1024;
const MAX_RECOVERY_DESTINATION_LEN: usize = 320;
const MAX_RECOVERY_CODE_LEN: usize = 10;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DeviceLifecycleState {
    Pending,
    Active,
    Suspended,
    Revoked,
}

impl DeviceLifecycleState {
    pub fn can_transition_to(self, next: Self) -> bool {
        if self == next {
            return true;
        }
        match self {
            Self::Pending => matches!(next, Self::Active | Self::Suspended | Self::Revoked),
            Self::Active => matches!(next, Self::Suspended | Self::Revoked),
            Self::Suspended => matches!(next, Self::Active | Self::Revoked),
            Self::Revoked => false,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryChannelKind {
    Email,
    Phone,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryPurpose {
    AddDevice,
    AccountRecovery,
    StepUp,
    ChangeChannel,
    RevokeDevice,
}

/// Public policy for protecting a random device-wrapping key locally.
///
/// No PIN, PIN verifier, biometric template, or private key is represented.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PinKdfPolicy {
    pub algorithm: String,
    pub memory_kib: u32,
    pub iterations: u32,
    pub parallelism: u32,
    pub max_attempts: u32,
    pub lockout_seconds: u32,
}

impl PinKdfPolicy {
    pub fn validate(&self) -> Result<(), &'static str> {
        if !matches!(self.algorithm.as_str(), "argon2id-v1" | "scrypt-v1") {
            return Err("unsupported PIN KDF policy");
        }
        if !(MIN_PIN_KDF_MEMORY_KIB..=MAX_PIN_KDF_MEMORY_KIB).contains(&self.memory_kib)
            || !(1..=20).contains(&self.iterations)
            || !(1..=8).contains(&self.parallelism)
            || !(3..=20).contains(&self.max_attempts)
            || !(1..=86_400).contains(&self.lockout_seconds)
        {
            return Err("PIN KDF or throttling policy is outside supported bounds");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LocalUnlockPolicy {
    pub pin_enabled: bool,
    pub biometric_enabled: bool,
    pub passkey_enabled: bool,
    pub pin_kdf: Option<PinKdfPolicy>,
}

impl LocalUnlockPolicy {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.pin_enabled && self.pin_kdf.is_none() {
            return Err("PIN unlock requires a bounded KDF policy");
        }
        if let Some(policy) = &self.pin_kdf {
            policy.validate()?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DeviceSummary {
    pub device_id: Uuid,
    pub device_name: String,
    pub platform: String,
    pub state: DeviceLifecycleState,
    pub device_list_revision: u64,
    pub identity_key_fingerprint_base64: String,
    pub local_unlock: LocalUnlockPolicy,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub verified_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_seen_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub revoked_at: Option<OffsetDateTime>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct UpsertRecoveryChannelRequest {
    pub kind: RecoveryChannelKind,
    pub destination: String,
}

impl UpsertRecoveryChannelRequest {
    pub fn validate(&self) -> Result<(), &'static str> {
        let destination = self.destination.trim();
        if destination.is_empty()
            || destination.len() > MAX_RECOVERY_DESTINATION_LEN
            || destination.chars().any(char::is_control)
        {
            return Err("recovery destination is invalid");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RecoveryChannelSummary {
    pub channel_id: Uuid,
    pub kind: RecoveryChannelKind,
    pub masked_destination: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub verified_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub disabled_at: Option<OffsetDateTime>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RecoveryChallenge {
    pub challenge_id: Uuid,
    pub channel_id: Uuid,
    pub purpose: RecoveryPurpose,
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
    pub remaining_attempts: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct VerifyRecoveryChallengeRequest {
    pub challenge_id: Uuid,
    pub code: String,
}

impl VerifyRecoveryChallengeRequest {
    pub fn validate(&self) -> Result<(), &'static str> {
        if !(6..=MAX_RECOVERY_CODE_LEN).contains(&self.code.len())
            || !self.code.bytes().all(|byte| byte.is_ascii_digit())
        {
            return Err("recovery code must contain 6 through 10 digits");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn revoked_is_terminal_but_suspension_is_reversible() {
        assert!(DeviceLifecycleState::Active.can_transition_to(DeviceLifecycleState::Suspended));
        assert!(DeviceLifecycleState::Suspended.can_transition_to(DeviceLifecycleState::Active));
        assert!(!DeviceLifecycleState::Revoked.can_transition_to(DeviceLifecycleState::Active));
    }

    #[test]
    fn pin_policy_never_contains_a_pin_and_is_bounded() {
        let policy = LocalUnlockPolicy {
            pin_enabled: true,
            biometric_enabled: true,
            passkey_enabled: true,
            pin_kdf: Some(PinKdfPolicy {
                algorithm: "argon2id-v1".into(),
                memory_kib: 65_536,
                iterations: 3,
                parallelism: 1,
                max_attempts: 10,
                lockout_seconds: 60,
            }),
        };
        assert_eq!(policy.validate(), Ok(()));
        let serialized = serde_json::to_string(&policy).unwrap();
        assert!(!serialized.contains("123456"));
        assert!(!serialized.contains("fingerprint"));
    }

    #[test]
    fn recovery_code_is_numeric_and_bounded() {
        let request = VerifyRecoveryChallengeRequest {
            challenge_id: Uuid::nil(),
            code: "123456".into(),
        };
        assert_eq!(request.validate(), Ok(()));
        let malformed = VerifyRecoveryChallengeRequest {
            code: "12 456".into(),
            ..request
        };
        assert!(malformed.validate().is_err());
    }
}
