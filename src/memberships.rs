//! Quaestor-owned tenant authorization.
//!
//! Shared Auth proves who the caller is and how strongly they authenticated.
//! This module decides which billing tenant that identity may operate and with
//! which financial scopes. Grants are stored in Quaestor's database so they are
//! auditable, immediately revocable, and never sourced from user-writable token
//! metadata.

use chrono::{DateTime, Utc};
use sea_orm::{ConnectionTrait, DatabaseConnection, TransactionTrait};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::db::stmt;
use crate::error::{AppError, AppResult};

pub const SCOPE_BILLING_READ: &str = "billing:read";
pub const SCOPE_BILLING_WRITE: &str = "billing:write";
pub const SCOPE_BILLING_ADMIN: &str = "billing:admin";

const ALLOWED_ROLES: [&str; 4] = ["owner", "admin", "billing", "reader"];
const ALLOWED_SCOPES: [&str; 3] = [SCOPE_BILLING_READ, SCOPE_BILLING_WRITE, SCOPE_BILLING_ADMIN];

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TenantGrant {
    pub tenant_id: Uuid,
    pub shared_user_id: String,
    pub role: String,
    pub scopes: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl TenantGrant {
    pub fn has_scope(&self, scope: &str) -> bool {
        self.scopes.iter().any(|candidate| candidate == scope)
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct UpsertMembership {
    pub role: String,
    pub scopes: Vec<String>,
}

#[derive(Clone)]
pub struct MembershipService {
    pool: DatabaseConnection,
}

impl MembershipService {
    pub fn new(pool: DatabaseConnection) -> Self {
        Self { pool }
    }

    pub async fn grant_for(
        &self,
        tenant_id: Uuid,
        shared_user_id: &str,
    ) -> AppResult<Option<TenantGrant>> {
        let shared_user_id = canonical_subject(shared_user_id)?;
        grant_for_on(&self.pool, tenant_id, &shared_user_id).await
    }

    pub async fn require_scope(
        &self,
        tenant_id: Uuid,
        shared_user_id: &str,
        scope: &str,
    ) -> AppResult<TenantGrant> {
        let grant = self
            .grant_for(tenant_id, shared_user_id)
            .await?
            .ok_or(AppError::Forbidden)?;
        if grant.has_scope(scope) {
            Ok(grant)
        } else {
            Err(AppError::Forbidden)
        }
    }

    pub async fn list(&self, tenant_id: Uuid) -> AppResult<Vec<TenantGrant>> {
        let rows = self
            .pool
            .query_all(stmt(
                r#"
                SELECT tenant_id, shared_user_id, role, scopes, created_at, updated_at
                FROM tenant_memberships
                WHERE tenant_id = $1 AND revoked_at IS NULL
                ORDER BY created_at ASC, shared_user_id ASC
                LIMIT 1000
                "#,
                [tenant_id.into()],
            ))
            .await?;
        rows.into_iter().map(grant_from_row).collect()
    }

    pub async fn upsert(
        &self,
        tenant_id: Uuid,
        shared_user_id: &str,
        input: UpsertMembership,
        actor_shared_user_id: &str,
    ) -> AppResult<TenantGrant> {
        let shared_user_id = canonical_subject(shared_user_id)?;
        let actor_shared_user_id = canonical_subject(actor_shared_user_id)?;
        let role = normalize_role(&input.role)?;
        let scopes = normalize_scopes(input.scopes, &role)?;
        let transaction = self.pool.begin().await?;
        lock_tenant(&transaction, tenant_id).await?;
        let actor = grant_for_on(&transaction, tenant_id, &actor_shared_user_id)
            .await?
            .ok_or(AppError::Forbidden)?;
        let current = grant_for_on(&transaction, tenant_id, &shared_user_id).await?;
        authorize_membership_change(
            &actor.role,
            actor.has_scope(SCOPE_BILLING_ADMIN),
            current.as_ref().map(|grant| grant.role.as_str()),
            Some(&role),
        )?;
        let row = transaction
            .query_one(stmt(
                r#"
                INSERT INTO tenant_memberships
                    (tenant_id, shared_user_id, role, scopes,
                     granted_by_shared_user_id, revoked_at, updated_at)
                VALUES ($1, $2, $3, $4, $5, NULL, now())
                ON CONFLICT (tenant_id, shared_user_id) DO UPDATE SET
                    role = EXCLUDED.role,
                    scopes = EXCLUDED.scopes,
                    granted_by_shared_user_id = EXCLUDED.granted_by_shared_user_id,
                    revoked_at = NULL,
                    updated_at = now()
                RETURNING tenant_id, shared_user_id, role, scopes,
                          created_at, updated_at
                "#,
                [
                    tenant_id.into(),
                    shared_user_id.clone().into(),
                    role.clone().into(),
                    scopes.clone().into(),
                    actor_shared_user_id.clone().into(),
                ],
            ))
            .await?
            .ok_or_else(|| AppError::Other(anyhow::anyhow!("membership upsert returned no row")))?;
        record_event(
            &transaction,
            tenant_id,
            &shared_user_id,
            &actor_shared_user_id,
            "grant_or_update",
            Some(&role),
            &scopes,
        )
        .await?;
        transaction.commit().await?;
        grant_from_row(row)
    }

    pub async fn revoke(
        &self,
        tenant_id: Uuid,
        shared_user_id: &str,
        actor_shared_user_id: &str,
    ) -> AppResult<()> {
        let shared_user_id = canonical_subject(shared_user_id)?;
        let actor_shared_user_id = canonical_subject(actor_shared_user_id)?;
        if shared_user_id == actor_shared_user_id {
            return Err(AppError::BadRequest(
                "operators may not revoke their own active membership".to_owned(),
            ));
        }
        let transaction = self.pool.begin().await?;
        lock_tenant(&transaction, tenant_id).await?;
        let actor = grant_for_on(&transaction, tenant_id, &actor_shared_user_id)
            .await?
            .ok_or(AppError::Forbidden)?;
        let current = grant_for_on(&transaction, tenant_id, &shared_user_id)
            .await?
            .ok_or_else(|| {
                AppError::NotFound(format!("active membership {tenant_id}/{shared_user_id}"))
            })?;
        authorize_membership_change(
            &actor.role,
            actor.has_scope(SCOPE_BILLING_ADMIN),
            Some(&current.role),
            None,
        )?;
        let result = transaction
            .execute(stmt(
                r#"
                UPDATE tenant_memberships
                SET revoked_at = now(), updated_at = now()
                WHERE tenant_id = $1
                  AND shared_user_id = $2
                  AND revoked_at IS NULL
                "#,
                [tenant_id.into(), shared_user_id.clone().into()],
            ))
            .await?;
        if result.rows_affected() == 0 {
            return Err(AppError::NotFound(format!(
                "active membership {tenant_id}/{shared_user_id}"
            )));
        }
        record_event(
            &transaction,
            tenant_id,
            &shared_user_id,
            &actor_shared_user_id,
            "revoke",
            None,
            &[],
        )
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    /// Create the first owner inside the same transaction as tenant creation.
    pub async fn create_owner_on<C>(
        connection: &C,
        tenant_id: Uuid,
        shared_user_id: &str,
    ) -> AppResult<()>
    where
        C: ConnectionTrait,
    {
        let shared_user_id = canonical_subject(shared_user_id)?;
        let scopes = vec![
            SCOPE_BILLING_READ.to_owned(),
            SCOPE_BILLING_WRITE.to_owned(),
            SCOPE_BILLING_ADMIN.to_owned(),
        ];
        connection
            .execute(stmt(
                r#"
                INSERT INTO tenant_memberships
                    (tenant_id, shared_user_id, role, scopes,
                     granted_by_shared_user_id)
                VALUES ($1, $2, 'owner', $3, $2)
                "#,
                [
                    tenant_id.into(),
                    shared_user_id.clone().into(),
                    scopes.clone().into(),
                ],
            ))
            .await?;
        record_event(
            connection,
            tenant_id,
            &shared_user_id,
            &shared_user_id,
            "create_owner",
            Some("owner"),
            &scopes,
        )
        .await?;
        Ok(())
    }

    /// Readiness check for the authorization schema. A deployment without the
    /// reviewed membership migration must not receive protected traffic.
    pub async fn schema_ready(&self) -> bool {
        self.pool
            .query_one(stmt("SELECT 1 FROM tenant_memberships LIMIT 1", []))
            .await
            .is_ok()
    }
}

async fn lock_tenant<C>(connection: &C, tenant_id: Uuid) -> AppResult<()>
where
    C: ConnectionTrait,
{
    connection
        .query_one(stmt(
            "SELECT id FROM tenants WHERE id = $1 FOR UPDATE",
            [tenant_id.into()],
        ))
        .await?
        .ok_or_else(|| AppError::NotFound(format!("tenant {tenant_id}")))?;
    Ok(())
}

async fn grant_for_on<C>(
    connection: &C,
    tenant_id: Uuid,
    shared_user_id: &str,
) -> AppResult<Option<TenantGrant>>
where
    C: ConnectionTrait,
{
    let row = connection
        .query_one(stmt(
            r#"
            SELECT tenant_id, shared_user_id, role, scopes, created_at, updated_at
            FROM tenant_memberships
            WHERE tenant_id = $1
              AND shared_user_id = $2
              AND revoked_at IS NULL
            "#,
            [tenant_id.into(), shared_user_id.to_owned().into()],
        ))
        .await?;
    row.map(grant_from_row).transpose()
}

fn authorize_membership_change(
    actor_role: &str,
    actor_has_admin_scope: bool,
    current_target_role: Option<&str>,
    requested_role: Option<&str>,
) -> AppResult<()> {
    if !actor_has_admin_scope || !matches!(actor_role, "owner" | "admin") {
        return Err(AppError::Forbidden);
    }

    let touches_ownership = current_target_role == Some("owner") || requested_role == Some("owner");
    if touches_ownership && actor_role != "owner" {
        return Err(AppError::Forbidden);
    }
    Ok(())
}

async fn record_event<C>(
    connection: &C,
    tenant_id: Uuid,
    shared_user_id: &str,
    actor_shared_user_id: &str,
    event_type: &str,
    role: Option<&str>,
    scopes: &[String],
) -> AppResult<()>
where
    C: ConnectionTrait,
{
    connection
        .execute(stmt(
            r#"
            INSERT INTO tenant_membership_events
                (tenant_id, shared_user_id, actor_shared_user_id,
                 event_type, role, scopes)
            VALUES ($1, $2, $3, $4, $5, $6)
            "#,
            [
                tenant_id.into(),
                shared_user_id.to_owned().into(),
                actor_shared_user_id.to_owned().into(),
                event_type.to_owned().into(),
                role.map(str::to_owned).into(),
                scopes.to_vec().into(),
            ],
        ))
        .await?;
    Ok(())
}

fn grant_from_row(row: sea_orm::QueryResult) -> AppResult<TenantGrant> {
    Ok(TenantGrant {
        tenant_id: row.try_get("", "tenant_id")?,
        shared_user_id: row.try_get("", "shared_user_id")?,
        role: row.try_get("", "role")?,
        scopes: row.try_get("", "scopes")?,
        created_at: row.try_get("", "created_at")?,
        updated_at: row.try_get("", "updated_at")?,
    })
}

fn canonical_subject(subject: &str) -> AppResult<String> {
    let canonical = subject.trim();
    if canonical != subject
        || canonical.is_empty()
        || canonical.len() > 200
        || canonical
            .chars()
            .any(|character| character.is_control() || matches!(character, '/' | '\\'))
    {
        return Err(AppError::BadRequest(
            "invalid or non-canonical Shared Auth subject".to_owned(),
        ));
    }
    Ok(canonical.to_owned())
}

fn normalize_role(role: &str) -> AppResult<String> {
    let role = role.trim().to_ascii_lowercase();
    if ALLOWED_ROLES.contains(&role.as_str()) {
        Ok(role)
    } else {
        Err(AppError::BadRequest(format!(
            "role must be one of {}",
            ALLOWED_ROLES.join(", ")
        )))
    }
}

fn normalize_scopes(mut scopes: Vec<String>, role: &str) -> AppResult<Vec<String>> {
    if scopes.len() > ALLOWED_SCOPES.len() {
        return Err(AppError::BadRequest(
            "too many membership scopes".to_owned(),
        ));
    }
    scopes = scopes
        .into_iter()
        .map(|scope| scope.trim().to_ascii_lowercase())
        .collect();
    scopes.sort_unstable();
    if scopes.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(AppError::BadRequest(
            "membership scopes must not contain duplicates".to_owned(),
        ));
    }
    if scopes.is_empty()
        || scopes
            .iter()
            .any(|scope| !ALLOWED_SCOPES.contains(&scope.as_str()))
    {
        return Err(AppError::BadRequest(format!(
            "scopes must be selected from {}",
            ALLOWED_SCOPES.join(", ")
        )));
    }
    if !scopes.iter().any(|scope| scope == SCOPE_BILLING_READ) {
        return Err(AppError::BadRequest(
            "every active membership must include billing:read".to_owned(),
        ));
    }

    match role {
        "owner" | "admin" => {
            if !scopes.iter().any(|scope| scope == SCOPE_BILLING_WRITE)
                || !scopes.iter().any(|scope| scope == SCOPE_BILLING_ADMIN)
            {
                return Err(AppError::BadRequest(
                    "owner/admin memberships require billing:read, billing:write, and billing:admin"
                        .to_owned(),
                ));
            }
        }
        "billing" => {
            if scopes.iter().any(|scope| scope == SCOPE_BILLING_ADMIN) {
                return Err(AppError::BadRequest(
                    "billing memberships may not carry billing:admin".to_owned(),
                ));
            }
        }
        "reader" => {
            if scopes != [SCOPE_BILLING_READ] {
                return Err(AppError::BadRequest(
                    "reader memberships may only carry billing:read".to_owned(),
                ));
            }
        }
        _ => {
            return Err(AppError::BadRequest("invalid membership role".to_owned()));
        }
    }

    Ok(scopes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_and_scope_policy_is_fail_closed() {
        assert!(normalize_role("owner").is_ok());
        assert!(normalize_role("superuser").is_err());
        assert!(normalize_scopes(vec![SCOPE_BILLING_READ.into()], "reader").is_ok());
        assert!(
            normalize_scopes(
                vec![SCOPE_BILLING_READ.into(), SCOPE_BILLING_READ.into()],
                "reader"
            )
            .is_err()
        );
        assert!(
            normalize_scopes(
                vec![" Billing:Read ".into(), "billing:read".into()],
                "reader"
            )
            .is_err()
        );
        assert!(normalize_scopes(vec![SCOPE_BILLING_WRITE.into()], "billing").is_err());
        assert!(
            normalize_scopes(
                vec![SCOPE_BILLING_READ.into(), SCOPE_BILLING_WRITE.into()],
                "reader"
            )
            .is_err()
        );
        assert!(
            normalize_scopes(
                vec![SCOPE_BILLING_READ.into(), SCOPE_BILLING_ADMIN.into()],
                "admin"
            )
            .is_err()
        );
        assert!(
            normalize_scopes(
                vec![
                    SCOPE_BILLING_READ.into(),
                    SCOPE_BILLING_WRITE.into(),
                    SCOPE_BILLING_ADMIN.into(),
                ],
                "admin"
            )
            .is_ok()
        );
        assert!(
            normalize_scopes(
                vec![SCOPE_BILLING_READ.into(), SCOPE_BILLING_ADMIN.into()],
                "billing"
            )
            .is_err()
        );
    }

    #[test]
    fn shared_auth_subjects_are_canonical_and_bounded() {
        assert_eq!(canonical_subject("shared-user-1").unwrap(), "shared-user-1");
        assert!(canonical_subject("").is_err());
        assert!(canonical_subject(" padded").is_err());
        assert!(canonical_subject("padded ").is_err());
        assert!(canonical_subject("../other").is_err());
        assert!(canonical_subject(&"x".repeat(201)).is_err());
    }

    #[test]
    fn only_owners_may_change_ownership() {
        assert!(authorize_membership_change("admin", true, Some("billing"), Some("admin")).is_ok());
        assert!(authorize_membership_change("admin", true, None, Some("owner")).is_err());
        assert!(authorize_membership_change("admin", true, Some("owner"), Some("admin")).is_err());
        assert!(authorize_membership_change("admin", true, Some("owner"), None).is_err());
        assert!(authorize_membership_change("owner", true, None, Some("owner")).is_ok());
        assert!(authorize_membership_change("owner", true, Some("owner"), Some("admin")).is_ok());
        assert!(authorize_membership_change("owner", true, Some("owner"), None).is_ok());
        assert!(
            authorize_membership_change("reader", false, Some("reader"), Some("reader")).is_err()
        );
    }
}
