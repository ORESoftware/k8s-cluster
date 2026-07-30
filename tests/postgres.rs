//! Stateful integration coverage. CI supplies disposable Postgres and Redis;
//! local runs skip cleanly unless the corresponding URLs are set.

use chrono::TimeDelta;
use shared_auth_server::cache::Cache;
use shared_auth_server::config::{DbConfig, RedisConfig};
use shared_auth_server::db::DbStore;
use shared_auth_server::session::RefreshToken;
use uuid::Uuid;

#[tokio::test]
async fn postgres_owns_identity_roles_and_rotating_sessions() {
    let Some(url) = std::env::var("AUTH_TEST_DATABASE_URL").ok() else {
        eprintln!("AUTH_TEST_DATABASE_URL unset; skipping stateful Postgres integration test");
        return;
    };
    let db = DbStore::connect(&DbConfig {
        url,
        max_connections: 2,
    })
    .await
    .unwrap();
    db.ping().await.unwrap();

    let email = format!("integration-{}@example.invalid", Uuid::new_v4());
    let password_hash = shared_auth_server::password::hash("correct horse battery staple".into())
        .await
        .unwrap();
    let local = db
        .create_local_user(&email, Some("Integration User"), &password_hash)
        .await
        .unwrap();
    assert_eq!(local.provider, "local");
    assert_eq!(local.roles, vec!["user"]);

    let loaded = db.local_credential(&email).await.unwrap().unwrap();
    assert_eq!(loaded.identity.shared_user_id, local.shared_user_id);
    assert!(shared_auth_server::password::verify(
        "correct horse battery staple".into(),
        Some(loaded.password_hash),
    )
    .await
    .unwrap());

    db.replace_roles(local.shared_user_id, &["admin".into(), "user".into()])
        .await
        .unwrap();
    let reloaded = db.local_credential(&email).await.unwrap().unwrap();
    assert_eq!(reloaded.identity.roles, vec!["admin", "user"]);

    let old_token = RefreshToken::generate();
    let expiry = chrono::Utc::now().fixed_offset() + TimeDelta::hours(1);
    let old_session = db
        .create_session(reloaded.identity, &old_token.hash, expiry, None)
        .await
        .unwrap();
    assert!(db.session_is_active(old_session.session_id).await.unwrap());

    // The model checker assumes single-use refresh rotation is atomic. Exercise
    // the real transaction with two contenders: exactly one may consume the
    // old token and the loser must observe an authorization failure.
    let left_replacement = RefreshToken::generate();
    let right_replacement = RefreshToken::generate();
    let (left, right) = tokio::join!(
        db.rotate_session(&old_token.hash, &left_replacement.hash, expiry),
        db.rotate_session(&old_token.hash, &right_replacement.hash, expiry),
    );
    assert_ne!(left.is_ok(), right.is_ok());
    let (rotated, replacement) = match (left, right) {
        (Ok(session), Err(_)) => (session, left_replacement),
        (Err(_), Ok(session)) => (session, right_replacement),
        _ => unreachable!("exactly one concurrent refresh must win"),
    };
    assert!(!db.session_is_active(old_session.session_id).await.unwrap());
    assert!(db.session_is_active(rotated.session_id).await.unwrap());
    assert!(db
        .rotate_session(&old_token.hash, &RefreshToken::generate().hash, expiry)
        .await
        .is_err());

    let revoked = db.revoke_by_refresh_hash(&replacement.hash).await.unwrap();
    assert_eq!(revoked, Some(rotated.session_id));
    assert!(!db.session_is_active(rotated.session_id).await.unwrap());

    let subject = Uuid::new_v4().to_string();
    let first = db
        .upsert_external_identity(
            "supabase",
            "integration-project",
            &subject,
            Some(email.clone()),
            true,
            serde_json::json!({ "source": "integration" }),
        )
        .await
        .unwrap();
    let second = db
        .upsert_external_identity(
            "supabase",
            "integration-project",
            &subject,
            Some(email),
            true,
            serde_json::json!({ "source": "retry" }),
        )
        .await
        .unwrap();
    assert_eq!(first.shared_user_id, second.shared_user_id);

    let event_id = Uuid::new_v4();
    assert!(db
        .record_webhook_event(event_id, "supabase", "identity.upsert", &old_token.hash)
        .await
        .unwrap());
    assert!(!db
        .record_webhook_event(event_id, "supabase", "identity.upsert", &old_token.hash)
        .await
        .unwrap());
}

#[tokio::test]
async fn redis_cache_is_disposable_and_uses_hashed_buckets() {
    let Some(url) = std::env::var("AUTH_TEST_REDIS_URL").ok() else {
        eprintln!("AUTH_TEST_REDIS_URL unset; skipping Redis integration test");
        return;
    };
    let cache = Cache::connect(&RedisConfig {
        url,
        key_prefix: format!("shared-auth:test:{}", Uuid::new_v4()),
    })
    .await
    .unwrap();
    cache.ping().await.unwrap();
    let bucket = shared_auth_server::session::hashed_identifier("person@example.invalid");
    assert!(cache.allow("login", &bucket, 2, 60).await.unwrap());
    assert!(cache.allow("login", &bucket, 2, 60).await.unwrap());
    assert!(!cache.allow("login", &bucket, 2, 60).await.unwrap());

    let session_id = Uuid::new_v4();
    assert!(!cache.is_revoked(session_id).await.unwrap());
    cache.mark_revoked(session_id, 60).await.unwrap();
    assert!(cache.is_revoked(session_id).await.unwrap());
}
