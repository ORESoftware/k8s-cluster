//! Durable MFA factor enrollment, OTP step-up, and WebAuthn/passkey ceremonies.
//!
//! All ceremony state is server-owned and consumed exactly once. TOTP seeds are
//! encrypted before persistence. WebAuthn stores only public credentials and
//! opaque server-side ceremony state; face and fingerprint templates never
//! leave the platform authenticator.

use std::sync::Arc;

use aes_gcm::{
    aead::{Aead, KeyInit as AeadKeyInit, Payload},
    Aes256Gcm, Nonce,
};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use chrono::{DateTime, FixedOffset, TimeDelta, Utc};
use hmac::{Hmac, KeyInit as HmacKeyInit, Mac};
use rand::{rngs::SysRng, TryRng};
use sea_orm::{
    ConnectOptions, ConnectionTrait, Database, DatabaseConnection, DbBackend, Statement,
    TransactionTrait,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha1::Sha1;
use sha2::Sha256;
use url::Url;
use uuid::Uuid;
use webauthn_rs::prelude::{
    Passkey, PasskeyAuthentication, PasskeyRegistration, PublicKeyCredential,
    RegisterPublicKeyCredential, Webauthn, WebauthnBuilder,
};

use crate::config::DbConfig;
use crate::error::AuthError;
use crate::state::AppState;
use crate::token::OreClaims;

use crate::http::bearer;
use crate::http::introspect::active_claims;
use crate::http::session_tokens;

const OTP_TTL_MINUTES: i64 = 10;
const OTP_RESEND_INTERVAL_SECONDS: i64 = 30;
const MAX_ACTIVE_OTP_CHALLENGES: i64 = 3;
const PASSKEY_TTL_MINUTES: i64 = 5;
const MAX_ACTIVE_PASSKEY_CEREMONIES: i64 = 3;
const TOTP_STEP_SECONDS: u64 = 30;
const TOTP_DIGITS: u32 = 1_000_000;
const TOTP_ENCRYPTION_VERSION: i64 = 1;
const MAX_OTP_ATTEMPTS: i32 = 5;

#[derive(Clone)]
pub struct FactorService {
    db: Arc<DatabaseConnection>,
    totp_key: Option<[u8; 32]>,
    webauthn: Option<Arc<Webauthn>>,
}

include!("factors/core.rs");
include!("factors/passkeys.rs");
include!("factors/otp.rs");
include!("factors/api.rs");
include!("factors/helpers.rs");
include!("factors/tests.rs");
