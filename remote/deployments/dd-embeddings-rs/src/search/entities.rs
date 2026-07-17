//! Hand-written SeaORM entities for the service-owned search database.
//!
//! This database is the service's own (like `billing-server-rs`), so its
//! entities live here rather than in the generated `dd-pg-defs-sea-orm`
//! crate — the pg-defs contract/generators intentionally exclude pgvector
//! tables and cannot represent `vector`/`tsvector` columns.
//!
//! `search_documents` is modeled **partially** on purpose: the `embedding`
//! (pgvector) and `content_tsv` (generated tsvector) columns are omitted so
//! entity selects never try to decode types the driver has no codec for, and
//! entity inserts can never write the DB-maintained generated column. Every
//! query touching those columns goes through a parameterized
//! `sea_orm::Statement` in `search::SearchService` instead.

use sea_orm::entity::prelude::*;

pub mod search_documents {
    use super::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "search_documents")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: Uuid,
        pub collection: String,
        pub external_id: Option<String>,
        pub content: String,
        pub attributes: Json,
        pub created_at: DateTimeWithTimeZone,
        pub updated_at: DateTimeWithTimeZone,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod search_edges {
    use super::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "search_edges")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub src_id: Uuid,
        #[sea_orm(primary_key, auto_increment = false)]
        pub dst_id: Uuid,
        #[sea_orm(primary_key, auto_increment = false)]
        pub relation: String,
        pub weight: f64,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}
