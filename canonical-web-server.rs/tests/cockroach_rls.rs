//! CockroachDB migration + row-level-security integration test.
//!
//! CockroachDB speaks the Postgres wire protocol and (since v25.2) supports
//! forced row-level security and policies, so the same SeaORM migrations that
//! target Supabase Postgres must apply cleanly and enforce the same
//! owner-isolation contract. Three deliberate differences from Postgres are
//! encoded here:
//!
//! - CockroachDB validates foreign keys with the *inserting* role's
//!   privileges (Postgres uses the referenced table owner's), so an app role
//!   needs SELECT on auth.users for owner-FK inserts to succeed.
//! - There is no LISTEN/NOTIFY, so the WebSocket invalidation backplane is a
//!   Postgres-only feature and is not exercised here; REST pull remains
//!   authoritative either way.
//! - CockroachDB does not implement the PostgreSQL SECURITY DEFINER function
//!   forms used by the admin capability/audit boundary. Those functions stay
//!   absent while the admin tables remain protected by forced RLS with no
//!   policies, so the boundary fails closed.
//!
//! Gated on TEST_COCKROACH_ADMIN_URL (e.g.
//! postgresql://root@127.0.0.1:26257/defaultdb?sslmode=disable) pointing at a
//! DISPOSABLE loopback single-node cluster.

use canonical_web_server::{db::begin_user_transaction, run_migrations};
use sea_orm::{ConnectionTrait, Database, DatabaseBackend, Statement};
use std::{env, error::Error, io};
use uuid::Uuid;

#[tokio::test]
async fn migrations_and_owner_rls_hold_on_cockroachdb() -> Result<(), Box<dyn Error>> {
    let Ok(admin_url) = env::var("TEST_COCKROACH_ADMIN_URL") else {
        eprintln!("skipping CockroachDB test; TEST_COCKROACH_ADMIN_URL is not set");
        return Ok(());
    };
    require_disposable_loopback_cluster(&admin_url)?;

    let suffix = Uuid::new_v4().simple().to_string();
    let database_name = format!("canonical_web_server_crdb_{suffix}");
    let app_role = format!("canonical_crdb_app_{suffix}");

    let admin = Database::connect(&admin_url).await?;
    admin
        .execute_unprepared(&format!("CREATE DATABASE {database_name}"))
        .await?;
    let privileged_url = database_url(&admin_url, &database_name, None)?;
    let privileged = Database::connect(&privileged_url).await?;

    // Supabase-shaped fixture: the auth schema and roles the migration
    // references. CockroachDB supports CREATE ROLE IF NOT EXISTS directly.
    privileged
        .execute_unprepared(
            r#"
            CREATE SCHEMA auth;
            CREATE TABLE auth.users (id uuid PRIMARY KEY);
            CREATE FUNCTION auth.uid() RETURNS uuid
              LANGUAGE sql STABLE
              AS $$
                SELECT nullif(current_setting('request.jwt.claim.sub', true), '')::uuid
              $$;
            CREATE ROLE IF NOT EXISTS anon NOLOGIN;
            CREATE ROLE IF NOT EXISTS authenticated NOLOGIN;
            "#,
        )
        .await?;

    // The full migration chain must apply, and re-applying must be a no-op.
    run_migrations(&privileged_url, 2).await?;
    run_migrations(&privileged_url, 2).await?;

    // Catalog shape: every customer owner policy exists, admin tables have no
    // customer policy, and RLS is enabled + forced everywhere.
    let policy_count = privileged
        .query_one_raw(Statement::from_string(
            DatabaseBackend::Postgres,
            "SELECT count(*)::bigint AS count FROM pg_policies WHERE schemaname = 'public'",
        ))
        .await?
        .ok_or_else(|| io::Error::other("policy count returned no row"))?
        .try_get::<i64>("", "count")?;
    assert_eq!(
        policy_count, 10,
        "expected nine owner policies plus the web-session process boundary"
    );

    let admin_policy_count = privileged
        .query_one_raw(Statement::from_string(
            DatabaseBackend::Postgres,
            r#"
            SELECT count(*)::bigint AS count
            FROM pg_policies
            WHERE schemaname = 'public'
              AND tablename IN ('admin_role_assignment', 'admin_audit_event')
            "#,
        ))
        .await?
        .ok_or_else(|| io::Error::other("admin policy count returned no row"))?
        .try_get::<i64>("", "count")?;
    assert_eq!(admin_policy_count, 0, "admin tables must fail closed");

    let admin_function_count = privileged
        .query_one_raw(Statement::from_string(
            DatabaseBackend::Postgres,
            r#"
            SELECT count(*)::bigint AS count
            FROM pg_proc AS proc
            JOIN pg_namespace AS namespace ON namespace.oid = proc.pronamespace
            WHERE namespace.nspname = 'public'
              AND proc.proname IN (
                'canonical_admin_has_capability',
                'canonical_admin_append_audit'
              )
            "#,
        ))
        .await?
        .ok_or_else(|| io::Error::other("admin function count returned no row"))?
        .try_get::<i64>("", "count")?;
    assert_eq!(
        admin_function_count, 0,
        "PostgreSQL-only admin functions must remain absent on CockroachDB"
    );

    let admin_fk_count = privileged
        .query_one_raw(Statement::from_string(
            DatabaseBackend::Postgres,
            r#"
            SELECT count(*)::bigint AS count
            FROM pg_constraint
            WHERE contype = 'f'
              AND conname IN (
                'admin_role_assignment_user_fk',
                'admin_role_assignment_granted_by_fk',
                'admin_audit_event_actor_fk'
              )
            "#,
        ))
        .await?
        .ok_or_else(|| io::Error::other("admin FK count returned no row"))?
        .try_get::<i64>("", "count")?;
    assert_eq!(admin_fk_count, 3, "admin auth-user FKs must be installed");

    let unforced = privileged
        .query_one_raw(Statement::from_string(
            DatabaseBackend::Postgres,
            r#"
            SELECT count(*)::bigint AS count
            FROM pg_class
            WHERE relname IN (
              'user_profile', 'sync_record', 'sync_clock',
              'sync_change', 'sync_receipt', 'audit_engagement',
              'engagement_note', 'admin_role_assignment', 'admin_audit_event',
              'web_session', 'canonical_context', 'compliance_quote'
            )
              AND NOT (relrowsecurity AND relforcerowsecurity)
            "#,
        ))
        .await?
        .ok_or_else(|| io::Error::other("rls flag query returned no row"))?
        .try_get::<i64>("", "count")?;
    assert_eq!(
        unforced, 0,
        "customer-data and admin-boundary tables must have forced RLS"
    );

    // Least-privilege app role. SELECT on auth.users is REQUIRED on
    // CockroachDB (unlike Postgres) because FK checks run with the inserting
    // role's privileges.
    privileged
        .execute_unprepared(&format!(
            r#"
            CREATE ROLE {app_role} LOGIN;
            GRANT USAGE ON SCHEMA public, auth TO {app_role};
            GRANT SELECT, INSERT, UPDATE, DELETE ON
              public.audit_engagement, public.engagement_note TO {app_role};
            GRANT SELECT ON auth.users TO {app_role};
            GRANT EXECUTE ON FUNCTION auth.uid() TO {app_role};
            "#
        ))
        .await?;

    let user_a = Uuid::new_v4();
    let user_b = Uuid::new_v4();
    let engagement_a = Uuid::new_v4();
    let engagement_b = Uuid::new_v4();
    privileged
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "INSERT INTO auth.users (id) VALUES ($1), ($2)",
            [user_a.into(), user_b.into()],
        ))
        .await?;
    privileged
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"
            INSERT INTO audit_engagement
              (id, owner_id, company, framework, status, opened_at, target_report_date, updated_at)
            VALUES
              ($1, $3, 'Own Co', 'soc2', 'scoping', now(), NULL, now()),
              ($2, $4, 'Other Co', 'hipaa', 'in_audit', now(), NULL, now())
            "#,
            [
                engagement_a.into(),
                engagement_b.into(),
                user_a.into(),
                user_b.into(),
            ],
        ))
        .await?;

    let runtime_url = database_url(&admin_url, &database_name, Some(&app_role))?;
    let runtime = Database::connect(&runtime_url).await?;

    for query in [
        "SELECT user_id FROM admin_role_assignment LIMIT 1",
        "SELECT id FROM admin_audit_event LIMIT 1",
        "SELECT canonical_admin_has_capability('audit.read')",
    ] {
        assert!(
            runtime
                .query_one_raw(Statement::from_string(DatabaseBackend::Postgres, query))
                .await
                .is_err(),
            "customer role unexpectedly crossed the admin boundary with: {query}"
        );
    }

    // Without an installed JWT context, auth.uid() is NULL and forced RLS
    // must hide every row.
    let blind_count = runtime
        .query_one_raw(Statement::from_string(
            DatabaseBackend::Postgres,
            "SELECT count(*)::bigint AS count FROM audit_engagement",
        ))
        .await?
        .ok_or_else(|| io::Error::other("count query returned no row"))?
        .try_get::<i64>("", "count")?;
    assert_eq!(blind_count, 0);

    // With user A's context: exactly A's row is visible.
    let transaction = begin_user_transaction(&runtime, user_a).await?;
    let visible = transaction
        .query_one_raw(Statement::from_string(
            DatabaseBackend::Postgres,
            "SELECT count(*)::bigint AS count FROM audit_engagement",
        ))
        .await?
        .ok_or_else(|| io::Error::other("count query returned no row"))?
        .try_get::<i64>("", "count")?;
    assert_eq!(visible, 1);

    // Cross-owner UPDATE silently matches nothing.
    let foreign_update = transaction
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "UPDATE audit_engagement SET status = 'complete' WHERE id = $1",
            [engagement_b.into()],
        ))
        .await?;
    assert_eq!(foreign_update.rows_affected(), 0);

    // Owner-consistent writes succeed: an engagement and a note.
    let engagement_c = Uuid::new_v4();
    transaction
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"
            INSERT INTO audit_engagement
              (id, owner_id, company, framework, status, opened_at, target_report_date, updated_at)
            VALUES ($1, $2, 'Runtime Co', 'gdpr', 'scoping', now(), NULL, now())
            "#,
            [engagement_c.into(), user_a.into()],
        ))
        .await?;
    transaction
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"
            INSERT INTO engagement_note (id, engagement_id, owner_id, body, created_at)
            VALUES ($1, $2, $3, 'crdb runtime note', now())
            "#,
            [Uuid::new_v4().into(), engagement_c.into(), user_a.into()],
        ))
        .await?;
    transaction.commit().await?;

    // WITH CHECK rejects rows claiming another owner.
    let spoof_transaction = begin_user_transaction(&runtime, user_a).await?;
    let spoofed_engagement = spoof_transaction
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"
            INSERT INTO audit_engagement
              (id, owner_id, company, framework, status, opened_at, target_report_date, updated_at)
            VALUES ($1, $2, 'Spoof Co', 'gdpr', 'scoping', now(), NULL, now())
            "#,
            [Uuid::new_v4().into(), user_b.into()],
        ))
        .await;
    assert!(spoofed_engagement.is_err());
    spoof_transaction.rollback().await?;

    let spoof_note_transaction = begin_user_transaction(&runtime, user_a).await?;
    let spoofed_note = spoof_note_transaction
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"
            INSERT INTO engagement_note (id, engagement_id, owner_id, body, created_at)
            VALUES ($1, $2, $3, 'spoofed owner', now())
            "#,
            [Uuid::new_v4().into(), engagement_c.into(), user_b.into()],
        ))
        .await;
    assert!(spoofed_note.is_err());
    spoof_note_transaction.rollback().await?;

    // Enum check constraints hold on CockroachDB too.
    let bad_framework = begin_user_transaction(&runtime, user_a).await?;
    let rejected = bad_framework
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"
            INSERT INTO audit_engagement
              (id, owner_id, company, framework, status, opened_at, target_report_date, updated_at)
            VALUES ($1, $2, 'Bad Co', 'warp9', 'scoping', now(), NULL, now())
            "#,
            [Uuid::new_v4().into(), user_a.into()],
        ))
        .await;
    assert!(rejected.is_err());
    bad_framework.rollback().await?;

    // The sync upgrade's invariants are enforced by CockroachDB, not merely
    // represented in the declarative PostgreSQL schema.
    let negative_clock = privileged
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "INSERT INTO sync_clock (owner_id, cursor) VALUES ($1, -1)",
            [user_a.into()],
        ))
        .await;
    assert!(negative_clock.is_err(), "negative sync clock was accepted");

    let non_positive_record_version = privileged
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"
            INSERT INTO sync_record (
              owner_id, collection, record_id, version, payload, deleted_at, updated_at
            ) VALUES ($1, 'engagements', $2, 0, '{}'::jsonb, NULL, now())
            "#,
            [user_a.into(), Uuid::new_v4().into()],
        ))
        .await;
    assert!(
        non_positive_record_version.is_err(),
        "non-positive sync-record version was accepted"
    );

    let non_positive_change_cursor = privileged
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"
            INSERT INTO sync_change (
              owner_id, cursor, collection, record_id, version,
              operation, payload, changed_at
            ) VALUES ($1, 0, 'engagements', $2, 1, 'put', '{}'::jsonb, now())
            "#,
            [user_a.into(), Uuid::new_v4().into()],
        ))
        .await;
    assert!(
        non_positive_change_cursor.is_err(),
        "non-positive sync-change cursor was accepted"
    );

    let non_positive_change_version = privileged
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"
            INSERT INTO sync_change (
              owner_id, cursor, collection, record_id, version,
              operation, payload, changed_at
            ) VALUES ($1, 1, 'engagements', $2, 0, 'put', '{}'::jsonb, now())
            "#,
            [user_a.into(), Uuid::new_v4().into()],
        ))
        .await;
    assert!(
        non_positive_change_version.is_err(),
        "non-positive sync-change version was accepted"
    );

    let invalid_change_operation = privileged
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"
            INSERT INTO sync_change (
              owner_id, cursor, collection, record_id, version,
              operation, payload, changed_at
            ) VALUES ($1, 2, 'engagements', $2, 1, 'patch', '{}'::jsonb, now())
            "#,
            [user_a.into(), Uuid::new_v4().into()],
        ))
        .await;
    assert!(
        invalid_change_operation.is_err(),
        "invalid sync-change operation was accepted"
    );

    // Deleting the auth user cascades through engagements to notes.
    privileged
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "DELETE FROM auth.users WHERE id = $1",
            [user_a.into()],
        ))
        .await?;
    let survivors = privileged
        .query_one_raw(Statement::from_string(
            DatabaseBackend::Postgres,
            r#"
            SELECT
              (SELECT count(*) FROM audit_engagement)::bigint
              + (SELECT count(*) FROM engagement_note)::bigint AS count
            "#,
        ))
        .await?
        .ok_or_else(|| io::Error::other("cascade count returned no row"))?
        .try_get::<i64>("", "count")?;
    assert_eq!(survivors, 1, "only user B's engagement should remain");

    runtime.close().await?;
    privileged.close().await?;
    admin
        .execute_unprepared(&format!("DROP DATABASE {database_name} CASCADE"))
        .await?;
    admin
        .execute_unprepared(&format!("DROP ROLE {app_role}"))
        .await?;
    admin.close().await?;

    Ok(())
}

fn database_url(
    admin_url: &str,
    database_name: &str,
    username: Option<&str>,
) -> Result<String, Box<dyn Error>> {
    let mut url = reqwest::Url::parse(admin_url)?;
    url.set_path(&format!("/{database_name}"));
    if let Some(username) = username {
        url.set_username(username)
            .map_err(|_| io::Error::other("invalid app username"))?;
    }
    Ok(url.into())
}

fn require_disposable_loopback_cluster(admin_url: &str) -> Result<(), Box<dyn Error>> {
    let url = reqwest::Url::parse(admin_url)?;
    let host = url.host_str().unwrap_or_default();
    let loopback = host == "localhost" || host == "127.0.0.1" || host == "::1";
    if !loopback || url.path() != "/defaultdb" {
        return Err(io::Error::other(
            "TEST_COCKROACH_ADMIN_URL must target the defaultdb database on loopback",
        )
        .into());
    }
    Ok(())
}
