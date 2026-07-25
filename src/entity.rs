//! SeaORM entities for the service-owned `threefa` Postgres schema.
//!
//! These models mirror the reviewed SQL migrations. The server deliberately
//! does not synchronize entities into DDL at runtime.

pub mod account {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "accounts", schema_name = "threefa")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: Uuid,
        #[sea_orm(unique)]
        pub username: Option<String>,
        pub auth_secret: Option<String>,
        pub shared_auth_user_id: Option<Uuid>,
        pub supabase_user_id: Option<Uuid>,
        pub email: Option<String>,
        pub created_at: TimeDateTimeWithTimeZone,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod device {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "devices", schema_name = "threefa")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: Uuid,
        pub account_id: Uuid,
        pub device_name: String,
        pub sync_token_hash: String,
        pub revoked: bool,
        pub created_at: TimeDateTimeWithTimeZone,
        pub last_seen_at: Option<TimeDateTimeWithTimeZone>,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod vault_blob {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "vault_blobs", schema_name = "threefa")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub account_id: Uuid,
        pub ciphertext: String,
        pub nonce: String,
        pub kdf_salt: String,
        pub kdf_params: Json,
        pub version: Json,
        pub updated_at: TimeDateTimeWithTimeZone,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use sea_orm::{EntityName, Iden, PrimaryKeyTrait};

    #[test]
    fn entities_target_only_the_service_owned_schema() {
        for (schema, table) in [
            (account::Entity.schema_name(), account::Entity.table_name()),
            (device::Entity.schema_name(), device::Entity.table_name()),
            (
                vault_blob::Entity.schema_name(),
                vault_blob::Entity.table_name(),
            ),
        ] {
            assert_eq!(schema, Some("threefa"));
            assert!(matches!(table, "accounts" | "devices" | "vault_blobs"));
        }
    }

    #[test]
    fn uuid_primary_keys_are_application_generated() {
        assert!(!account::PrimaryKey::auto_increment());
        assert!(!device::PrimaryKey::auto_increment());
        assert!(!vault_blob::PrimaryKey::auto_increment());
    }

    #[test]
    fn shared_auth_entity_and_reviewed_migration_stay_in_lockstep() {
        let migration = include_str!("../migrations/0004_shared_auth_identity.sql");
        for required in [
            "shared_auth_user_id UUID",
            "accounts_shared_auth_user_idx",
            "shared_auth_user_id IS NOT NULL",
            "accounts_identity_present",
        ] {
            assert!(migration.contains(required), "migration missing {required}");
        }
        assert_eq!(
            account::Column::SharedAuthUserId.to_string(),
            "shared_auth_user_id"
        );
    }
}
