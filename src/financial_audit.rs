//! Transactionally durable actor attribution for accepted financial operations.
//!
//! Shared Auth proves identity and session state; Quaestor owns the financial
//! audit record. This module deliberately stores only bounded identifiers and
//! authorization evidence. Bearer tokens, provider credentials, payment data,
//! request bodies, and raw idempotency keys never cross this boundary.

use axum::http::HeaderMap;
use sea_orm::ConnectionTrait;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::api::auth::Principal;
use crate::db::stmt;
use crate::error::{AppError, AppResult};

pub const REQUEST_ID_HEADER: &str = "x-request-id";
pub const LEDGER_POST_OPERATION: &str = "ledger.post_transaction";
pub const LEDGER_TRANSACTION_RESOURCE: &str = "ledger_transaction";
pub const BILLING_WRITE_SCOPE: &str = "billing:write";
pub const LEGACY_SERVICE_SCOPE: &str = "legacy:service";

const IDEMPOTENCY_KEY_MAX_BYTES: usize = 1024;
const IDEMPOTENCY_FINGERPRINT_DOMAIN: &[u8] = b"quaestor:financial-operation-idempotency:v1\0";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FinancialOperationContext {
    pub request_correlation_id: Uuid,
    pub actor_kind: &'static str,
    pub shared_user_id: Option<String>,
    pub shared_session_id: Option<String>,
    pub authorization_scope: &'static str,
    pub aal: i16,
    pub acr: Option<String>,
    pub auth_time_unix: Option<i64>,
}

impl FinancialOperationContext {
    /// Attribute a provider-sync posting to the scheduler boundary. Provider
    /// syncs have no human Shared Auth session, but they still need an explicit
    /// durable actor class and a fresh correlation id for every accepted
    /// operation.
    pub fn provider_sync() -> Self {
        Self {
            request_correlation_id: Uuid::new_v4(),
            actor_kind: "provider_sync",
            shared_user_id: None,
            shared_session_id: None,
            authorization_scope: BILLING_WRITE_SCOPE,
            aal: 0,
            acr: None,
            auth_time_unix: None,
        }
    }

    /// Resolve the authenticated principal and canonical request correlation ID.
    /// A Shared Auth user must carry a live session identifier for a durable
    /// financial mutation, even when a lower-risk development deployment has
    /// relaxed the global introspection setting.
    pub fn from_request(
        principal: &Principal,
        headers: &HeaderMap,
        authorization_scope: &'static str,
    ) -> AppResult<Self> {
        let request_correlation_id = request_correlation_id(headers)?;
        match principal {
            Principal::Service => Ok(Self {
                request_correlation_id,
                actor_kind: "legacy_service",
                shared_user_id: None,
                shared_session_id: None,
                authorization_scope: LEGACY_SERVICE_SCOPE,
                aal: 0,
                acr: None,
                auth_time_unix: None,
            }),
            Principal::User(user) => {
                let shared_session_id = user
                    .identity
                    .session_id
                    .as_deref()
                    .ok_or(AppError::Forbidden)?;
                validate_canonical_identifier(&user.identity.subject, "Shared Auth subject")?;
                validate_canonical_identifier(shared_session_id, "Shared Auth session")?;

                let aal = if user.identity.assurance.is_aal2() {
                    2
                } else {
                    1
                };
                let auth_time_unix = if aal == 2 {
                    Some(i64::try_from(user.identity.issued_at).map_err(|_| AppError::Forbidden)?)
                } else {
                    None
                };

                Ok(Self {
                    request_correlation_id,
                    actor_kind: "shared_auth_user",
                    shared_user_id: Some(user.identity.subject.clone()),
                    shared_session_id: Some(shared_session_id.to_owned()),
                    authorization_scope,
                    aal,
                    acr: user.identity.acr.clone(),
                    auth_time_unix,
                })
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FinancialAuditStatus {
    Recorded,
    LegacyUnattributed,
}

impl FinancialAuditStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Recorded => "recorded",
            Self::LegacyUnattributed => "legacy_unattributed",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FinancialOperationReceipt {
    pub event_id: Option<Uuid>,
    pub operation_correlation_id: Option<Uuid>,
    pub status: FinancialAuditStatus,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuditedLedgerPost {
    pub transaction_id: Uuid,
    pub audit: FinancialOperationReceipt,
    pub replayed: bool,
}

impl FinancialOperationReceipt {
    fn recorded(event_id: Uuid, operation_correlation_id: Uuid) -> Self {
        Self {
            event_id: Some(event_id),
            operation_correlation_id: Some(operation_correlation_id),
            status: FinancialAuditStatus::Recorded,
        }
    }

    fn legacy_unattributed() -> Self {
        Self {
            event_id: None,
            operation_correlation_id: None,
            status: FinancialAuditStatus::LegacyUnattributed,
        }
    }
}

/// Record the accepted ledger posting on the same SeaORM transaction used for
/// the transaction header and postings. A failure here aborts the financial
/// mutation; an accepted mutation without its audit event is not allowed.
pub async fn record_ledger_post<C>(
    connection: &C,
    tenant_id: Uuid,
    ledger_transaction_id: Uuid,
    context: &FinancialOperationContext,
    idempotency_key: &str,
) -> AppResult<FinancialOperationReceipt>
where
    C: ConnectionTrait,
{
    let event_id = Uuid::new_v4();
    let fingerprint = idempotency_key_fingerprint(idempotency_key)?;
    let row = connection
        .query_one(stmt(
            r#"
            INSERT INTO financial_operation_events (
                id,
                tenant_id,
                operation,
                outcome,
                actor_kind,
                shared_user_id,
                shared_session_id,
                request_correlation_id,
                authorization_scope,
                aal,
                acr,
                auth_time_unix,
                idempotency_key_fingerprint,
                resource_type,
                resource_id,
                ledger_transaction_id,
                schema_version
            )
            VALUES (
                $1, $2, $3, 'accepted', $4, $5, $6, $7, $8, $9,
                $10, $11, $12, $13, $14, $14, 1
            )
            RETURNING id, request_correlation_id
            "#,
            [
                event_id.into(),
                tenant_id.into(),
                LEDGER_POST_OPERATION.into(),
                context.actor_kind.into(),
                context.shared_user_id.clone().into(),
                context.shared_session_id.clone().into(),
                context.request_correlation_id.into(),
                context.authorization_scope.into(),
                context.aal.into(),
                context.acr.clone().into(),
                context.auth_time_unix.into(),
                fingerprint.into(),
                LEDGER_TRANSACTION_RESOURCE.into(),
                ledger_transaction_id.into(),
            ],
        ))
        .await?
        .ok_or_else(|| {
            AppError::Other(anyhow::anyhow!("financial audit insert returned no row"))
        })?;

    Ok(FinancialOperationReceipt::recorded(
        row.try_get("", "id")?,
        row.try_get("", "request_correlation_id")?,
    ))
}

/// Load the original accepted-operation identity for an idempotent replay. Rows
/// that predate the audit migration are reported explicitly rather than being
/// attributed to the replaying caller.
pub async fn load_ledger_post<C>(
    connection: &C,
    tenant_id: Uuid,
    ledger_transaction_id: Uuid,
) -> AppResult<FinancialOperationReceipt>
where
    C: ConnectionTrait,
{
    let row = connection
        .query_one(stmt(
            r#"
            SELECT id, request_correlation_id
            FROM financial_operation_events
            WHERE tenant_id = $1
              AND operation = $2
              AND ledger_transaction_id = $3
            "#,
            [
                tenant_id.into(),
                LEDGER_POST_OPERATION.into(),
                ledger_transaction_id.into(),
            ],
        ))
        .await?;

    match row {
        Some(row) => Ok(FinancialOperationReceipt::recorded(
            row.try_get("", "id")?,
            row.try_get("", "request_correlation_id")?,
        )),
        None => Ok(FinancialOperationReceipt::legacy_unattributed()),
    }
}

pub fn request_correlation_id(headers: &HeaderMap) -> AppResult<Uuid> {
    let values = headers.get_all(REQUEST_ID_HEADER);
    let mut values = values.iter();
    let Some(first) = values.next() else {
        return Ok(Uuid::new_v4());
    };
    if values.next().is_some() {
        return Err(AppError::BadRequest(
            "x-request-id must appear at most once".to_owned(),
        ));
    }

    let raw = first
        .to_str()
        .map_err(|_| AppError::BadRequest("x-request-id must be ASCII".to_owned()))?;
    let parsed = Uuid::parse_str(raw)
        .map_err(|_| AppError::BadRequest("x-request-id must be a UUID".to_owned()))?;
    if parsed.to_string() != raw {
        return Err(AppError::BadRequest(
            "x-request-id must use canonical lowercase UUID form".to_owned(),
        ));
    }
    Ok(parsed)
}

pub fn idempotency_key_fingerprint(idempotency_key: &str) -> AppResult<String> {
    if idempotency_key.is_empty() || idempotency_key.len() > IDEMPOTENCY_KEY_MAX_BYTES {
        return Err(AppError::BadRequest(format!(
            "idempotency_key must contain 1..={IDEMPOTENCY_KEY_MAX_BYTES} bytes"
        )));
    }
    let mut hasher = Sha256::new();
    hasher.update(IDEMPOTENCY_FINGERPRINT_DOMAIN);
    hasher.update(idempotency_key.as_bytes());
    Ok(format!("sha256:v1:{}", hex::encode(hasher.finalize())))
}

fn validate_canonical_identifier(value: &str, field: &str) -> AppResult<()> {
    if value.is_empty()
        || value.len() > 200
        || value.trim() != value
        || value
            .chars()
            .any(|character| character.is_control() || matches!(character, '/' | '\\'))
    {
        return Err(AppError::BadRequest(format!("invalid canonical {field}")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{HeaderValue, header::HeaderName};
    use chrono::Utc;

    use crate::api::auth::{AuthorizedUser, Principal};
    use crate::memberships::TenantGrant;
    use crate::shared_auth::{Aal, SharedAuthIdentity};

    fn principal(aal: Aal, session_id: Option<&str>) -> Principal {
        let tenant_id = Uuid::from_u128(1);
        Principal::User(Box::new(AuthorizedUser {
            identity: SharedAuthIdentity {
                subject: "shared-user-1".to_owned(),
                provider: "supabase".to_owned(),
                provider_tenant: "quaestor".to_owned(),
                provider_subject: "provider-user-1".to_owned(),
                session_id: session_id.map(str::to_owned),
                email: None,
                email_verified: false,
                roles: vec!["user".to_owned()],
                assurance: aal,
                amr: if aal.is_aal2() {
                    vec!["pwd".to_owned(), "totp".to_owned()]
                } else {
                    vec!["pwd".to_owned()]
                },
                acr: Some(
                    if aal.is_aal2() {
                        "urn:oresoftware:loa:2"
                    } else {
                        "urn:oresoftware:loa:1"
                    }
                    .to_owned(),
                ),
                issued_at: 1_700_000_000,
                expires_at: 1_700_003_600,
            },
            grant: Some(TenantGrant {
                tenant_id,
                shared_user_id: "shared-user-1".to_owned(),
                role: "billing".to_owned(),
                scopes: vec!["billing:read".to_owned(), "billing:write".to_owned()],
                created_at: Utc::now(),
                updated_at: Utc::now(),
            }),
        }))
    }

    #[test]
    fn caller_supplied_request_id_must_be_canonical_and_single() {
        let expected = Uuid::from_u128(42);
        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static(REQUEST_ID_HEADER),
            HeaderValue::from_str(&expected.to_string()).unwrap(),
        );
        assert_eq!(request_correlation_id(&headers).unwrap(), expected);

        headers.append(
            HeaderName::from_static(REQUEST_ID_HEADER),
            HeaderValue::from_static("00000000-0000-0000-0000-000000000043"),
        );
        assert!(request_correlation_id(&headers).is_err());

        let mut noncanonical = HeaderMap::new();
        noncanonical.insert(
            HeaderName::from_static(REQUEST_ID_HEADER),
            HeaderValue::from_static("00000000-0000-0000-0000-00000000002A"),
        );
        assert!(request_correlation_id(&noncanonical).is_err());
    }

    #[test]
    fn missing_request_id_is_generated() {
        assert!(!request_correlation_id(&HeaderMap::new()).unwrap().is_nil());
    }

    #[test]
    fn service_actor_uses_the_explicit_legacy_scope() {
        let context = FinancialOperationContext::from_request(
            &Principal::Service,
            &HeaderMap::new(),
            BILLING_WRITE_SCOPE,
        )
        .unwrap();
        assert_eq!(context.actor_kind, "legacy_service");
        assert_eq!(context.authorization_scope, LEGACY_SERVICE_SCOPE);
        assert_eq!(context.aal, 0);
    }

    #[test]
    fn provider_sync_has_explicit_nonhuman_attribution() {
        let context = FinancialOperationContext::provider_sync();
        assert_eq!(context.actor_kind, "provider_sync");
        assert_eq!(context.authorization_scope, BILLING_WRITE_SCOPE);
        assert!(context.shared_user_id.is_none());
        assert!(context.shared_session_id.is_none());
        assert_eq!(context.aal, 0);
    }

    #[test]
    fn shared_auth_actor_requires_a_canonical_session() {
        assert!(
            FinancialOperationContext::from_request(
                &principal(Aal::Aal2, None),
                &HeaderMap::new(),
                BILLING_WRITE_SCOPE,
            )
            .is_err()
        );
        assert!(
            FinancialOperationContext::from_request(
                &principal(Aal::Aal2, Some(" session-1")),
                &HeaderMap::new(),
                BILLING_WRITE_SCOPE,
            )
            .is_err()
        );
    }

    #[test]
    fn aal2_actor_preserves_ceremony_time_but_aal1_does_not() {
        let strong = FinancialOperationContext::from_request(
            &principal(Aal::Aal2, Some("session-1")),
            &HeaderMap::new(),
            BILLING_WRITE_SCOPE,
        )
        .unwrap();
        assert_eq!(strong.aal, 2);
        assert_eq!(strong.auth_time_unix, Some(1_700_000_000));

        let base = FinancialOperationContext::from_request(
            &principal(Aal::Aal1, Some("session-1")),
            &HeaderMap::new(),
            BILLING_WRITE_SCOPE,
        )
        .unwrap();
        assert_eq!(base.aal, 1);
        assert_eq!(base.auth_time_unix, None);
    }

    #[test]
    fn idempotency_fingerprint_is_stable_domain_separated_and_secret_free() {
        let first = idempotency_key_fingerprint("customer-visible-key").unwrap();
        let second = idempotency_key_fingerprint("customer-visible-key").unwrap();
        assert_eq!(first, second);
        assert!(first.starts_with("sha256:v1:"));
        assert_eq!(first.len(), "sha256:v1:".len() + 64);
        assert!(!first.contains("customer-visible-key"));
        assert_ne!(first, idempotency_key_fingerprint("another-key").unwrap());
    }
}
