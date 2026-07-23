//! Account-facing response contracts shared by identity enrollment routes.
//!
//! Human registration and login live in github.com/shared-auth. This service
//! owns only the mapping from a verified identity to a zero-knowledge vault and
//! its per-device sync credentials.

use serde::Serialize;
use uuid::Uuid;

pub(crate) const MAX_DEVICE_NAME_LEN: usize = 200;

#[derive(Serialize)]
pub(crate) struct TokenResponse {
    pub(crate) account_id: Uuid,
    pub(crate) device_id: Uuid,
    /// Service-local bearer token shown once; only its SHA-256 hash is stored.
    pub(crate) sync_token: String,
}

pub(crate) fn device_name_is_valid(device_name: &str) -> bool {
    !device_name.trim().is_empty()
        && device_name == device_name.trim()
        && device_name.len() <= MAX_DEVICE_NAME_LEN
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_names_are_trimmed_nonempty_and_bounded() {
        assert!(device_name_is_valid("primary desktop"));
        for invalid in ["", " desktop", "desktop "] {
            assert!(!device_name_is_valid(invalid));
        }
        assert!(!device_name_is_valid(&"d".repeat(MAX_DEVICE_NAME_LEN + 1)));
    }
}
