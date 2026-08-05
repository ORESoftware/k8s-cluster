//! Stateful integration coverage. CI supplies disposable Postgres and Redis;
//! local runs skip cleanly unless the corresponding URLs are set.

use chrono::TimeDelta;
use shared_auth_server::cache::Cache;
use shared_auth_server::config::{DbConfig, RedisConfig};
use shared_auth_server::db::DbStore;
use shared_auth_server::session::{hash_otp, hashed_identifier, MagicLinkToken, RefreshToken};
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
        .create_session(
            reloaded.identity,
            &old_token.hash,
            expiry,
            None,
            1,
            &["password".into()],
        )
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

    let passwordless_email = format!("passwordless-{}@example.invalid", Uuid::new_v4());
    let identifier_hash = hashed_identifier(&passwordless_email);
    let otp_pepper = "integration-test-otp-pepper-is-at-least-32-bytes";
    let email_otp = MagicLinkToken::generate();
    assert!(db
        .prepare_magic_link(
            &passwordless_email,
            true,
            &email_otp.hash,
            &hash_otp(otp_pepper, &passwordless_email, &email_otp.otp),
            &identifier_hash,
            chrono::Utc::now().fixed_offset() + TimeDelta::minutes(15),
        )
        .await
        .unwrap());
    assert!(db
        .consume_email_otp(
            &identifier_hash,
            &hash_otp(otp_pepper, &passwordless_email, "000000"),
        )
        .await
        .is_err());
    let passwordless_identity = db
        .consume_email_otp(
            &identifier_hash,
            &hash_otp(otp_pepper, &passwordless_email, &email_otp.otp),
        )
        .await
        .unwrap();
    assert_eq!(
        passwordless_identity.email.as_deref(),
        Some(passwordless_email.as_str())
    );
    assert!(passwordless_identity.email_verified);
    assert_eq!(passwordless_identity.roles, vec!["user"]);
    assert!(db
        .consume_email_otp(
            &identifier_hash,
            &hash_otp(otp_pepper, &passwordless_email, &email_otp.otp),
        )
        .await
        .is_err());

    let link_email = format!("magic-link-{}@example.invalid", Uuid::new_v4());
    let magic_link = MagicLinkToken::generate();
    assert!(db
        .prepare_magic_link(
            &link_email,
            true,
            &magic_link.hash,
            &hash_otp(otp_pepper, &link_email, &magic_link.otp),
            &hashed_identifier(&link_email),
            chrono::Utc::now().fixed_offset() + TimeDelta::minutes(15),
        )
        .await
        .unwrap());
    let linked = db.consume_magic_link(&magic_link.hash).await.unwrap();
    assert!(linked.email_verified);
    assert!(db.consume_magic_link(&magic_link.hash).await.is_err());

    assert_eq!(
        db.phone_for_user(passwordless_identity.shared_user_id)
            .await
            .unwrap(),
        None
    );
    let sms_challenge = db
        .create_sms_challenge(
            passwordless_identity.shared_user_id,
            "+14155550100",
            chrono::Utc::now().fixed_offset() + TimeDelta::minutes(10),
        )
        .await
        .unwrap();
    assert_eq!(
        db.sms_challenge_phone(passwordless_identity.shared_user_id, sms_challenge)
            .await
            .unwrap(),
        "+14155550100"
    );
    db.complete_sms_challenge(passwordless_identity.shared_user_id, sms_challenge)
        .await
        .unwrap();
    assert_eq!(
        db.phone_for_user(passwordless_identity.shared_user_id)
            .await
            .unwrap()
            .as_deref(),
        Some("+14155550100")
    );
    assert!(db
        .sms_challenge_phone(passwordless_identity.shared_user_id, sms_challenge)
        .await
        .is_err());
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

#[tokio::test]
async fn mfa_sms_enrollment_rate_limit_denies_after_threshold() {
    // Locks the DEN-1311 toll-fraud policy that request_sms enforces before any
    // Twilio call: at most 3 SMS enrollment sends per destination number within
    // an hour, keyed on a hashed identifier (as enforce_limit does internally).
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
    let phone = shared_auth_server::session::hashed_identifier("+15550001234");
    for _ in 0..3 {
        assert!(cache.allow("mfa_sms_phone", &phone, 3, 3600).await.unwrap());
    }
    assert!(
        !cache.allow("mfa_sms_phone", &phone, 3, 3600).await.unwrap(),
        "the 4th SMS enrollment to one number within the window must be denied"
    );

    // A different destination number has an independent budget.
    let other = shared_auth_server::session::hashed_identifier("+15550009999");
    assert!(cache.allow("mfa_sms_phone", &other, 3, 3600).await.unwrap());
}
