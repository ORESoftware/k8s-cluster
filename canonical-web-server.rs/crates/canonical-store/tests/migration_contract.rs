const MIGRATIONS: &str = include_str!("../src/migration.rs");
const DECLARATIVE_SCHEMA: &str = include_str!("../../../deploy/postgres/schema.sql");

#[test]
fn sync_invariants_have_an_upgrade_migration_and_declarative_contract() {
    assert!(MIGRATIONS.contains("m20260718_000006_harden_sync_invariants"));
    assert!(MIGRATIONS.contains("UPDATE sync_clock\n                SET cursor = 0"));
    assert!(MIGRATIONS.contains("WHERE operation NOT IN ('put', 'delete')"));
    assert!(MIGRATIONS.contains("ALTER COLUMN operation TYPE text"));
    assert!(MIGRATIONS.contains("INSERT INTO sync_clock (owner_id, cursor)"));
    assert!(MIGRATIONS.contains("ON CONFLICT (owner_id) DO UPDATE"));

    for constraint in [
        "sync_clock_cursor_check",
        "sync_record_version_check",
        "sync_change_cursor_check",
        "sync_change_operation_check",
        "sync_change_version_check",
    ] {
        assert!(
            MIGRATIONS.contains(&format!("ADD CONSTRAINT {constraint}")),
            "upgrade migration is missing {constraint}"
        );
        assert!(
            DECLARATIVE_SCHEMA.contains(&format!("CONSTRAINT {constraint}")),
            "declarative schema is missing {constraint}"
        );
    }

    assert!(DECLARATIVE_SCHEMA.contains("cursor bigint DEFAULT 0 NOT NULL"));
    assert!(DECLARATIVE_SCHEMA.contains("collection character varying NOT NULL"));
    assert!(DECLARATIVE_SCHEMA.contains("operation text NOT NULL"));
}
