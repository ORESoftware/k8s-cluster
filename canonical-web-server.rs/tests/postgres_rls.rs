use async_trait::async_trait;
use axum::{
    body::Body,
    http::{header, Request, StatusCode},
    response::Response,
    Router,
};
use canonical_session::SessionRevoker;
use canonical_web_server::{
    auth::{AuthProvider, AuthProviderError, AuthTokens, SessionService, SupabaseUser},
    build_app,
    config::Config,
    db::{
        begin_admin_transaction, begin_session_revocation_transaction, begin_user_transaction,
        verify_admin_database_role, verify_runtime_database_role,
        verify_session_revoker_database_role, AssuranceLevel,
    },
    run_migrations,
    ws::{Hub, POSTGRES_INVALIDATION_CHANNEL},
    AppState,
};
use chrono::{Duration as ChronoDuration, Utc};
use http_body_util::BodyExt;
use sea_orm::{
    sqlx::postgres::PgListener, ConnectOptions, ConnectionTrait, Database, DatabaseBackend,
    DatabaseConnection, DatabaseTransaction, Statement, TransactionTrait,
};
use std::{
    collections::HashSet,
    env,
    error::Error,
    io,
    path::PathBuf,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};
use tower::ServiceExt;
use uuid::Uuid;

const RUNTIME_ROLE: &str = "canonical_web_server";
const ADMIN_RUNTIME_ROLE: &str = "canonical_admin_server";
const SESSION_REVOKER_ROLE: &str = "canonical_session_revoker";
const BOOTSTRAP_SQL: &str = include_str!("../deploy/postgres/bootstrap_runtime_role.sql");
const ADMIN_BOOTSTRAP_SQL: &str = include_str!("../deploy/postgres/bootstrap_admin_role.sql");
const SESSION_REVOKER_BOOTSTRAP_SQL: &str =
    include_str!("../deploy/postgres/bootstrap_session_revoker_role.sql");

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
    if role_exists(&admin, ADMIN_RUNTIME_ROLE).await? {
        return Err(io::Error::other(format!(
            "{ADMIN_RUNTIME_ROLE} already exists; the RLS fixture requires a disposable cluster"
        ))
        .into());
    }
    if role_exists(&admin, SESSION_REVOKER_ROLE).await? {
        return Err(io::Error::other(format!(
            "{SESSION_REVOKER_ROLE} already exists; the RLS fixture requires a disposable cluster"
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
    privileged.execute_unprepared(ADMIN_BOOTSTRAP_SQL).await?;
    privileged.execute_unprepared(ADMIN_BOOTSTRAP_SQL).await?;
    privileged
        .execute_unprepared(SESSION_REVOKER_BOOTSTRAP_SQL)
        .await?;
    assert_bootstraps_reject_inherited_public_function_access(&privileged).await?;
    assert_bootstraps_reject_inherited_public_relation_access(&privileged).await?;
    assert_runtime_role_has_no_blanket_sequence_access(&privileged).await?;
    assert_role_cannot_be_delegated(&privileged, RUNTIME_ROLE, BOOTSTRAP_SQL).await?;
    assert_role_cannot_be_delegated(&privileged, ADMIN_RUNTIME_ROLE, ADMIN_BOOTSTRAP_SQL).await?;
    assert_role_cannot_be_delegated(
        &privileged,
        SESSION_REVOKER_ROLE,
        SESSION_REVOKER_BOOTSTRAP_SQL,
    )
    .await?;
    assert_role_cannot_own_unlisted_objects(
        &privileged,
        SESSION_REVOKER_ROLE,
        SESSION_REVOKER_BOOTSTRAP_SQL,
    )
    .await?;
    privileged
        .execute_unprepared(SESSION_REVOKER_BOOTSTRAP_SQL)
        .await?;

    let runtime_is_restricted = privileged
        .query_one_raw(Statement::from_string(
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
    let admin_fixture_password = format!("admin-rls-{}", Uuid::new_v4().simple());
    let revoker_fixture_password = format!("revoker-rls-{}", Uuid::new_v4().simple());
    privileged
        .execute_unprepared(&format!(
            "ALTER ROLE {RUNTIME_ROLE} PASSWORD '{fixture_password}'"
        ))
        .await?;
    privileged
        .execute_unprepared(&format!(
            "ALTER ROLE {ADMIN_RUNTIME_ROLE} PASSWORD '{admin_fixture_password}'"
        ))
        .await?;
    privileged
        .execute_unprepared(&format!(
            "ALTER ROLE {SESSION_REVOKER_ROLE} PASSWORD '{revoker_fixture_password}'"
        ))
        .await?;

    let user_a = Uuid::new_v4();
    let user_b = Uuid::new_v4();
    let record_a = Uuid::new_v4();
    let record_b = Uuid::new_v4();
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
            INSERT INTO admin_role_assignment
              (user_id, role, granted_by, granted_at, revoked_at)
            VALUES
              ($1, 'security_admin', $1, now(), NULL),
              ($2, 'support', $1, now(), NULL)
            "#,
            [user_a.into(), user_b.into()],
        ))
        .await?;
    privileged
        .execute_raw(Statement::from_sql_and_values(
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

    verify_runtime_database_role(&runtime).await?;
    assert!(verify_runtime_database_role(&privileged).await.is_err());
    for scoped_role in [RUNTIME_ROLE, ADMIN_RUNTIME_ROLE, SESSION_REVOKER_ROLE] {
        assert_set_role_cannot_impersonate_isolated_login(&privileged_url, scoped_role).await?;
    }
    assert_runtime_role_verifier_rejects_catalog_drift(&privileged, &runtime, &database_name)
        .await?;

    assert_backplane_commit_delivery(&runtime, &runtime_url, user_a).await?;

    assert_eq!(visible_record_count(&runtime).await?, 0);
    assert!(runtime
        .execute_unprepared("CREATE TABLE runtime_role_must_not_create (id integer)")
        .await
        .is_err());
    assert_customer_cannot_cross_admin_boundary(&runtime).await?;
    assert!(runtime
        .query_one_raw(Statement::from_string(
            DatabaseBackend::Postgres,
            "SELECT id FROM auth.users LIMIT 1",
        ))
        .await
        .is_err());
    assert!(runtime
        .execute_raw(Statement::from_sql_and_values(
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
        .query_one_raw(Statement::from_string(
            DatabaseBackend::Postgres,
            "SELECT count(*)::bigint AS count FROM sync_record",
        ))
        .await?
        .ok_or_else(|| io::Error::other("count query returned no row"))?
        .try_get::<i64>("", "count")?;
    assert_eq!(own_count, 1);

    let other_count = own_transaction
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT count(*)::bigint AS count FROM sync_record WHERE owner_id = $1",
            [user_b.into()],
        ))
        .await?
        .ok_or_else(|| io::Error::other("count query returned no row"))?
        .try_get::<i64>("", "count")?;
    assert_eq!(other_count, 0);

    let updated = own_transaction
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "UPDATE sync_record SET version = 2 WHERE record_id = $1",
            [record_a.into()],
        ))
        .await?;
    assert_eq!(updated.rows_affected(), 1);
    own_transaction.commit().await?;

    let cross_user_transaction = begin_user_transaction(&runtime, user_a).await?;
    let cross_user_insert = cross_user_transaction
        .execute_raw(Statement::from_sql_and_values(
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

    let engagement_transaction = begin_user_transaction(&runtime, user_a).await?;
    let visible_engagements = engagement_transaction
        .query_one_raw(Statement::from_string(
            DatabaseBackend::Postgres,
            "SELECT count(*)::bigint AS count FROM audit_engagement",
        ))
        .await?
        .ok_or_else(|| io::Error::other("engagement count returned no row"))?
        .try_get::<i64>("", "count")?;
    assert_eq!(visible_engagements, 1);

    let foreign_update = engagement_transaction
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "UPDATE audit_engagement SET status = 'complete' WHERE id = $1",
            [engagement_b.into()],
        ))
        .await?;
    assert_eq!(foreign_update.rows_affected(), 0);

    let own_note = engagement_transaction
        .execute_raw(Statement::from_sql_and_values(
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
        .execute_raw(Statement::from_sql_and_values(
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

    assert_page_routes_use_rls_context(
        &runtime,
        &runtime_url,
        user_a,
        user_b,
        engagement_a,
        engagement_b,
    )
    .await?;

    assert_admin_role_is_capability_scoped(
        &privileged,
        &admin_url,
        &database_name,
        &admin_fixture_password,
        user_a,
        user_b,
    )
    .await?;
    assert_session_revoker_is_scoped(
        &privileged,
        &admin_url,
        &database_name,
        &revoker_fixture_password,
        &runtime,
        user_a,
    )
    .await?;

    runtime.close().await?;
    privileged.close().await?;
    admin
        .execute_unprepared(&format!("DROP DATABASE {database_name}"))
        .await?;
    admin
        .execute_unprepared(&format!("DROP ROLE {RUNTIME_ROLE}"))
        .await?;
    admin
        .execute_unprepared(&format!("DROP ROLE {ADMIN_RUNTIME_ROLE}"))
        .await?;
    admin
        .execute_unprepared(&format!("DROP ROLE {SESSION_REVOKER_ROLE}"))
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

#[derive(Clone)]
struct UnusedAuth;

#[async_trait]
impl AuthProvider for UnusedAuth {
    async fn password_sign_in(
        &self,
        _email: &str,
        _password: &str,
    ) -> Result<AuthTokens, AuthProviderError> {
        Err(AuthProviderError::Unavailable)
    }

    async fn refresh(&self, _refresh_token: &str) -> Result<AuthTokens, AuthProviderError> {
        Err(AuthProviderError::Unavailable)
    }

    async fn user_for_token(&self, _access_token: &str) -> Result<SupabaseUser, AuthProviderError> {
        Err(AuthProviderError::Unavailable)
    }

    async fn sign_out(&self, _access_token: &str) -> Result<(), AuthProviderError> {
        Err(AuthProviderError::Unavailable)
    }
}

struct RevocationAuth {
    fail_sign_out: bool,
    sign_out_calls: AtomicUsize,
}

#[async_trait]
impl AuthProvider for RevocationAuth {
    async fn password_sign_in(
        &self,
        _email: &str,
        _password: &str,
    ) -> Result<AuthTokens, AuthProviderError> {
        Err(AuthProviderError::Unavailable)
    }

    async fn refresh(&self, _refresh_token: &str) -> Result<AuthTokens, AuthProviderError> {
        Err(AuthProviderError::Unavailable)
    }

    async fn user_for_token(&self, _access_token: &str) -> Result<SupabaseUser, AuthProviderError> {
        Err(AuthProviderError::Unavailable)
    }

    async fn sign_out(&self, _access_token: &str) -> Result<(), AuthProviderError> {
        self.sign_out_calls.fetch_add(1, Ordering::SeqCst);
        if self.fail_sign_out {
            Err(AuthProviderError::Unavailable)
        } else {
            Ok(())
        }
    }
}

async fn assert_page_routes_use_rls_context(
    runtime: &DatabaseConnection,
    runtime_url: &str,
    user_a: Uuid,
    user_b: Uuid,
    engagement_a: Uuid,
    engagement_b: Uuid,
) -> Result<(), Box<dyn Error>> {
    let config = Config {
        port: 8081,
        static_dir: PathBuf::from("test-fixtures/no-marketing-site"),
        app_asset_dir: PathBuf::from("client/dist"),
        database_url: runtime_url.to_string(),
        database_max_connections: 4,
        app_base_url: "http://localhost:8081".into(),
        allowed_origins: HashSet::from(["http://localhost:8081".into()]),
        session_cookie: "canonical_session".into(),
        cookie_secure: false,
        session_encryption_key: vec![7; 32],
        session_ttl: Duration::from_secs(30 * 24 * 60 * 60),
        login_rate_limit_attempts: 5,
        login_rate_limit_global_attempts: 500,
        login_rate_limit_window: Duration::from_secs(600),
        login_rate_limit_max_keys: 4_096,
        login_auth_max_concurrency: 16,
        bearer_auth_max_concurrency: 32,
        supabase_url: "http://localhost:9999".into(),
        supabase_publishable_key: "test-publishable-key".into(),
    };
    let state = AppState::new(config, runtime.clone(), Arc::new(UnusedAuth))?;
    let session_a = state
        .sessions
        .create(tokens_for(user_a, "a@example.com"))
        .await?;
    let session_b = state
        .sessions
        .create(tokens_for(user_b, "b@example.com"))
        .await?;
    let app = build_app(state);

    let (status, page_a) = page_get(&app, "/app/engagements", &session_a.raw_id).await;
    assert_eq!(status, StatusCode::OK);
    assert!(page_a.contains("Own Co"));
    assert!(!page_a.contains("Other Co"));

    let (status, page_b) = page_get(&app, "/app/engagements", &session_b.raw_id).await;
    assert_eq!(status, StatusCode::OK);
    assert!(page_b.contains("Other Co"));
    assert!(!page_b.contains("Own Co"));

    assert_eq!(
        page_get(
            &app,
            &format!("/app/engagements/{engagement_a}"),
            &session_b.raw_id,
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        page_get(
            &app,
            &format!("/app/engagements/{engagement_b}"),
            &session_a.raw_id,
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );

    let created = page_post(
        &app,
        "/app/engagements",
        &session_a.raw_id,
        format!(
            "csrf={}&company=Runtime%20Created&framework=fedramp&target_report_date=",
            session_a.csrf_token
        ),
        false,
    )
    .await;
    assert_eq!(created.status(), StatusCode::SEE_OTHER);
    let (_, page_a) = page_get(&app, "/app/engagements", &session_a.raw_id).await;
    assert!(page_a.contains("Runtime Created"));
    assert!(!page_a.contains("Other Co"));

    let updated = page_post(
        &app,
        &format!("/app/engagements/{engagement_a}/status"),
        &session_a.raw_id,
        format!("csrf={}&status=complete", session_a.csrf_token),
        true,
    )
    .await;
    assert_eq!(updated.status(), StatusCode::OK);
    assert!(response_text(updated).await.contains("Status: Complete"));

    let note = page_post(
        &app,
        &format!("/app/engagements/{engagement_a}/notes"),
        &session_a.raw_id,
        format!("csrf={}&body=RLS%20route%20note", session_a.csrf_token),
        true,
    )
    .await;
    assert_eq!(note.status(), StatusCode::OK);
    assert!(response_text(note).await.contains("RLS route note"));

    let (status, detail) = page_get(
        &app,
        &format!("/app/engagements/{engagement_a}"),
        &session_a.raw_id,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(detail.contains("Status: Complete"));
    assert!(detail.contains("RLS route note"));
    Ok(())
}

fn tokens_for(user_id: Uuid, email: &str) -> AuthTokens {
    AuthTokens {
        access_token: format!("access-{user_id}"),
        refresh_token: format!("refresh-{user_id}"),
        expires_at: Utc::now() + ChronoDuration::hours(1),
        user: SupabaseUser {
            id: user_id,
            email: Some(email.into()),
        },
    }
}

async fn page_get(app: &Router, path: &str, session: &str) -> (StatusCode, String) {
    let response = app
        .clone()
        .oneshot(
            Request::get(path)
                .header(header::COOKIE, format!("canonical_session={session}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    (status, response_text(response).await)
}

async fn page_post(app: &Router, path: &str, session: &str, body: String, htmx: bool) -> Response {
    let mut request = Request::post(path)
        .header(header::ORIGIN, "http://localhost:8081")
        .header(header::COOKIE, format!("canonical_session={session}"))
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
    if htmx {
        request = request.header("hx-request", "true");
    }
    app.clone()
        .oneshot(request.body(Body::from(body)).unwrap())
        .await
        .unwrap()
}

async fn response_text(response: Response) -> String {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
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
    db.query_one_raw(Statement::from_string(
        DatabaseBackend::Postgres,
        "SELECT count(*)::bigint AS count FROM sync_record",
    ))
    .await?
    .ok_or_else(|| sea_orm::DbErr::Custom("count query returned no row".into()))?
    .try_get("", "count")
}

async fn assert_customer_cannot_cross_admin_boundary(
    runtime: &DatabaseConnection,
) -> Result<(), Box<dyn Error>> {
    for query in [
        "SELECT user_id FROM admin_role_assignment LIMIT 1",
        "SELECT id FROM admin_audit_event LIMIT 1",
        "SELECT canonical_admin_has_capability('audit.read')",
        "SELECT canonical_admin_append_audit(\
           gen_random_uuid(), 'audit.read', 'test', NULL, gen_random_uuid(), 'denied', '{}'::jsonb\
         )",
    ] {
        assert!(
            runtime
                .query_one_raw(Statement::from_string(DatabaseBackend::Postgres, query))
                .await
                .is_err(),
            "customer runtime unexpectedly crossed the admin boundary with: {query}"
        );
    }
    Ok(())
}

async fn assert_admin_role_is_capability_scoped(
    privileged: &DatabaseConnection,
    admin_url: &str,
    database_name: &str,
    password: &str,
    security_admin: Uuid,
    support_admin: Uuid,
) -> Result<(), Box<dyn Error>> {
    let restricted = privileged
        .query_one_raw(Statement::from_string(
            DatabaseBackend::Postgres,
            format!(
                r#"
                SELECT
                  NOT rolsuper
                  AND NOT rolbypassrls
                  AND NOT rolcreaterole
                  AND NOT rolcreatedb
                  AND NOT has_table_privilege(
                    '{ADMIN_RUNTIME_ROLE}',
                    'public.admin_role_assignment',
                    'SELECT,INSERT,UPDATE,DELETE'
                  )
                  AND NOT has_table_privilege(
                    '{ADMIN_RUNTIME_ROLE}',
                    'public.admin_audit_event',
                    'SELECT,INSERT,UPDATE,DELETE'
                  ) AS restricted
                FROM pg_roles
                WHERE rolname = '{ADMIN_RUNTIME_ROLE}'
                "#
            ),
        ))
        .await?
        .ok_or_else(|| io::Error::other("admin runtime role was not created"))?
        .try_get::<bool>("", "restricted")?;
    assert!(
        restricted,
        "admin runtime must be non-owner and function-only"
    );

    let runtime_url = database_url(
        admin_url,
        database_name,
        Some(ADMIN_RUNTIME_ROLE),
        Some(password),
    )?;
    let runtime = Database::connect(&runtime_url).await?;
    verify_admin_database_role(&runtime).await?;
    assert_scoped_role_attribute_drift_is_rejected(privileged, &runtime, ADMIN_RUNTIME_ROLE)
        .await?;

    assert!(
        begin_admin_transaction(&runtime, security_admin, AssuranceLevel::Aal1)
            .await
            .is_err(),
        "production admin transaction helper must reject AAL1"
    );

    for query in [
        "SELECT user_id FROM admin_role_assignment LIMIT 1",
        "SELECT id FROM admin_audit_event LIMIT 1",
        "SELECT id FROM audit_engagement LIMIT 1",
        "UPDATE admin_audit_event SET outcome = 'failed'",
        "DELETE FROM admin_audit_event",
    ] {
        assert!(
            runtime.execute_unprepared(query).await.is_err(),
            "admin runtime unexpectedly received direct table access with: {query}"
        );
    }

    let aal1 = runtime.begin().await?;
    install_admin_claims(&aal1, security_admin, "aal1").await?;
    assert!(!has_admin_capability(&aal1, "audit.read").await?);
    aal1.rollback().await?;

    let user_admin = Uuid::new_v4();
    let compliance_admin = Uuid::new_v4();
    privileged
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "INSERT INTO auth.users (id) VALUES ($1), ($2)",
            [user_admin.into(), compliance_admin.into()],
        ))
        .await?;
    privileged
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"
            INSERT INTO admin_role_assignment
              (user_id, role, granted_by, granted_at, revoked_at)
            VALUES
              ($1, 'user_admin', $3, now(), NULL),
              ($2, 'compliance_admin', $3, now(), NULL)
            "#,
            [
                user_admin.into(),
                compliance_admin.into(),
                security_admin.into(),
            ],
        ))
        .await?;

    for (actor, allowed, denied) in [
        (support_admin, "engagement.read", "role.manage"),
        (user_admin, "user.invite", "engagement.write"),
        (compliance_admin, "engagement.write", "user.disable"),
        (security_admin, "role.manage", "unknown.capability"),
    ] {
        let transaction = begin_admin_transaction(&runtime, actor, AssuranceLevel::Aal2).await?;
        assert!(has_admin_capability(&transaction, allowed).await?);
        assert!(!has_admin_capability(&transaction, denied).await?);
        transaction.rollback().await?;
    }

    privileged
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "UPDATE admin_role_assignment SET revoked_at = now() WHERE user_id = $1",
            [user_admin.into()],
        ))
        .await?;
    let revoked = begin_admin_transaction(&runtime, user_admin, AssuranceLevel::Aal2).await?;
    assert!(!has_admin_capability(&revoked, "user.invite").await?);
    revoked.rollback().await?;

    let security_event = Uuid::new_v4();
    let security_request = Uuid::new_v4();
    let security = begin_admin_transaction(&runtime, security_admin, AssuranceLevel::Aal2).await?;
    assert!(has_admin_capability(&security, "audit.read").await?);
    assert!(has_admin_capability(&security, "role.manage").await?);
    security
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"
            SELECT canonical_admin_append_audit(
              $1, 'role.manage', 'admin_role_assignment', $2, $3,
              'succeeded', '{"source":"postgres-rls-test"}'::jsonb
            ) AS event_id
            "#,
            [
                security_event.into(),
                support_admin.to_string().into(),
                security_request.into(),
            ],
        ))
        .await?
        .ok_or_else(|| io::Error::other("audit append returned no row"))?;
    security.commit().await?;

    let unsupported_success =
        begin_admin_transaction(&runtime, support_admin, AssuranceLevel::Aal2).await?;
    assert!(has_admin_capability(&unsupported_success, "user.read").await?);
    assert!(!has_admin_capability(&unsupported_success, "role.manage").await?);
    let rejected = unsupported_success
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"
            SELECT canonical_admin_append_audit(
              $1, 'role.manage', 'admin_role_assignment', NULL, $2,
              'succeeded', '{}'::jsonb
            )
            "#,
            [Uuid::new_v4().into(), Uuid::new_v4().into()],
        ))
        .await;
    assert!(rejected.is_err());
    unsupported_success.rollback().await?;

    let support_event = Uuid::new_v4();
    let support_request = Uuid::new_v4();
    let denied_attempt =
        begin_admin_transaction(&runtime, support_admin, AssuranceLevel::Aal2).await?;
    denied_attempt
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"
            SELECT canonical_admin_append_audit(
              $1, 'role.manage', 'admin_role_assignment', NULL, $2,
              'denied', '{}'::jsonb
            )
            "#,
            [support_event.into(), support_request.into()],
        ))
        .await?
        .ok_or_else(|| io::Error::other("denied audit append returned no row"))?;
    denied_attempt.commit().await?;

    let recorded = privileged
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"
            SELECT
              count(*)::bigint AS event_count,
              count(*) FILTER (
                WHERE id = $1 AND actor_id = $2 AND request_id = $3
                  AND capability = 'role.manage' AND outcome = 'succeeded'
              )::bigint AS security_event_count,
              count(*) FILTER (
                WHERE id = $4 AND actor_id = $5 AND request_id = $6
                  AND capability = 'role.manage' AND outcome = 'denied'
              )::bigint AS support_event_count
            FROM admin_audit_event
            "#,
            [
                security_event.into(),
                security_admin.into(),
                security_request.into(),
                support_event.into(),
                support_admin.into(),
                support_request.into(),
            ],
        ))
        .await?
        .ok_or_else(|| io::Error::other("audit verification returned no row"))?;
    assert_eq!(recorded.try_get::<i64>("", "event_count")?, 2);
    assert_eq!(recorded.try_get::<i64>("", "security_event_count")?, 1);
    assert_eq!(recorded.try_get::<i64>("", "support_event_count")?, 1);

    runtime.close().await?;
    Ok(())
}

async fn install_admin_claims(
    transaction: &DatabaseTransaction,
    actor_id: Uuid,
    aal: &str,
) -> Result<(), sea_orm::DbErr> {
    let claims = serde_json::json!({
        "sub": actor_id,
        "role": "authenticated",
        "aal": aal,
    })
    .to_string();
    transaction
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            r#"
            SELECT
              set_config('request.jwt.claim.sub', $1, true),
              set_config('request.jwt.claims', $2, true),
              set_config('request.jwt.claim.role', 'authenticated', true)
            "#,
            [actor_id.to_string().into(), claims.into()],
        ))
        .await?;
    Ok(())
}

async fn has_admin_capability(
    transaction: &DatabaseTransaction,
    capability: &str,
) -> Result<bool, sea_orm::DbErr> {
    transaction
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT canonical_admin_has_capability($1) AS allowed",
            [capability.into()],
        ))
        .await?
        .ok_or_else(|| sea_orm::DbErr::Custom("capability query returned no row".into()))?
        .try_get("", "allowed")
}

async fn assert_session_revoker_is_scoped(
    privileged: &DatabaseConnection,
    admin_url: &str,
    database_name: &str,
    password: &str,
    customer_runtime: &DatabaseConnection,
    user_id: Uuid,
) -> Result<(), Box<dyn Error>> {
    let restricted = privileged
        .query_one_raw(Statement::from_string(
            DatabaseBackend::Postgres,
            format!(
                r#"
                SELECT
                  NOT rolsuper
                  AND NOT rolbypassrls
                  AND NOT rolcreaterole
                  AND NOT rolcreatedb
                  AND has_table_privilege(
                    '{SESSION_REVOKER_ROLE}', 'public.web_session', 'SELECT,UPDATE,DELETE'
                  )
                  AND NOT has_table_privilege(
                    '{SESSION_REVOKER_ROLE}', 'public.web_session', 'INSERT'
                  )
                  AND NOT has_table_privilege(
                    '{SESSION_REVOKER_ROLE}', 'public.user_profile', 'SELECT,INSERT,UPDATE,DELETE'
                  )
                  AND NOT has_table_privilege(
                    '{SESSION_REVOKER_ROLE}', 'public.sync_record', 'SELECT,INSERT,UPDATE,DELETE'
                  )
                  AND NOT has_table_privilege(
                    '{SESSION_REVOKER_ROLE}', 'public.admin_role_assignment', 'SELECT,INSERT,UPDATE,DELETE'
                  ) AS restricted
                FROM pg_roles
                WHERE rolname = '{SESSION_REVOKER_ROLE}'
                "#
            ),
        ))
        .await?
        .ok_or_else(|| io::Error::other("session revoker role was not created"))?
        .try_get::<bool>("", "restricted")?;
    assert!(
        restricted,
        "session revoker must have only SELECT/UPDATE/DELETE on web_session"
    );

    assert!(
        begin_session_revocation_transaction(customer_runtime)
            .await
            .is_err(),
        "customer runtime must fail the revoker role check"
    );

    let revoker_url = database_url(
        admin_url,
        database_name,
        Some(SESSION_REVOKER_ROLE),
        Some(password),
    )?;
    let revoker = Database::connect(&revoker_url).await?;
    verify_session_revoker_database_role(&revoker).await?;
    assert_scoped_role_attribute_drift_is_rejected(privileged, &revoker, SESSION_REVOKER_ROLE)
        .await?;

    let enqueue_auth = Arc::new(RevocationAuth {
        fail_sign_out: true,
        sign_out_calls: AtomicUsize::new(0),
    });
    let sessions = SessionService::new(
        customer_runtime.clone(),
        enqueue_auth.clone(),
        &[7; 32],
        Duration::from_secs(3_600),
    )?;
    let created = sessions
        .create(tokens_for(user_id, "revoker@example.com"))
        .await?;
    let session_hash = created
        .context
        .session_hash
        .ok_or_else(|| io::Error::other("created session had no hash"))?;
    sessions.revoke(&created.raw_id).await?;
    assert_eq!(enqueue_auth.sign_out_calls.load(Ordering::SeqCst), 1);
    customer_runtime
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "UPDATE web_session SET revocation_next_attempt_at = now() - interval '1 second' WHERE id_hash = $1",
            [session_hash.clone().into()],
        ))
        .await?;

    let direct_visible = revoker
        .query_one_raw(Statement::from_string(
            DatabaseBackend::Postgres,
            "SELECT count(*)::bigint AS count FROM web_session",
        ))
        .await?
        .ok_or_else(|| io::Error::other("direct revoker count returned no row"))?
        .try_get::<i64>("", "count")?;
    assert_eq!(
        direct_visible, 0,
        "revoker DML/reads must be mediated by the tagged transaction"
    );

    let transaction = begin_session_revocation_transaction(&revoker).await?;
    let task = transaction
        .query_one_raw(Statement::from_string(
            DatabaseBackend::Postgres,
            "SELECT current_setting('canonical.system_task', true) AS task",
        ))
        .await?
        .ok_or_else(|| io::Error::other("revoker task setting returned no row"))?
        .try_get::<String>("", "task")?;
    assert_eq!(task, "session_revocation");
    let tagged_visible = transaction
        .query_one_raw(Statement::from_string(
            DatabaseBackend::Postgres,
            "SELECT count(*)::bigint AS count FROM web_session",
        ))
        .await?
        .ok_or_else(|| io::Error::other("tagged revoker count returned no row"))?
        .try_get::<i64>("", "count")?;
    assert!(tagged_visible > 0);
    transaction.commit().await?;

    let task_after_commit = revoker
        .query_one_raw(Statement::from_string(
            DatabaseBackend::Postgres,
            "SELECT current_setting('canonical.system_task', true) AS task",
        ))
        .await?
        .ok_or_else(|| io::Error::other("post-commit task query returned no row"))?
        .try_get::<Option<String>>("", "task")?;
    assert_ne!(task_after_commit.as_deref(), Some("session_revocation"));

    let success_auth = Arc::new(RevocationAuth {
        fail_sign_out: false,
        sign_out_calls: AtomicUsize::new(0),
    });
    let worker_a = SessionRevoker::new(revoker.clone(), success_auth.clone(), &[7; 32])?;
    let worker_b = SessionRevoker::new(revoker.clone(), success_auth.clone(), &[7; 32])?;
    worker_a.verify_database_role().await?;
    let (first, second) = tokio::join!(worker_a.reconcile_once(), worker_b.reconcile_once());
    first?;
    second?;
    assert_eq!(
        success_auth.sign_out_calls.load(Ordering::SeqCst),
        1,
        "the database lease must prevent duplicate upstream logout"
    );
    let reconciled = customer_runtime
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT upstream_revoked_at IS NOT NULL AS reconciled FROM web_session WHERE id_hash = $1",
            [session_hash.into()],
        ))
        .await?
        .ok_or_else(|| io::Error::other("reconciled session row was missing"))?
        .try_get::<bool>("", "reconciled")?;
    assert!(reconciled);

    assert!(revoker
        .execute_unprepared("CREATE TABLE revoker_must_not_create (id integer)")
        .await
        .is_err());
    let customer_transaction = begin_user_transaction(&revoker, user_id).await?;
    assert!(customer_transaction
        .query_one_raw(Statement::from_string(
            DatabaseBackend::Postgres,
            "SELECT user_id FROM user_profile LIMIT 1",
        ))
        .await
        .is_err());
    customer_transaction.rollback().await?;

    revoker.close().await?;
    Ok(())
}

async fn role_exists(db: &DatabaseConnection, role: &str) -> Result<bool, sea_orm::DbErr> {
    db.query_one_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = $1) AS present",
        [role.into()],
    ))
    .await?
    .ok_or_else(|| sea_orm::DbErr::Custom("role query returned no row".into()))?
    .try_get("", "present")
}

async fn assert_bootstraps_reject_inherited_public_function_access(
    privileged: &DatabaseConnection,
) -> Result<(), Box<dyn Error>> {
    let function = format!("canonical_public_probe_{}", Uuid::new_v4().simple());
    privileged
        .execute_unprepared(&format!(
            "CREATE FUNCTION public.{function}() RETURNS integer LANGUAGE sql AS 'SELECT 1'"
        ))
        .await?;

    for (role, bootstrap) in [
        (RUNTIME_ROLE, BOOTSTRAP_SQL),
        (ADMIN_RUNTIME_ROLE, ADMIN_BOOTSTRAP_SQL),
        (SESSION_REVOKER_ROLE, SESSION_REVOKER_BOOTSTRAP_SQL),
    ] {
        assert!(
            privileged.execute_unprepared(bootstrap).await.is_err(),
            "bootstrap accepted inherited PUBLIC EXECUTE for {role}"
        );
    }

    privileged
        .execute_unprepared(&format!(
            "REVOKE EXECUTE ON FUNCTION public.{function}() FROM PUBLIC"
        ))
        .await?;
    privileged.execute_unprepared(BOOTSTRAP_SQL).await?;
    privileged.execute_unprepared(ADMIN_BOOTSTRAP_SQL).await?;
    privileged
        .execute_unprepared(SESSION_REVOKER_BOOTSTRAP_SQL)
        .await?;
    privileged
        .execute_unprepared(&format!("DROP FUNCTION public.{function}()"))
        .await?;
    Ok(())
}

async fn assert_bootstraps_reject_inherited_public_relation_access(
    privileged: &DatabaseConnection,
) -> Result<(), Box<dyn Error>> {
    let table = format!("canonical_public_table_probe_{}", Uuid::new_v4().simple());
    let sequence = format!(
        "canonical_public_sequence_probe_{}",
        Uuid::new_v4().simple()
    );
    privileged
        .execute_unprepared(&format!("CREATE TABLE public.{table} (id integer)"))
        .await?;
    privileged
        .execute_unprepared(&format!("GRANT SELECT ON TABLE public.{table} TO PUBLIC"))
        .await?;

    for (role, bootstrap) in [
        (RUNTIME_ROLE, BOOTSTRAP_SQL),
        (ADMIN_RUNTIME_ROLE, ADMIN_BOOTSTRAP_SQL),
        (SESSION_REVOKER_ROLE, SESSION_REVOKER_BOOTSTRAP_SQL),
    ] {
        assert!(
            privileged.execute_unprepared(bootstrap).await.is_err(),
            "bootstrap accepted inherited PUBLIC table access for {role}"
        );
    }

    privileged
        .execute_unprepared(&format!(
            "REVOKE SELECT ON TABLE public.{table} FROM PUBLIC"
        ))
        .await?;
    privileged
        .execute_unprepared(&format!("CREATE SEQUENCE public.{sequence}"))
        .await?;
    privileged
        .execute_unprepared(&format!(
            "GRANT USAGE ON SEQUENCE public.{sequence} TO PUBLIC"
        ))
        .await?;

    for (role, bootstrap) in [
        (RUNTIME_ROLE, BOOTSTRAP_SQL),
        (ADMIN_RUNTIME_ROLE, ADMIN_BOOTSTRAP_SQL),
        (SESSION_REVOKER_ROLE, SESSION_REVOKER_BOOTSTRAP_SQL),
    ] {
        assert!(
            privileged.execute_unprepared(bootstrap).await.is_err(),
            "bootstrap accepted inherited PUBLIC sequence access for {role}"
        );
    }

    privileged
        .execute_unprepared(&format!(
            "REVOKE USAGE ON SEQUENCE public.{sequence} FROM PUBLIC"
        ))
        .await?;
    privileged.execute_unprepared(BOOTSTRAP_SQL).await?;
    privileged.execute_unprepared(ADMIN_BOOTSTRAP_SQL).await?;
    privileged
        .execute_unprepared(SESSION_REVOKER_BOOTSTRAP_SQL)
        .await?;
    privileged
        .execute_unprepared(&format!("DROP TABLE public.{table}"))
        .await?;
    privileged
        .execute_unprepared(&format!("DROP SEQUENCE public.{sequence}"))
        .await?;
    Ok(())
}

async fn assert_runtime_role_has_no_blanket_sequence_access(
    privileged: &DatabaseConnection,
) -> Result<(), Box<dyn Error>> {
    let sequence = format!("canonical_sequence_probe_{}", Uuid::new_v4().simple());
    privileged
        .execute_unprepared(&format!("CREATE SEQUENCE public.{sequence}"))
        .await?;
    privileged.execute_unprepared(BOOTSTRAP_SQL).await?;
    let has_access = privileged
        .query_one_raw(Statement::from_string(
            DatabaseBackend::Postgres,
            format!(
                "SELECT has_sequence_privilege('{RUNTIME_ROLE}', 'public.{sequence}', 'USAGE') OR has_sequence_privilege('{RUNTIME_ROLE}', 'public.{sequence}', 'SELECT') OR has_sequence_privilege('{RUNTIME_ROLE}', 'public.{sequence}', 'UPDATE') AS allowed"
            ),
        ))
        .await?
        .ok_or_else(|| io::Error::other("sequence privilege query returned no row"))?
        .try_get::<bool>("", "allowed")?;
    assert!(!has_access, "runtime role received blanket sequence access");
    privileged
        .execute_unprepared(&format!("DROP SEQUENCE public.{sequence}"))
        .await?;
    Ok(())
}

async fn assert_set_role_cannot_impersonate_isolated_login(
    privileged_url: &str,
    scoped_role: &str,
) -> Result<(), Box<dyn Error>> {
    let mut options = ConnectOptions::new(privileged_url);
    options.max_connections(1).min_connections(1);
    let impersonated_runtime = Database::connect(options).await?;
    impersonated_runtime
        .execute_unprepared(&format!("SET ROLE {scoped_role}"))
        .await?;
    let impersonation_was_rejected = match scoped_role {
        RUNTIME_ROLE => verify_runtime_database_role(&impersonated_runtime)
            .await
            .is_err(),
        ADMIN_RUNTIME_ROLE => verify_admin_database_role(&impersonated_runtime)
            .await
            .is_err(),
        SESSION_REVOKER_ROLE => verify_session_revoker_database_role(&impersonated_runtime)
            .await
            .is_err(),
        _ => unreachable!("test only accepts an isolated application role"),
    };

    impersonated_runtime
        .execute_unprepared("RESET ROLE")
        .await?;
    impersonated_runtime.close().await?;
    assert!(
        impersonation_was_rejected,
        "{scoped_role} verifier accepted SET ROLE from a privileged login"
    );
    Ok(())
}

async fn assert_scoped_role_attribute_drift_is_rejected(
    privileged: &DatabaseConnection,
    scoped_connection: &DatabaseConnection,
    scoped_role: &str,
) -> Result<(), Box<dyn Error>> {
    privileged
        .execute_unprepared(&format!("ALTER ROLE {scoped_role} BYPASSRLS"))
        .await?;
    let drift_was_rejected = match scoped_role {
        ADMIN_RUNTIME_ROLE => verify_admin_database_role(scoped_connection).await.is_err(),
        SESSION_REVOKER_ROLE => verify_session_revoker_database_role(scoped_connection)
            .await
            .is_err(),
        _ => unreachable!("test only accepts admin or revoker roles"),
    };
    privileged
        .execute_unprepared(&format!("ALTER ROLE {scoped_role} NOBYPASSRLS"))
        .await?;
    match scoped_role {
        ADMIN_RUNTIME_ROLE => verify_admin_database_role(scoped_connection).await?,
        SESSION_REVOKER_ROLE => verify_session_revoker_database_role(scoped_connection).await?,
        _ => unreachable!("test only accepts admin or revoker roles"),
    }
    assert!(
        drift_was_rejected,
        "{scoped_role} verifier accepted BYPASSRLS catalog drift"
    );
    Ok(())
}

async fn assert_runtime_role_verifier_rejects_catalog_drift(
    privileged: &DatabaseConnection,
    runtime: &DatabaseConnection,
    database_name: &str,
) -> Result<(), Box<dyn Error>> {
    assert_runtime_role_drift_is_rejected_then_restored(
        privileged,
        runtime,
        &format!("ALTER ROLE {RUNTIME_ROLE} BYPASSRLS"),
        &format!("ALTER ROLE {RUNTIME_ROLE} NOBYPASSRLS"),
        "BYPASSRLS attribute",
    )
    .await?;

    let membership_probe = format!("canonical_runtime_parent_{}", Uuid::new_v4().simple());
    privileged
        .execute_unprepared(&format!("CREATE ROLE {membership_probe} NOLOGIN"))
        .await?;
    assert_runtime_role_drift_is_rejected_then_restored(
        privileged,
        runtime,
        &format!("GRANT {membership_probe} TO {RUNTIME_ROLE}"),
        &format!("REVOKE {membership_probe} FROM {RUNTIME_ROLE}"),
        "parent role membership",
    )
    .await?;
    assert_runtime_role_drift_is_rejected_then_restored(
        privileged,
        runtime,
        &format!("GRANT {RUNTIME_ROLE} TO {membership_probe}"),
        &format!("REVOKE {RUNTIME_ROLE} FROM {membership_probe}"),
        "delegated role member",
    )
    .await?;
    privileged
        .execute_unprepared(&format!("DROP ROLE {membership_probe}"))
        .await?;

    assert_runtime_role_drift_is_rejected_then_restored(
        privileged,
        runtime,
        &format!("ALTER DATABASE {database_name} OWNER TO {RUNTIME_ROLE}"),
        &format!("ALTER DATABASE {database_name} OWNER TO CURRENT_USER"),
        "current database ownership",
    )
    .await?;
    assert_runtime_role_drift_is_rejected_then_restored(
        privileged,
        runtime,
        &format!("ALTER SCHEMA auth OWNER TO {RUNTIME_ROLE}"),
        "ALTER SCHEMA auth OWNER TO CURRENT_USER",
        "auth schema ownership",
    )
    .await?;

    let relation_probe = format!("canonical_runtime_relation_{}", Uuid::new_v4().simple());
    privileged
        .execute_unprepared(&format!(
            "CREATE TABLE public.{relation_probe} (id integer)"
        ))
        .await?;
    assert_runtime_role_drift_is_rejected_then_restored(
        privileged,
        runtime,
        &format!("ALTER TABLE public.{relation_probe} OWNER TO {RUNTIME_ROLE}"),
        &format!("ALTER TABLE public.{relation_probe} OWNER TO CURRENT_USER"),
        "public relation ownership",
    )
    .await?;
    privileged
        .execute_unprepared(&format!("DROP TABLE public.{relation_probe}"))
        .await?;

    let function_probe = format!("canonical_runtime_function_{}", Uuid::new_v4().simple());
    privileged
        .execute_unprepared(&format!(
            "CREATE FUNCTION public.{function_probe}() RETURNS integer LANGUAGE sql AS 'SELECT 1'"
        ))
        .await?;
    assert_runtime_role_drift_is_rejected_then_restored(
        privileged,
        runtime,
        &format!("ALTER FUNCTION public.{function_probe}() OWNER TO {RUNTIME_ROLE}"),
        &format!("ALTER FUNCTION public.{function_probe}() OWNER TO CURRENT_USER"),
        "public function ownership",
    )
    .await?;
    privileged
        .execute_unprepared(&format!("DROP FUNCTION public.{function_probe}()"))
        .await?;

    Ok(())
}

async fn assert_runtime_role_drift_is_rejected_then_restored(
    privileged: &DatabaseConnection,
    runtime: &DatabaseConnection,
    drift_sql: &str,
    restore_sql: &str,
    drift_description: &str,
) -> Result<(), Box<dyn Error>> {
    privileged.execute_unprepared(drift_sql).await?;
    let drift_was_rejected = verify_runtime_database_role(runtime).await.is_err();

    // Restore before asserting so a regression failure does not leave the
    // disposable fixture in a privileged state or mask later checks.
    privileged.execute_unprepared(restore_sql).await?;
    verify_runtime_database_role(runtime).await?;

    assert!(
        drift_was_rejected,
        "runtime role verifier accepted {drift_description}"
    );
    Ok(())
}

async fn assert_role_cannot_be_delegated(
    privileged: &DatabaseConnection,
    scoped_role: &str,
    bootstrap_sql: &str,
) -> Result<(), Box<dyn Error>> {
    let child = format!("canonical_scope_probe_{}", Uuid::new_v4().simple());
    privileged
        .execute_unprepared(&format!("CREATE ROLE {child} NOLOGIN"))
        .await?;
    privileged
        .execute_unprepared(&format!("GRANT {scoped_role} TO {child}"))
        .await?;
    assert!(
        privileged.execute_unprepared(bootstrap_sql).await.is_err(),
        "bootstrap accepted {scoped_role} with a delegated member"
    );
    privileged
        .execute_unprepared(&format!("REVOKE {scoped_role} FROM {child}"))
        .await?;
    privileged
        .execute_unprepared(&format!("DROP ROLE {child}"))
        .await?;
    Ok(())
}

async fn assert_role_cannot_own_unlisted_objects(
    privileged: &DatabaseConnection,
    scoped_role: &str,
    bootstrap_sql: &str,
) -> Result<(), Box<dyn Error>> {
    let table = format!("canonical_scope_probe_{}", Uuid::new_v4().simple());
    privileged
        .execute_unprepared(&format!("CREATE TABLE public.{table} (id integer)"))
        .await?;
    privileged
        .execute_unprepared(&format!(
            "ALTER TABLE public.{table} OWNER TO {scoped_role}"
        ))
        .await?;
    assert!(
        privileged.execute_unprepared(bootstrap_sql).await.is_err(),
        "bootstrap accepted {scoped_role} as owner of an unlisted object"
    );
    privileged
        .execute_unprepared(&format!("ALTER TABLE public.{table} OWNER TO CURRENT_USER"))
        .await?;
    privileged
        .execute_unprepared(&format!("DROP TABLE public.{table}"))
        .await?;
    Ok(())
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
