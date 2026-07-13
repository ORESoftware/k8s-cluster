use sea_orm::DatabaseBackend;
use sea_orm_migration::prelude::*;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![Box::new(Migration)]
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
