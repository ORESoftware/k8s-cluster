//! DB-backed test for `db::record_payment_event` (ignored by default; needs a
//! real DATABASE_URL). This is the ENTIRE webhook replay-dedup guarantee: the
//! first insert of a `(provider, event_id)` claims it (`true`); every later
//! insert of the same pair -- a provider retry, or a concurrent redelivery to
//! another replica -- is a no-op (`false`). All three handlers bail on `false`.
//!
//!   DATABASE_URL=... cargo test --test payment_events_db -- --ignored --nocapture
use athleto_app_rs::db::{self, PaymentProvider};
use sea_orm::{ConnectionTrait, DbBackend, Statement};

fn stmt(sql: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DbBackend::Postgres, sql, values)
}

#[tokio::test]
#[ignore]
async fn record_payment_event_claims_once_then_dedupes() {
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
    let conn = db::build_pool(&url).await.expect("pool");
    // A unique event id so repeated test runs don't collide.
    let event_id = format!("evt_test_{}", uuid::Uuid::new_v4().simple());
    let payload = serde_json::json!({ "type": "checkout.session.completed", "n": 1 });

    // First claim wins.
    let first = db::record_payment_event(&conn, PaymentProvider::Stripe, &event_id, &payload)
        .await
        .expect("insert ok");
    assert!(first, "first insert claims the event");

    // A retry with the SAME (provider, event_id) is deduped -- even with a
    // different payload body, the composite PK is what matters.
    let replay = db::record_payment_event(
        &conn,
        PaymentProvider::Stripe,
        &event_id,
        &serde_json::json!({ "type": "checkout.session.completed", "n": 2 }),
    )
    .await
    .expect("insert ok");
    assert!(!replay, "replay of the same event is a no-op");

    // The SAME event id under a DIFFERENT provider is a distinct event (the PK
    // is (provider, event_id)), so it claims independently.
    let other_provider =
        db::record_payment_event(&conn, PaymentProvider::Paypal, &event_id, &payload)
            .await
            .expect("insert ok");
    assert!(other_provider, "same id, different provider is its own event");

    // Exactly one Stripe row exists for the id.
    let n: i64 = conn
        .query_one(stmt(
            "SELECT count(*)::bigint AS n FROM payment_events WHERE provider = 'stripe' AND event_id = $1",
            vec![event_id.clone().into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get::<i64>("", "n")
        .unwrap();
    assert_eq!(n, 1, "the replay did not insert a second row");

    conn.execute(stmt(
        "DELETE FROM payment_events WHERE event_id = $1",
        vec![event_id.into()],
    ))
    .await
    .ok();
}
