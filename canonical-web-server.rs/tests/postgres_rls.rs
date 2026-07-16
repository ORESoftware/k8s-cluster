use canonical_web_server::{
    db::begin_user_transaction,
    run_migrations,
    ws::{Hub, POSTGRES_INVALIDATION_CHANNEL},
};
use sea_orm::{
    sqlx::postgres::PgListener, ConnectionTrait, Database, DatabaseBackend, DatabaseConnection,
    Statement,
};
use std::{env, error::Error, io, time::Duration};
use uuid::Uuid;

const RUNTIME_ROLE: &str = "canonical_web_server";
const BOOTSTRAP_SQL: &str = include_str!("../deploy/postgres/bootstrap_runtime_role.sql");

#[tokio::test]
async fn runtime_role_enforces_supabase_rls_context() -> Result<(), Box<dyn Error>> {
    let Ok(admin_url) = env::var("TEST_POSTGRES_ADMIN_URL") else {
        eprintln!("skipping PostgreSQL RLS test; TEST_POSTGRES_ADMIN_URL is not set");
        return Ok(());
    };
    require_disposable_loopback_database(&admin_url)?;

    let admin = Database::connect(&admin_url).await?;
    if role_exists(&admin, RUNTIME_ROLE).await? {
        return Err(io::Error::other(format!(
            "{RUNTIME_ROLE} already exists; the RLS fixture requires a disposable cluster"
        ))
        .into());
    }

    let anon_existed = role_exists(&admin, "anon").await?;
    let authenticated_existed = role_exists(&admin, "authenticated").await?;
    admin
        .execute_unprepared(
            r#"
            DO $fixture$
            BEGIN
              IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'anon') THEN
                CREATE ROLE anon NOLOGIN;
              END IF;
              IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'authenticated') THEN
                CREATE ROLE authenticated NOLOGIN;
              END IF;
            END
            $fixture$;
            "#,
        )
        .await?;

    let database_name = format!("canonical_web_server_rls_{}", Uuid::new_v4().simple());
    admin
        .execute_unprepared(&format!("CREATE DATABASE {database_name}"))
        .await?;
    let privileged_url = database_url(&admin_url, &database_name, None, None)?;
    let privileged = Database::connect(&privileged_url).await?;

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
            "#,
        )
        .await?;

    run_migrations(&privileged_url, 2).await?;
    privileged.execute_unprepared(BOOTSTRAP_SQL).await?;
    // Running the bootstrap repeatedly must not widen privileges or fail.
    privileged.execute_unprepared(BOOTSTRAP_SQL).await?;

    let runtime_is_restricted = privileged
        .query_one(Statement::from_string(
            DatabaseBackend::Postgres,
            format!(
                r#"
                SELECT
                  NOT r.rolsuper
                  AND NOT r.rolbypassrls
                  AND NOT r.rolcreaterole
                  AND NOT r.rolcreatedb
                  AND pg_get_userbyid(c.relowner) <> r.rolname AS restricted
                FROM pg_roles r
                JOIN pg_class c ON c.relname = 'sync_record'
                WHERE r.rolname = '{RUNTIME_ROLE}'
                "#
            ),
        ))
        .await?
        .ok_or_else(|| io::Error::other("runtime role was not created"))?
        .try_get::<bool>("", "restricted")?;
    assert!(
        runtime_is_restricted,
        "runtime role must not own or bypass RLS"
    );

    let fixture_password = format!("rls-{}", Uuid::new_v4().simple());
    privileged
        .execute_unprepared(&format!(
            "ALTER ROLE {RUNTIME_ROLE} PASSWORD '{fixture_password}'"
        ))
        .await?;

    let user_a = Uuid::new_v4();
    let user_b = Uuid::new_v4();
    let record_a = Uuid::new_v4();
    let record_b = Uuid::new_v4();
    privileged
        .execute(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "INSERT INTO auth.users (id) VALUES ($1), ($2)",
            [user_a.into(), user_b.into()],
        ))
        .await?;
    privileged
        .execute(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"
            INSERT INTO sync_record
              (owner_id, collection, record_id, version, payload, deleted_at, updated_at)
            VALUES
              ($1, 'draft_note', $3, 1, '{"title":"A","body":"own"}'::jsonb, NULL, now()),
              ($2, 'draft_note', $4, 1, '{"title":"B","body":"other"}'::jsonb, NULL, now())
            "#,
            [
                user_a.into(),
                user_b.into(),
                record_a.into(),
                record_b.into(),
            ],
        ))
        .await?;

    let runtime_url = database_url(
        &admin_url,
        &database_name,
        Some(RUNTIME_ROLE),
        Some(&fixture_password),
    )?;
    let runtime = Database::connect(&runtime_url).await?;
    assert_backplane_commit_delivery(&runtime, &runtime_url, user_a).await?;

    assert_eq!(visible_record_count(&runtime).await?, 0);
    assert!(runtime
        .execute_unprepared("CREATE TABLE runtime_role_must_not_create (id integer)")
        .await
        .is_err());
    assert!(runtime
        .query_one(Statement::from_string(
            DatabaseBackend::Postgres,
            "SELECT id FROM auth.users LIMIT 1",
        ))
        .await
        .is_err());
    assert!(runtime
        .execute(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"
            INSERT INTO sync_record
              (owner_id, collection, record_id, version, payload, deleted_at, updated_at)
            VALUES ($1, 'draft_note', $2, 1, '{}'::jsonb, NULL, now())
            "#,
            [user_a.into(), Uuid::new_v4().into()],
        ))
        .await
        .is_err());

    let own_transaction = begin_user_transaction(&runtime, user_a).await?;
    let own_count = own_transaction
        .query_one(Statement::from_string(
            DatabaseBackend::Postgres,
            "SELECT count(*)::bigint AS count FROM sync_record",
        ))
        .await?
        .ok_or_else(|| io::Error::other("count query returned no row"))?
        .try_get::<i64>("", "count")?;
    assert_eq!(own_count, 1);

    let other_count = own_transaction
        .query_one(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT count(*)::bigint AS count FROM sync_record WHERE owner_id = $1",
            [user_b.into()],
        ))
        .await?
        .ok_or_else(|| io::Error::other("count query returned no row"))?
        .try_get::<i64>("", "count")?;
    assert_eq!(other_count, 0);

    let updated = own_transaction
        .execute(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "UPDATE sync_record SET version = 2 WHERE record_id = $1",
            [record_a.into()],
        ))
        .await?;
    assert_eq!(updated.rows_affected(), 1);
    own_transaction.commit().await?;

    let cross_user_transaction = begin_user_transaction(&runtime, user_a).await?;
    let cross_user_insert = cross_user_transaction
        .execute(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"
            INSERT INTO sync_record
              (owner_id, collection, record_id, version, payload, deleted_at, updated_at)
            VALUES ($1, 'draft_note', $2, 1, '{}'::jsonb, NULL, now())
            "#,
            [user_b.into(), Uuid::new_v4().into()],
        ))
        .await;
    assert!(cross_user_insert.is_err());
    cross_user_transaction.rollback().await?;

    // Engagement tables carry the same forced owner-scoped RLS contract.
    let engagement_a = Uuid::new_v4();
    let engagement_b = Uuid::new_v4();
    privileged
        .execute(Statement::from_sql_and_values(
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

    let engagement_transaction = begin_user_transaction(&runtime, user_a).await?;
    let visible_engagements = engagement_transaction
        .query_one(Statement::from_string(
            DatabaseBackend::Postgres,
            "SELECT count(*)::bigint AS count FROM audit_engagement",
        ))
        .await?
        .ok_or_else(|| io::Error::other("engagement count returned no row"))?
        .try_get::<i64>("", "count")?;
    assert_eq!(visible_engagements, 1);

    let foreign_update = engagement_transaction
        .execute(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "UPDATE audit_engagement SET status = 'complete' WHERE id = $1",
            [engagement_b.into()],
        ))
        .await?;
    assert_eq!(foreign_update.rows_affected(), 0);

    let own_note = engagement_transaction
        .execute(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"
            INSERT INTO engagement_note (id, engagement_id, owner_id, body, created_at)
            VALUES ($1, $2, $3, 'runtime note', now())
            "#,
            [Uuid::new_v4().into(), engagement_a.into(), user_a.into()],
        ))
        .await?;
    assert_eq!(own_note.rows_affected(), 1);
    engagement_transaction.commit().await?;

    // WITH CHECK must reject notes claiming another owner, even on a visible
    // engagement id.
    let foreign_note_transaction = begin_user_transaction(&runtime, user_a).await?;
    let foreign_note = foreign_note_transaction
        .execute(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"
            INSERT INTO engagement_note (id, engagement_id, owner_id, body, created_at)
            VALUES ($1, $2, $3, 'spoofed owner', now())
            "#,
            [Uuid::new_v4().into(), engagement_a.into(), user_b.into()],
        ))
        .await;
    assert!(foreign_note.is_err());
    foreign_note_transaction.rollback().await?;

    runtime.close().await?;
    privileged.close().await?;
    admin
        .execute_unprepared(&format!("DROP DATABASE {database_name}"))
        .await?;
    admin
        .execute_unprepared(&format!("DROP ROLE {RUNTIME_ROLE}"))
        .await?;
    if !anon_existed {
        admin.execute_unprepared("DROP ROLE anon").await?;
    }
    if !authenticated_existed {
        admin.execute_unprepared("DROP ROLE authenticated").await?;
    }
    admin.close().await?;

    Ok(())
}

async fn assert_backplane_commit_delivery(
    runtime: &DatabaseConnection,
    runtime_url: &str,
    owner_id: Uuid,
) -> Result<(), Box<dyn Error>> {
    let hub = Hub::new(8);
    let mut listener = PgListener::connect(runtime_url).await?;
    listener.listen(POSTGRES_INVALIDATION_CHANNEL).await?;

    let rolled_back = begin_user_transaction(runtime, owner_id).await?;
    hub.enqueue_postgres_invalidation(&rolled_back, owner_id, 6)
        .await?;
    rolled_back.rollback().await?;

    let committed = begin_user_transaction(runtime, owner_id).await?;
    hub.enqueue_postgres_invalidation(&committed, owner_id, 7)
        .await?;
    committed.commit().await?;

    let notification = tokio::time::timeout(Duration::from_secs(5), listener.recv()).await??;
    assert_eq!(notification.channel(), POSTGRES_INVALIDATION_CHANNEL);
    let payload: serde_json::Value = serde_json::from_str(notification.payload())?;
    assert_eq!(payload["version"], 1);
    assert_eq!(payload["ownerId"], owner_id.to_string());
    assert_eq!(payload["cursor"], 7);
    assert!(payload["sourceInstance"].as_str().is_some());

    listener.unlisten_all().await?;
    Ok(())
}

async fn visible_record_count(db: &DatabaseConnection) -> Result<i64, sea_orm::DbErr> {
    db.query_one(Statement::from_string(
        DatabaseBackend::Postgres,
        "SELECT count(*)::bigint AS count FROM sync_record",
    ))
    .await?
    .ok_or_else(|| sea_orm::DbErr::Custom("count query returned no row".into()))?
    .try_get("", "count")
}

async fn role_exists(db: &DatabaseConnection, role: &str) -> Result<bool, sea_orm::DbErr> {
    db.query_one(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = $1) AS present",
        [role.into()],
    ))
    .await?
    .ok_or_else(|| sea_orm::DbErr::Custom("role query returned no row".into()))?
    .try_get("", "present")
}

fn database_url(
    admin_url: &str,
    database_name: &str,
    username: Option<&str>,
    password: Option<&str>,
) -> Result<String, Box<dyn Error>> {
    let mut url = reqwest::Url::parse(admin_url)?;
    url.set_path(&format!("/{database_name}"));
    if let Some(username) = username {
        url.set_username(username)
            .map_err(|_| io::Error::other("invalid runtime username"))?;
    }
    if let Some(password) = password {
        url.set_password(Some(password))
            .map_err(|_| io::Error::other("invalid runtime password"))?;
    }
    Ok(url.into())
}

fn require_disposable_loopback_database(admin_url: &str) -> Result<(), Box<dyn Error>> {
    let url = reqwest::Url::parse(admin_url)?;
    let host = url.host_str().unwrap_or_default();
    let loopback = host == "localhost" || host == "127.0.0.1" || host == "::1";
    if !loopback || url.path() != "/postgres" {
        return Err(io::Error::other(
            "TEST_POSTGRES_ADMIN_URL must target the postgres database on loopback",
        )
        .into());
    }
    Ok(())
}
