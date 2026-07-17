//! t2v-migration — SeaORM schema migrations.
//!
//! Both servers call `Migrator::up` at boot (idempotent), so a fresh database
//! — Postgres in the cluster, SQLite for local dev/tests — self-provisions.

pub use sea_orm_migration::prelude::*;

mod m20260717_000001_create_tables;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![Box::new(m20260717_000001_create_tables::Migration)]
    }
}
