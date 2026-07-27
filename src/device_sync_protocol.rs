//! Signal-family multi-device sync wire types and structural validation.
//!
//! Cryptography is client-side. The backend may validate routing metadata,
//! public prekey bundles, bounds, expiry, and replay identifiers, but it must
//! never decrypt ciphertext or receive private key/session state.

use serde::{Deserialize, Serialize};
use std::fmt;
use thiserror::Error;

pub const SIGNAL_ENVELOPE_VERSION: u16 = 1;
pub const MAX_SIGNAL_CIPHERTEXT_LEN: usize = 512 * 1024;
pub const MAX_SIGNAL_IDENTIFIER_LEN: usize = 128;
pub const MAX_SIGNAL_DEVICE_ID_LEN: usize = 64;
pub const MAX_SIGNAL_PUBLIC_KEY_LEN: usize = 4096;
pub const MAX_SIGNAL_SIGNATURE_LEN: usize = 512;
pub const MAX_SIGNAL_ENVELOPE_LIFETIME_MS: i64 = 30 * 24 * 60 * 60 * 1000;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SignalEnvelopeKind {
    VaultSnapshot,
    VaultMutation,
    VaultKeyTransfer,
    DeviceControl,
}

impl SignalEnvelopeKind {
    fn wire_name(self) -> &'static str {
        match self {
            Self::VaultSnapshot => "vault_snapshot",
            Self::VaultMutation => "vault_mutation",
            Self::VaultKeyTransfer => "vault_key_transfer",
            Self::DeviceControl => "device_control",
        }
    }
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SignalEnvelopeMetadata {
    pub version: u16,
    pub envelope_id: String,
    pub account_id: String,
    pub sender_device_id: String,
    pub recipient_device_id: String,
    pub session_id: String,
    pub message_number: u64,
    pub kind: SignalEnvelopeKind,
    pub created_at_ms: i64,
    pub expires_at_ms: i64,
}

impl fmt::Debug for SignalEnvelopeMetadata {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SignalEnvelopeMetadata")
            .field("version", &self.version)
            .field("envelope_id", &"<redacted>")
            .field("account_id", &"<redacted>")
            .field("sender_device_id", &"<redacted>")
            .field("recipient_device_id", &"<redacted>")
            .field("session_id", &"<redacted>")
            .field("message_number", &self.message_number)
            .field("kind", &self.kind)
            .field("created_at_ms", &self.created_at_ms)
            .field("expires_at_ms", &self.expires_at_ms)
            .finish()
    }
}

impl SignalEnvelopeMetadata {
    pub fn validate(&self) -> Result<(), SignalProtocolValidationError> {
        if self.version != SIGNAL_ENVELOPE_VERSION {
            return Err(SignalProtocolValidationError::UnsupportedVersion);
        }
        require_identifier(&self.envelope_id, "envelope_id", MAX_SIGNAL_IDENTIFIER_LEN)?;
        require_identifier(&self.account_id, "account_id", MAX_SIGNAL_IDENTIFIER_LEN)?;
        require_identifier(
            &self.sender_device_id,
            "sender_device_id",
            MAX_SIGNAL_DEVICE_ID_LEN,
        )?;
        require_identifier(
            &self.recipient_device_id,
            "recipient_device_id",
            MAX_SIGNAL_DEVICE_ID_LEN,
        )?;
        require_identifier(&self.session_id, "session_id", MAX_SIGNAL_IDENTIFIER_LEN)?;
        if self.sender_device_id == self.recipient_device_id {
            return Err(SignalProtocolValidationError::SelfDelivery);
        }
        let lifetime = self
            .expires_at_ms
            .checked_sub(self.created_at_ms)
            .ok_or(SignalProtocolValidationError::InvalidTimestamps)?;
        if self.created_at_ms < 0 || lifetime <= 0 || lifetime > MAX_SIGNAL_ENVELOPE_LIFETIME_MS {
            return Err(SignalProtocolValidationError::InvalidTimestamps);
        }
        Ok(())
    }

    /// Deterministic length-prefixed AEAD associated data shared with Flutter.
    pub fn associated_data(&self) -> Result<Vec<u8>, SignalProtocolValidationError> {
        self.validate()?;
        let components = [
            "3fa-signal-envelope".to_owned(),
            self.version.to_string(),
            self.envelope_id.clone(),
            self.account_id.clone(),
            self.sender_device_id.clone(),
            self.recipient_device_id.clone(),
            self.session_id.clone(),
            self.message_number.to_string(),
            self.kind.wire_name().to_owned(),
            self.created_at_ms.to_string(),
            self.expires_at_ms.to_string(),
        ];
        let mut output = Vec::with_capacity(256);
        for component in components {
            let bytes = component.as_bytes();
            output.extend_from_slice(bytes.len().to_string().as_bytes());
            output.push(b':');
            output.extend_from_slice(bytes);
            output.push(b';');
        }
        Ok(output)
    }
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SignalCiphertextEnvelope {
    pub metadata: SignalEnvelopeMetadata,
    pub ciphertext: Vec<u8>,
}

impl fmt::Debug for SignalCiphertextEnvelope {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SignalCiphertextEnvelope")
            .field("metadata", &self.metadata)
            .field("ciphertext_len", &self.ciphertext.len())
            .finish()
    }
}

impl SignalCiphertextEnvelope {
    pub fn validate(&self) -> Result<(), SignalProtocolValidationError> {
        self.metadata.validate()?;
        if self.ciphertext.is_empty() || self.ciphertext.len() > MAX_SIGNAL_CIPHERTEXT_LEN {
            return Err(SignalProtocolValidationError::InvalidCiphertextLength);
        }
        Ok(())
    }
}

/// Public material used by a client for asynchronous PQXDH-compatible session
/// setup. Private key material is intentionally impossible to represent here.
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SignalDevicePreKeyBundle {
    pub version: u16,
    pub device_id: String,
    pub registration_id: u32,
    pub identity_key: Vec<u8>,
    pub signed_pre_key_id: u32,
    pub signed_pre_key: Vec<u8>,
    pub signed_pre_key_signature: Vec<u8>,
    pub pq_signed_pre_key_id: u32,
    pub pq_signed_pre_key: Vec<u8>,
    pub pq_signed_pre_key_signature: Vec<u8>,
    pub one_time_pre_key_id: Option<u32>,
    pub one_time_pre_key: Option<Vec<u8>>,
}

impl fmt::Debug for SignalDevicePreKeyBundle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SignalDevicePreKeyBundle")
            .field("version", &self.version)
            .field("device_id", &"<redacted>")
            .field("registration_id", &self.registration_id)
            .field("identity_key_len", &self.identity_key.len())
            .field("signed_pre_key_id", &self.signed_pre_key_id)
            .field("signed_pre_key_len", &self.signed_pre_key.len())
            .field(
                "signed_pre_key_signature_len",
                &self.signed_pre_key_signature.len(),
            )
            .field("pq_signed_pre_key_id", &self.pq_signed_pre_key_id)
            .field("pq_signed_pre_key_len", &self.pq_signed_pre_key.len())
            .field(
                "pq_signed_pre_key_signature_len",
                &self.pq_signed_pre_key_signature.len(),
            )
            .field("one_time_pre_key_id", &self.one_time_pre_key_id)
            .field(
                "one_time_pre_key_len",
                &self.one_time_pre_key.as_ref().map(Vec::len),
            )
            .finish()
    }
}

impl SignalDevicePreKeyBundle {
    pub fn validate(&self) -> Result<(), SignalProtocolValidationError> {
        if self.version != SIGNAL_ENVELOPE_VERSION {
            return Err(SignalProtocolValidationError::UnsupportedVersion);
        }
        require_identifier(&self.device_id, "device_id", MAX_SIGNAL_DEVICE_ID_LEN)?;
        if self.registration_id == 0 {
            return Err(SignalProtocolValidationError::InvalidPreKeyIdentifier);
        }
        require_byte_len(
            &self.identity_key,
            "identity_key",
            32,
            MAX_SIGNAL_PUBLIC_KEY_LEN,
        )?;
        require_byte_len(
            &self.signed_pre_key,
            "signed_pre_key",
            32,
            MAX_SIGNAL_PUBLIC_KEY_LEN,
        )?;
        require_byte_len(
            &self.signed_pre_key_signature,
            "signed_pre_key_signature",
            32,
            MAX_SIGNAL_SIGNATURE_LEN,
        )?;
        require_byte_len(
            &self.pq_signed_pre_key,
            "pq_signed_pre_key",
            32,
            MAX_SIGNAL_PUBLIC_KEY_LEN,
        )?;
        require_byte_len(
            &self.pq_signed_pre_key_signature,
            "pq_signed_pre_key_signature",
            32,
            MAX_SIGNAL_SIGNATURE_LEN,
        )?;
        match (&self.one_time_pre_key_id, &self.one_time_pre_key) {
            (Some(_), Some(key)) => {
                require_byte_len(key, "one_time_pre_key", 32, MAX_SIGNAL_PUBLIC_KEY_LEN)?;
            }
            (None, None) => {}
            _ => return Err(SignalProtocolValidationError::IncompleteOneTimePreKey),
        }
        Ok(())
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum SignalProtocolValidationError {
    #[error("unsupported Signal envelope version")]
    UnsupportedVersion,
    #[error("{field} is invalid")]
    InvalidIdentifier { field: &'static str },
    #[error("sender and recipient devices must differ")]
    SelfDelivery,
    #[error("invalid envelope timestamps")]
    InvalidTimestamps,
    #[error("Signal ciphertext length is invalid")]
    InvalidCiphertextLength,
    #[error("prekey identifier is invalid")]
    InvalidPreKeyIdentifier,
    #[error("{field} length is invalid")]
    InvalidByteLength { field: &'static str },
    #[error("one-time prekey id and public material must be present together")]
    IncompleteOneTimePreKey,
}

fn identifier_is_valid(value: &str, max_len: usize) -> bool {
    !value.is_empty()
        && value.len() <= max_len
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b':' | b'.'))
}

fn require_identifier(
    value: &str,
    field: &'static str,
    max_len: usize,
) -> Result<(), SignalProtocolValidationError> {
    if identifier_is_valid(value, max_len) {
        Ok(())
    } else {
        Err(SignalProtocolValidationError::InvalidIdentifier { field })
    }
}

fn require_byte_len(
    value: &[u8],
    field: &'static str,
    min_len: usize,
    max_len: usize,
) -> Result<(), SignalProtocolValidationError> {
    if (min_len..=max_len).contains(&value.len()) {
        Ok(())
    } else {
        Err(SignalProtocolValidationError::InvalidByteLength { field })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metadata() -> SignalEnvelopeMetadata {
        SignalEnvelopeMetadata {
            version: SIGNAL_ENVELOPE_VERSION,
            envelope_id: "env-1".into(),
            account_id: "account-1".into(),
            sender_device_id: "device-a".into(),
            recipient_device_id: "device-b".into(),
            session_id: "session-1".into(),
            message_number: 7,
            kind: SignalEnvelopeKind::VaultMutation,
            created_at_ms: 1_000_000,
            expires_at_ms: 1_060_000,
        }
    }

    fn prekey_bundle() -> SignalDevicePreKeyBundle {
        SignalDevicePreKeyBundle {
            version: SIGNAL_ENVELOPE_VERSION,
            device_id: "device-b".into(),
            registration_id: 42,
            identity_key: vec![0; 32],
            signed_pre_key_id: 1,
            signed_pre_key: vec![0; 32],
            signed_pre_key_signature: vec![0; 64],
            pq_signed_pre_key_id: 2,
            pq_signed_pre_key: vec![0; 64],
            pq_signed_pre_key_signature: vec![0; 64],
            one_time_pre_key_id: Some(3),
            one_time_pre_key: Some(vec![0; 32]),
        }
    }

    #[test]
    fn associated_data_matches_the_flutter_fixture() {
        let associated_data = metadata().associated_data().unwrap();
        assert_eq!(
            String::from_utf8(associated_data).unwrap(),
            "19:3fa-signal-envelope;1:1;5:env-1;9:account-1;8:device-a;8:device-b;9:session-1;1:7;14:vault_mutation;7:1000000;7:1060000;"
        );
    }

    #[test]
    fn envelope_round_trips_and_validates() {
        let envelope = SignalCiphertextEnvelope {
            metadata: metadata(),
            ciphertext: vec![1, 2, 3, 4],
        };
        envelope.validate().unwrap();
        let json = serde_json::to_string(&envelope).unwrap();
        let decoded: SignalCiphertextEnvelope = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, envelope);
    }

    #[test]
    fn debug_output_redacts_routing_identifiers_ciphertext_and_public_key_bytes() {
        let mut secret_metadata = metadata();
        secret_metadata.envelope_id = "env-secret-123".into();
        secret_metadata.account_id = "account-secret-456".into();
        secret_metadata.sender_device_id = "sender-secret-789".into();
        secret_metadata.recipient_device_id = "recipient-secret-012".into();
        secret_metadata.session_id = "session-secret-345".into();
        let envelope = SignalCiphertextEnvelope {
            metadata: secret_metadata,
            ciphertext: vec![211, 8, 92, 177],
        };
        let debug = format!("{envelope:?}");
        for secret in [
            "env-secret-123",
            "account-secret-456",
            "sender-secret-789",
            "recipient-secret-012",
            "session-secret-345",
            "211, 8, 92, 177",
        ] {
            assert!(!debug.contains(secret), "debug output leaked {secret}: {debug}");
        }
        assert!(debug.contains("ciphertext_len: 4"));
        assert!(debug.contains("<redacted>"));

        let mut bundle = prekey_bundle();
        bundle.device_id = "device-secret-999".into();
        bundle.identity_key = vec![211; 32];
        bundle.signed_pre_key = vec![177; 32];
        bundle.signed_pre_key_signature = vec![166; 64];
        bundle.pq_signed_pre_key = vec![155; 64];
        bundle.pq_signed_pre_key_signature = vec![144; 64];
        bundle.one_time_pre_key = Some(vec![133; 32]);
        let debug = format!("{bundle:?}");
        for secret in [
            "device-secret-999",
            "211, 211",
            "177, 177",
            "166, 166",
            "155, 155",
            "144, 144",
            "133, 133",
        ] {
            assert!(!debug.contains(secret), "debug output leaked {secret}: {debug}");
        }
        assert!(debug.contains("identity_key_len: 32"));
        assert!(debug.contains("one_time_pre_key_len: Some(32)"));
    }

    #[test]
    fn malformed_routing_and_bounds_are_rejected() {
        let mut value = metadata();
        value.recipient_device_id = value.sender_device_id.clone();
        assert_eq!(
            value.validate(),
            Err(SignalProtocolValidationError::SelfDelivery)
        );

        let mut value = metadata();
        value.envelope_id = "bad id".into();
        assert!(matches!(
            value.validate(),
            Err(SignalProtocolValidationError::InvalidIdentifier {
                field: "envelope_id"
            })
        ));

        let mut value = metadata();
        value.expires_at_ms = value.created_at_ms + MAX_SIGNAL_ENVELOPE_LIFETIME_MS + 1;
        assert_eq!(
            value.validate(),
            Err(SignalProtocolValidationError::InvalidTimestamps)
        );
    }

    #[test]
    fn ciphertext_limits_are_enforced_before_decryption() {
        let empty = SignalCiphertextEnvelope {
            metadata: metadata(),
            ciphertext: Vec::new(),
        };
        assert_eq!(
            empty.validate(),
            Err(SignalProtocolValidationError::InvalidCiphertextLength)
        );

        let oversized = SignalCiphertextEnvelope {
            metadata: metadata(),
            ciphertext: vec![0; MAX_SIGNAL_CIPHERTEXT_LEN + 1],
        };
        assert_eq!(
            oversized.validate(),
            Err(SignalProtocolValidationError::InvalidCiphertextLength)
        );
    }

    #[test]
    fn prekey_bundle_never_accepts_partial_one_time_material() {
        prekey_bundle().validate().unwrap();

        let mut partial = prekey_bundle();
        partial.one_time_pre_key = None;
        assert_eq!(
            partial.validate(),
            Err(SignalProtocolValidationError::IncompleteOneTimePreKey)
        );
    }
}
