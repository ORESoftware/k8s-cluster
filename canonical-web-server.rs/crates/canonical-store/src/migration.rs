use sea_orm::{DatabaseBackend, Statement};
use sea_orm_migration::prelude::*;

async fn is_cockroachdb(manager: &SchemaManager<'_>) -> Result<bool, DbErr> {
    if manager.get_database_backend() != DatabaseBackend::Postgres {
        return Ok(false);
    }

    let row = manager
        .get_connection()
        .query_one_raw(Statement::from_string(
            DatabaseBackend::Postgres,
            "SELECT version()::text AS database_version",
        ))
        .await?
        .ok_or_else(|| DbErr::Custom("database version query returned no row".to_owned()))?;
    let version = row.try_get::<String>("", "database_version")?;
    Ok(version.to_ascii_lowercase().contains("cockroachdb"))
}

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(Migration),
            Box::new(AddEngagements),
            Box::new(HardenSessions),
            Box::new(AddAdminBoundary),
            Box::new(HardenSessionRevocationIdentity),
            Box::new(AddSessionRefreshLease),
            Box::new(HardenSyncInvariants),
        ]
    }
}

#[derive(DeriveMigrationName)]
struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(UserProfile::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(UserProfile::UserId)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(UserProfile::Email).string().not_null())
                    .col(ColumnDef::new(UserProfile::DisplayName).string())
                    .col(
                        ColumnDef::new(UserProfile::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(UserProfile::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(WebSession::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(WebSession::IdHash)
                            .string()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(WebSession::UserId).uuid().not_null())
                    .col(ColumnDef::new(WebSession::Email).string().not_null())
                    .col(ColumnDef::new(WebSession::SupabaseSessionId).uuid())
                    .col(
                        ColumnDef::new(WebSession::EncryptedAccessToken)
                            .text()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(WebSession::EncryptedRefreshToken)
                            .text()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(WebSession::AccessExpiresAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(ColumnDef::new(WebSession::CsrfToken).string().not_null())
                    .col(
                        ColumnDef::new(WebSession::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(WebSession::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(WebSession::ExpiresAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(ColumnDef::new(WebSession::RevokedAt).timestamp_with_time_zone())
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("web_session_user_id_idx")
                    .table(WebSession::Table)
                    .col(WebSession::UserId)
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(SyncRecord::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(SyncRecord::OwnerId).uuid().not_null())
                    .col(ColumnDef::new(SyncRecord::Collection).string().not_null())
                    .col(ColumnDef::new(SyncRecord::RecordId).uuid().not_null())
                    .col(ColumnDef::new(SyncRecord::Version).big_integer().not_null())
                    .col(ColumnDef::new(SyncRecord::Payload).json_binary().not_null())
                    .col(ColumnDef::new(SyncRecord::DeletedAt).timestamp_with_time_zone())
                    .col(
                        ColumnDef::new(SyncRecord::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .primary_key(
                        Index::create()
                            .col(SyncRecord::OwnerId)
                            .col(SyncRecord::Collection)
                            .col(SyncRecord::RecordId),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(SyncClock::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(SyncClock::OwnerId)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(SyncClock::Cursor).big_integer().not_null())
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(SyncChange::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(SyncChange::OwnerId).uuid().not_null())
                    .col(ColumnDef::new(SyncChange::Cursor).big_integer().not_null())
                    .col(ColumnDef::new(SyncChange::Collection).string().not_null())
                    .col(ColumnDef::new(SyncChange::RecordId).uuid().not_null())
                    .col(ColumnDef::new(SyncChange::Version).big_integer().not_null())
                    .col(ColumnDef::new(SyncChange::Operation).string().not_null())
                    .col(ColumnDef::new(SyncChange::Payload).json_binary().not_null())
                    .col(
                        ColumnDef::new(SyncChange::ChangedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .primary_key(
                        Index::create()
                            .col(SyncChange::OwnerId)
                            .col(SyncChange::Cursor),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("sync_change_owner_cursor_idx")
                    .table(SyncChange::Table)
                    .col(SyncChange::OwnerId)
                    .col(SyncChange::Cursor)
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(SyncReceipt::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(SyncReceipt::OwnerId).uuid().not_null())
                    .col(ColumnDef::new(SyncReceipt::ClientId).uuid().not_null())
                    .col(ColumnDef::new(SyncReceipt::MutationId).uuid().not_null())
                    .col(ColumnDef::new(SyncReceipt::RequestHash).string().not_null())
                    .col(ColumnDef::new(SyncReceipt::Result).json_binary().not_null())
                    .col(
                        ColumnDef::new(SyncReceipt::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .primary_key(
                        Index::create()
                            .col(SyncReceipt::OwnerId)
                            .col(SyncReceipt::ClientId)
                            .col(SyncReceipt::MutationId),
                    )
                    .to_owned(),
            )
            .await?;

        if manager.get_database_backend() == DatabaseBackend::Postgres {
            manager
                .get_connection()
                .execute_unprepared(
                    r#"
                    ALTER TABLE user_profile ENABLE ROW LEVEL SECURITY;
                    ALTER TABLE user_profile FORCE ROW LEVEL SECURITY;
                    ALTER TABLE sync_record ENABLE ROW LEVEL SECURITY;
                    ALTER TABLE sync_record FORCE ROW LEVEL SECURITY;
                    ALTER TABLE sync_clock ENABLE ROW LEVEL SECURITY;
                    ALTER TABLE sync_clock FORCE ROW LEVEL SECURITY;
                    ALTER TABLE sync_change ENABLE ROW LEVEL SECURITY;
                    ALTER TABLE sync_change FORCE ROW LEVEL SECURITY;
                    ALTER TABLE sync_receipt ENABLE ROW LEVEL SECURITY;
                    ALTER TABLE sync_receipt FORCE ROW LEVEL SECURITY;

                    ALTER TABLE user_profile
                      ADD CONSTRAINT user_profile_auth_user_fk
                      FOREIGN KEY (user_id) REFERENCES auth.users(id) ON DELETE CASCADE;

                    CREATE POLICY user_profile_owner ON user_profile
                      USING (user_id = auth.uid()) WITH CHECK (user_id = auth.uid());
                    CREATE POLICY sync_record_owner ON sync_record
                      USING (owner_id = auth.uid()) WITH CHECK (owner_id = auth.uid());
                    CREATE POLICY sync_clock_owner ON sync_clock
                      USING (owner_id = auth.uid()) WITH CHECK (owner_id = auth.uid());
                    CREATE POLICY sync_change_owner ON sync_change
                      USING (owner_id = auth.uid()) WITH CHECK (owner_id = auth.uid());
                    CREATE POLICY sync_receipt_owner ON sync_receipt
                      USING (owner_id = auth.uid()) WITH CHECK (owner_id = auth.uid());

                    REVOKE ALL ON TABLE web_session FROM PUBLIC, anon, authenticated;
                    "#,
                )
                .await?;
        }

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(SyncReceipt::Table)
                    .if_exists()
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(
                Table::drop()
                    .table(SyncChange::Table)
                    .if_exists()
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(Table::drop().table(SyncClock::Table).if_exists().to_owned())
            .await?;
        manager
            .drop_table(
                Table::drop()
                    .table(SyncRecord::Table)
                    .if_exists()
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(
                Table::drop()
                    .table(WebSession::Table)
                    .if_exists()
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(
                Table::drop()
                    .table(UserProfile::Table)
                    .if_exists()
                    .to_owned(),
            )
            .await?;
        Ok(())
    }
}

struct AddEngagements;

// DeriveMigrationName would reuse this module's name, colliding with the
// initial migration's version key; name this one explicitly.
impl MigrationName for AddEngagements {
    fn name(&self) -> &str {
        "m20260713_000001_add_engagements"
    }
}

struct HardenSessions;

impl MigrationName for HardenSessions {
    fn name(&self) -> &str {
        "m20260718_000002_harden_sessions"
    }
}

struct AddAdminBoundary;

impl MigrationName for AddAdminBoundary {
    fn name(&self) -> &str {
        "m20260718_000003_add_admin_boundary"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for AddAdminBoundary {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(AdminRoleAssignment::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(AdminRoleAssignment::UserId)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(AdminRoleAssignment::Role)
                            .text()
                            .not_null()
                            .check(Expr::col(AdminRoleAssignment::Role).is_in([
                                "support",
                                "user_admin",
                                "compliance_admin",
                                "security_admin",
                            ])),
                    )
                    .col(
                        ColumnDef::new(AdminRoleAssignment::GrantedBy)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(AdminRoleAssignment::GrantedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(ColumnDef::new(AdminRoleAssignment::RevokedAt).timestamp_with_time_zone())
                    .primary_key(
                        Index::create()
                            .col(AdminRoleAssignment::UserId)
                            .col(AdminRoleAssignment::Role),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("admin_role_assignment_active_idx")
                    .table(AdminRoleAssignment::Table)
                    .col(AdminRoleAssignment::UserId)
                    .col(AdminRoleAssignment::RevokedAt)
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(AdminAuditEvent::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(AdminAuditEvent::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(AdminAuditEvent::ActorId).uuid().not_null())
                    .col(
                        ColumnDef::new(AdminAuditEvent::Capability)
                            .text()
                            .not_null()
                            .check(Expr::col(AdminAuditEvent::Capability).is_in([
                                "user.read",
                                "user.invite",
                                "user.disable",
                                "engagement.read",
                                "engagement.write",
                                "role.manage",
                                "audit.read",
                                "audit.write",
                            ])),
                    )
                    .col(
                        ColumnDef::new(AdminAuditEvent::TargetType)
                            .string()
                            .not_null(),
                    )
                    .col(ColumnDef::new(AdminAuditEvent::TargetId).string())
                    .col(ColumnDef::new(AdminAuditEvent::RequestId).uuid().not_null())
                    .col(
                        ColumnDef::new(AdminAuditEvent::OccurredAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(AdminAuditEvent::Outcome)
                            .text()
                            .not_null()
                            .check(Expr::col(AdminAuditEvent::Outcome).is_in([
                                "succeeded",
                                "denied",
                                "failed",
                            ])),
                    )
                    .col(
                        ColumnDef::new(AdminAuditEvent::Metadata)
                            .json_binary()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("admin_audit_event_actor_occurred_idx")
                    .table(AdminAuditEvent::Table)
                    .col(AdminAuditEvent::ActorId)
                    .col(AdminAuditEvent::OccurredAt)
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("admin_audit_event_request_idx")
                    .table(AdminAuditEvent::Table)
                    .col(AdminAuditEvent::RequestId)
                    .to_owned(),
            )
            .await?;

        if manager.get_database_backend() == DatabaseBackend::Postgres {
            manager
                .get_connection()
                .execute_unprepared(
                    r#"
                    ALTER TABLE admin_role_assignment ENABLE ROW LEVEL SECURITY;
                    ALTER TABLE admin_role_assignment FORCE ROW LEVEL SECURITY;
                    ALTER TABLE admin_audit_event ENABLE ROW LEVEL SECURITY;
                    ALTER TABLE admin_audit_event FORCE ROW LEVEL SECURITY;

                    ALTER TABLE admin_role_assignment
                      ADD CONSTRAINT admin_role_assignment_user_fk
                      FOREIGN KEY (user_id) REFERENCES auth.users(id) ON DELETE CASCADE;
                    ALTER TABLE admin_role_assignment
                      ADD CONSTRAINT admin_role_assignment_granted_by_fk
                      FOREIGN KEY (granted_by) REFERENCES auth.users(id) ON DELETE RESTRICT;
                    ALTER TABLE admin_audit_event
                      ADD CONSTRAINT admin_audit_event_actor_fk
                      FOREIGN KEY (actor_id) REFERENCES auth.users(id) ON DELETE RESTRICT;

                    REVOKE ALL ON TABLE admin_role_assignment, admin_audit_event
                      FROM PUBLIC, anon, authenticated;
                    "#,
                )
                .await?;

            // CockroachDB exposes a PostgreSQL-compatible backend to SeaORM,
            // but does not implement these SECURITY DEFINER function forms.
            // Its admin tables deliberately have no policies, so forced RLS
            // keeps the boundary fail-closed while the functions are absent.
            if !is_cockroachdb(manager).await? {
                manager
                    .get_connection()
                    .execute_unprepared(
                        r#"

                    CREATE FUNCTION canonical_admin_has_capability(
                      requested_capability text
                    ) RETURNS boolean
                    LANGUAGE sql
                    STABLE
                    SECURITY DEFINER
                    SET search_path = pg_catalog, public
                    AS $function$
                      SELECT COALESCE(
                        (
                          COALESCE(
                            NULLIF(current_setting('request.jwt.claims', true), ''),
                            '{}'
                          )::jsonb ->> 'aal'
                        ) = 'aal2',
                        false
                      )
                      AND auth.uid() IS NOT NULL
                      AND EXISTS (
                          SELECT 1
                          FROM public.admin_role_assignment assignment
                          WHERE assignment.user_id = auth.uid()
                            AND assignment.revoked_at IS NULL
                            AND CASE assignment.role
                              WHEN 'support' THEN requested_capability IN (
                                'user.read', 'engagement.read', 'audit.write'
                              )
                              WHEN 'user_admin' THEN requested_capability IN (
                                'user.read', 'user.invite', 'user.disable', 'audit.write'
                              )
                              WHEN 'compliance_admin' THEN requested_capability IN (
                                'engagement.read', 'engagement.write', 'audit.write'
                              )
                              WHEN 'security_admin' THEN requested_capability IN (
                                'user.read', 'user.invite', 'user.disable',
                                'engagement.read', 'engagement.write', 'role.manage',
                                'audit.read', 'audit.write'
                              )
                              ELSE false
                            END
                        )
                    $function$;

                    CREATE FUNCTION canonical_admin_append_audit(
                      event_id uuid,
                      requested_capability text,
                      target_type varchar,
                      target_id varchar,
                      request_id uuid,
                      outcome text,
                      metadata jsonb
                    ) RETURNS uuid
                    LANGUAGE plpgsql
                    VOLATILE
                    SECURITY DEFINER
                    SET search_path = pg_catalog, public
                    AS $function$
                    BEGIN
                      IF NOT public.canonical_admin_has_capability('audit.write') THEN
                        RAISE EXCEPTION 'actor is not an active administrator'
                          USING ERRCODE = '42501';
                      END IF;
                      IF outcome = 'succeeded'
                         AND NOT public.canonical_admin_has_capability(requested_capability) THEN
                        RAISE EXCEPTION 'actor lacks the recorded capability'
                          USING ERRCODE = '42501';
                      END IF;
                      INSERT INTO public.admin_audit_event (
                        id, actor_id, capability, target_type, target_id,
                        request_id, occurred_at, outcome, metadata
                      ) VALUES (
                        event_id, auth.uid(), requested_capability, target_type, target_id,
                        request_id, now(), outcome, metadata
                      );
                      RETURN event_id;
                    END
                    $function$;

                    REVOKE ALL ON FUNCTION canonical_admin_has_capability(text)
                      FROM PUBLIC, anon, authenticated;
                    REVOKE ALL ON FUNCTION canonical_admin_append_audit(
                      uuid, text, varchar, varchar, uuid, text, jsonb
                    ) FROM PUBLIC, anon, authenticated;
                    "#,
                    )
                    .await?;
            }
        }

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if manager.get_database_backend() == DatabaseBackend::Postgres
            && !is_cockroachdb(manager).await?
        {
            manager
                .get_connection()
                .execute_unprepared(
                    r#"
                    DROP FUNCTION IF EXISTS canonical_admin_append_audit(
                      uuid, text, varchar, varchar, uuid, text, jsonb
                    );
                    DROP FUNCTION IF EXISTS canonical_admin_has_capability(text);
                    "#,
                )
                .await?;
        }
        manager
            .drop_table(
                Table::drop()
                    .table(AdminAuditEvent::Table)
                    .if_exists()
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(
                Table::drop()
                    .table(AdminRoleAssignment::Table)
                    .if_exists()
                    .to_owned(),
            )
            .await?;
        Ok(())
    }
}

#[async_trait::async_trait]
impl MigrationTrait for HardenSessions {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for column in [
            ColumnDef::new(WebSession::RevocationPendingAt).timestamp_with_time_zone(),
            ColumnDef::new(WebSession::RevocationNextAttemptAt).timestamp_with_time_zone(),
            ColumnDef::new(WebSession::RevocationAttempts)
                .integer()
                .not_null()
                .default(0),
            ColumnDef::new(WebSession::UpstreamRevokedAt).timestamp_with_time_zone(),
        ] {
            manager
                .alter_table(
                    Table::alter()
                        .table(WebSession::Table)
                        .add_column(column)
                        .to_owned(),
                )
                .await?;
        }
        manager
            .create_index(
                Index::create()
                    .name("web_session_revocation_retry_idx")
                    .table(WebSession::Table)
                    .col(WebSession::RevocationNextAttemptAt)
                    .to_owned(),
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_index(
                Index::drop()
                    .name("web_session_revocation_retry_idx")
                    .table(WebSession::Table)
                    .to_owned(),
            )
            .await?;
        for column in [
            WebSession::UpstreamRevokedAt,
            WebSession::RevocationAttempts,
            WebSession::RevocationNextAttemptAt,
            WebSession::RevocationPendingAt,
        ] {
            manager
                .alter_table(
                    Table::alter()
                        .table(WebSession::Table)
                        .drop_column(column)
                        .to_owned(),
                )
                .await?;
        }
        Ok(())
    }
}

struct HardenSessionRevocationIdentity;

impl MigrationName for HardenSessionRevocationIdentity {
    fn name(&self) -> &str {
        "m20260718_000004_harden_session_revocation_identity"
    }
}

struct AddSessionRefreshLease;

impl MigrationName for AddSessionRefreshLease {
    fn name(&self) -> &str {
        "m20260718_000005_add_session_refresh_lease"
    }
}

struct HardenSyncInvariants;

impl MigrationName for HardenSyncInvariants {
    fn name(&self) -> &str {
        "m20260718_000006_harden_sync_invariants"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for HardenSyncInvariants {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if manager.get_database_backend() != DatabaseBackend::Postgres {
            // The shared SQL contract describes Supabase Postgres. SQLite is
            // retained for isolated application tests, where the service
            // already validates these values before every write.
            return Ok(());
        }

        let cockroachdb = is_cockroachdb(manager).await?;

        manager
            .get_connection()
            .execute_unprepared(
                r#"
                -- Values outside the public sync contract were never emitted
                -- by the application, but normalize any legacy/manual rows so
                -- adding validated constraints remains upgrade-safe.
                UPDATE sync_clock
                SET cursor = 0
                WHERE cursor < 0;

                UPDATE sync_record
                SET version = 1
                WHERE version <= 0;

                UPDATE sync_change
                SET version = 1
                WHERE version <= 0;
                "#,
            )
            .await?;

        if !cockroachdb {
            // Checked text columns round-trip through PostgreSQL deparsing
            // byte-stably, which the dpm declarative-schema gate requires.
            // CockroachDB treats varchar and text as aliases and rejects this
            // no-op rewrite inside a schema migration transaction.
            manager
                .get_connection()
                .execute_unprepared(
                    r#"
                    ALTER TABLE sync_change
                      ALTER COLUMN operation TYPE text USING operation::text;
                    "#,
                )
                .await?;
        }

        manager
            .get_connection()
            .execute_unprepared(
                r#"
                UPDATE sync_change
                SET operation = CASE
                  WHEN lower(COALESCE(payload ->> 'deleted', 'false')) = 'true'
                    THEN 'delete'
                  ELSE 'put'
                END
                WHERE operation NOT IN ('put', 'delete');

                WITH positive_max AS (
                  SELECT owner_id, max(cursor) AS cursor
                  FROM sync_change
                  WHERE cursor > 0
                  GROUP BY owner_id
                ), invalid_cursor AS (
                  SELECT
                    sc.owner_id,
                    sc.cursor,
                    COALESCE(positive_max.cursor, 0)
                      + row_number() OVER (
                          PARTITION BY sc.owner_id
                          ORDER BY sc.changed_at, sc.cursor, sc.record_id
                        ) AS replacement_cursor
                  FROM sync_change AS sc
                  LEFT JOIN positive_max
                    ON positive_max.owner_id = sc.owner_id
                  WHERE sc.cursor <= 0
                )
                UPDATE sync_change AS sc
                SET cursor = invalid_cursor.replacement_cursor
                FROM invalid_cursor
                WHERE sc.owner_id = invalid_cursor.owner_id
                  AND sc.cursor = invalid_cursor.cursor;

                INSERT INTO sync_clock (owner_id, cursor)
                SELECT owner_id, max(cursor)
                FROM sync_change
                GROUP BY owner_id
                ON CONFLICT (owner_id) DO UPDATE
                SET cursor = GREATEST(sync_clock.cursor, excluded.cursor);

                ALTER TABLE sync_clock
                  ALTER COLUMN cursor SET DEFAULT 0;
                ALTER TABLE sync_clock
                  ADD CONSTRAINT sync_clock_cursor_check CHECK (cursor >= 0);
                ALTER TABLE sync_record
                  ADD CONSTRAINT sync_record_version_check CHECK (version > 0);
                ALTER TABLE sync_change
                  ADD CONSTRAINT sync_change_cursor_check CHECK (cursor > 0);
                ALTER TABLE sync_change
                  ADD CONSTRAINT sync_change_version_check CHECK (version > 0);
                ALTER TABLE sync_change
                  ADD CONSTRAINT sync_change_operation_check
                  CHECK (operation IN ('put', 'delete'));
                "#,
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if manager.get_database_backend() != DatabaseBackend::Postgres {
            return Ok(());
        }

        let cockroachdb = is_cockroachdb(manager).await?;

        manager
            .get_connection()
            .execute_unprepared(
                r#"
                ALTER TABLE sync_change
                  DROP CONSTRAINT sync_change_operation_check;
                ALTER TABLE sync_change
                  DROP CONSTRAINT sync_change_version_check;
                ALTER TABLE sync_change
                  DROP CONSTRAINT sync_change_cursor_check;
                ALTER TABLE sync_record
                  DROP CONSTRAINT sync_record_version_check;
                ALTER TABLE sync_clock
                  DROP CONSTRAINT sync_clock_cursor_check;
                ALTER TABLE sync_clock
                  ALTER COLUMN cursor DROP DEFAULT;
                "#,
            )
            .await?;

        if !cockroachdb {
            manager
                .get_connection()
                .execute_unprepared(
                    r#"
                    ALTER TABLE sync_change
                      ALTER COLUMN operation TYPE varchar USING operation::varchar;
                    "#,
                )
                .await?;
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl MigrationTrait for AddSessionRefreshLease {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for column in [
            ColumnDef::new(WebSession::RefreshLeaseId).uuid(),
            ColumnDef::new(WebSession::RefreshLeaseExpiresAt).timestamp_with_time_zone(),
        ] {
            manager
                .alter_table(
                    Table::alter()
                        .table(WebSession::Table)
                        .add_column(column)
                        .to_owned(),
                )
                .await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for column in [
            WebSession::RefreshLeaseExpiresAt,
            WebSession::RefreshLeaseId,
        ] {
            manager
                .alter_table(
                    Table::alter()
                        .table(WebSession::Table)
                        .drop_column(column)
                        .to_owned(),
                )
                .await?;
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl MigrationTrait for HardenSessionRevocationIdentity {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for column in [
            ColumnDef::new(WebSession::RevocationAbandonedAt).timestamp_with_time_zone(),
            ColumnDef::new(WebSession::RevocationFailureKind).string(),
        ] {
            manager
                .alter_table(
                    Table::alter()
                        .table(WebSession::Table)
                        .add_column(column)
                        .to_owned(),
                )
                .await?;
        }
        manager
            .create_index(
                Index::create()
                    .name("web_session_supabase_revocation_idx")
                    .table(WebSession::Table)
                    .col(WebSession::SupabaseSessionId)
                    .col(WebSession::RevokedAt)
                    .to_owned(),
            )
            .await?;
        if manager.get_database_backend() == DatabaseBackend::Postgres {
            manager
                .get_connection()
                .execute_unprepared(
                    r#"
                    ALTER TABLE web_session ENABLE ROW LEVEL SECURITY;
                    ALTER TABLE web_session FORCE ROW LEVEL SECURITY;
                    CREATE POLICY web_session_process_boundary ON web_session
                      USING (
                        current_user = 'canonical_web_server'
                        OR (
                          current_user = 'canonical_session_revoker'
                          AND current_setting('canonical.system_task', true) = 'session_revocation'
                        )
                      )
                      WITH CHECK (
                        current_user = 'canonical_web_server'
                        OR (
                          current_user = 'canonical_session_revoker'
                          AND current_setting('canonical.system_task', true) = 'session_revocation'
                        )
                      );
                    "#,
                )
                .await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if manager.get_database_backend() == DatabaseBackend::Postgres {
            manager
                .get_connection()
                .execute_unprepared(
                    r#"
                    DROP POLICY IF EXISTS web_session_process_boundary ON web_session;
                    ALTER TABLE web_session NO FORCE ROW LEVEL SECURITY;
                    ALTER TABLE web_session DISABLE ROW LEVEL SECURITY;
                    "#,
                )
                .await?;
        }
        manager
            .drop_index(
                Index::drop()
                    .name("web_session_supabase_revocation_idx")
                    .table(WebSession::Table)
                    .to_owned(),
            )
            .await?;
        for column in [
            WebSession::RevocationFailureKind,
            WebSession::RevocationAbandonedAt,
        ] {
            manager
                .alter_table(
                    Table::alter()
                        .table(WebSession::Table)
                        .drop_column(column)
                        .to_owned(),
                )
                .await?;
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl MigrationTrait for AddEngagements {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(AuditEngagement::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(AuditEngagement::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(AuditEngagement::OwnerId).uuid().not_null())
                    .col(ColumnDef::new(AuditEngagement::Company).string().not_null())
                    .col(
                        // The enum values are also enforced in the handlers so
                        // SQLite deployments get the same protection.
                        // text (not varchar): text-column check constraints
                        // round-trip Postgres deparsing byte-stably, which the
                        // dpm declarative-schema gate depends on.
                        ColumnDef::new(AuditEngagement::Framework)
                            .text()
                            .not_null()
                            .check(Expr::col(AuditEngagement::Framework).is_in([
                                "soc2",
                                "fedramp",
                                "hipaa",
                                "iso_27001",
                                "pci_dss",
                                "gdpr",
                            ])),
                    )
                    .col(
                        ColumnDef::new(AuditEngagement::Status)
                            .text()
                            .not_null()
                            .check(Expr::col(AuditEngagement::Status).is_in([
                                "scoping",
                                "remediation",
                                "in_audit",
                                "complete",
                            ])),
                    )
                    .col(
                        ColumnDef::new(AuditEngagement::OpenedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(ColumnDef::new(AuditEngagement::TargetReportDate).date())
                    .col(
                        ColumnDef::new(AuditEngagement::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("audit_engagement_owner_idx")
                    .table(AuditEngagement::Table)
                    .col(AuditEngagement::OwnerId)
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("audit_engagement_owner_status_idx")
                    .table(AuditEngagement::Table)
                    .col(AuditEngagement::OwnerId)
                    .col(AuditEngagement::Status)
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(EngagementNote::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(EngagementNote::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(EngagementNote::EngagementId)
                            .uuid()
                            .not_null(),
                    )
                    .col(ColumnDef::new(EngagementNote::OwnerId).uuid().not_null())
                    .col(ColumnDef::new(EngagementNote::Body).string().not_null())
                    .col(
                        ColumnDef::new(EngagementNote::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("engagement_note_engagement_fk")
                            .from(EngagementNote::Table, EngagementNote::EngagementId)
                            .to(AuditEngagement::Table, AuditEngagement::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("engagement_note_engagement_created_idx")
                    .table(EngagementNote::Table)
                    .col(EngagementNote::EngagementId)
                    .col(EngagementNote::CreatedAt)
                    .to_owned(),
            )
            .await?;
        // Supports the owner FK (auth.users cascade deletes) and owner scans.
        manager
            .create_index(
                Index::create()
                    .name("engagement_note_owner_idx")
                    .table(EngagementNote::Table)
                    .col(EngagementNote::OwnerId)
                    .to_owned(),
            )
            .await?;

        if manager.get_database_backend() == DatabaseBackend::Postgres {
            manager
                .get_connection()
                .execute_unprepared(
                    r#"
                    ALTER TABLE audit_engagement ENABLE ROW LEVEL SECURITY;
                    ALTER TABLE audit_engagement FORCE ROW LEVEL SECURITY;
                    ALTER TABLE engagement_note ENABLE ROW LEVEL SECURITY;
                    ALTER TABLE engagement_note FORCE ROW LEVEL SECURITY;

                    ALTER TABLE audit_engagement
                      ADD CONSTRAINT audit_engagement_auth_user_fk
                      FOREIGN KEY (owner_id) REFERENCES auth.users(id) ON DELETE CASCADE;
                    ALTER TABLE engagement_note
                      ADD CONSTRAINT engagement_note_auth_user_fk
                      FOREIGN KEY (owner_id) REFERENCES auth.users(id) ON DELETE CASCADE;

                    CREATE POLICY audit_engagement_owner ON audit_engagement
                      USING (owner_id = auth.uid()) WITH CHECK (owner_id = auth.uid());
                    CREATE POLICY engagement_note_owner ON engagement_note
                      USING (owner_id = auth.uid()) WITH CHECK (owner_id = auth.uid());
                    "#,
                )
                .await?;
        }

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(EngagementNote::Table)
                    .if_exists()
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(
                Table::drop()
                    .table(AuditEngagement::Table)
                    .if_exists()
                    .to_owned(),
            )
            .await?;
        Ok(())
    }
}

#[derive(DeriveIden)]
enum AuditEngagement {
    Table,
    Id,
    OwnerId,
    Company,
    Framework,
    Status,
    OpenedAt,
    TargetReportDate,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum EngagementNote {
    Table,
    Id,
    EngagementId,
    OwnerId,
    Body,
    CreatedAt,
}

#[derive(DeriveIden)]
enum AdminRoleAssignment {
    Table,
    UserId,
    Role,
    GrantedBy,
    GrantedAt,
    RevokedAt,
}

#[derive(DeriveIden)]
enum AdminAuditEvent {
    Table,
    Id,
    ActorId,
    Capability,
    TargetType,
    TargetId,
    RequestId,
    OccurredAt,
    Outcome,
    Metadata,
}

#[derive(DeriveIden)]
enum UserProfile {
    Table,
    UserId,
    Email,
    DisplayName,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum WebSession {
    Table,
    IdHash,
    UserId,
    Email,
    SupabaseSessionId,
    EncryptedAccessToken,
    EncryptedRefreshToken,
    AccessExpiresAt,
    RefreshLeaseId,
    RefreshLeaseExpiresAt,
    CsrfToken,
    CreatedAt,
    UpdatedAt,
    ExpiresAt,
    RevokedAt,
    RevocationPendingAt,
    RevocationNextAttemptAt,
    RevocationAttempts,
    UpstreamRevokedAt,
    RevocationAbandonedAt,
    RevocationFailureKind,
}

#[derive(DeriveIden)]
enum SyncRecord {
    Table,
    OwnerId,
    Collection,
    RecordId,
    Version,
    Payload,
    DeletedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum SyncChange {
    Table,
    OwnerId,
    Cursor,
    Collection,
    RecordId,
    Version,
    Operation,
    Payload,
    ChangedAt,
}

#[derive(DeriveIden)]
enum SyncClock {
    Table,
    OwnerId,
    Cursor,
}

#[derive(DeriveIden)]
enum SyncReceipt {
    Table,
    OwnerId,
    ClientId,
    MutationId,
    RequestHash,
    Result,
    CreatedAt,
}
