use sea_orm::DatabaseBackend;
use sea_orm_migration::prelude::*;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(Migration),
            Box::new(AddEngagements),
            Box::new(HardenSessions),
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
    CsrfToken,
    CreatedAt,
    UpdatedAt,
    ExpiresAt,
    RevokedAt,
    RevocationPendingAt,
    RevocationNextAttemptAt,
    RevocationAttempts,
    UpstreamRevokedAt,
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
