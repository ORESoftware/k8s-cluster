//! DB-backed test for `db::record_payment` (ignored by default; needs a real
//! DATABASE_URL). This is the money-movement idempotency primitive that keeps a
//! settlement -- and its ledger post -- from being applied twice: it dedupes on
//! `UNIQUE (provider, provider_ref)`. The first call for a `provider_ref`
//! inserts and returns `true` (settle_order posts to the ledger only when it's
//! `true`); a second call for the same pair updates the existing row's status
//! and returns `false`; a different `provider_ref`, or the same ref under a
//! different provider, is its own payment.
//!
//!   DATABASE_URL=... cargo test --test record_payment_db -- --ignored --nocapture
use athleto_app_rs::db::{self, PaymentKind, PaymentProvider, PaymentStatus};
use sea_orm::{ConnectionTrait, DbBackend, Statement};
use uuid::Uuid;

fn stmt(sql: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DbBackend::Postgres, sql, values)
}

#[tokio::test]
#[ignore]
async fn record_payment_is_idempotent_on_provider_ref() {
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
    let conn = db::build_pool(&url).await.expect("pool");
    let user = Uuid::new_v4();
    // order_id = None so we don't need a real order row; the dedup key is
    // (provider, provider_ref), independent of the order.
    let reference = format!("pi_test_{}", Uuid::new_v4().simple());

    // First settlement of this provider_ref: newly recorded.
    let first = db::record_payment(
        &conn,
        None,
        user,
        PaymentProvider::Stripe,
        PaymentKind::Charge,
        &reference,
        1000,
        PaymentStatus::Paid,
    )
    .await
    .expect("insert ok");
    assert!(first, "first record for a provider_ref is newly recorded");

    // Same (provider, provider_ref) again -- a webhook + return-URL double
    // settlement -- is NOT newly recorded (so the ledger post is not repeated).
    let second = db::record_payment(
        &conn,
        None,
        user,
        PaymentProvider::Stripe,
        PaymentKind::Charge,
        &reference,
        1000,
        PaymentStatus::Paid,
    )
    .await
    .expect("insert ok");
    assert!(!second, "same provider_ref is a dedup, not a new payment");

    // Exactly one Stripe row for this ref.
    let n: i64 = conn
        .query_one(stmt(
            "SELECT count(*)::bigint AS n FROM payments WHERE provider = 'stripe' AND provider_ref = $1",
            vec![reference.clone().into()],
        ))
        .await.unwrap().unwrap().try_get::<i64>("", "n").unwrap();
    assert_eq!(n, 1, "the second settlement did not insert a duplicate row");

    // A DIFFERENT provider_ref is its own payment.
    let other_ref = format!("pi_test_{}", Uuid::new_v4().simple());
    assert!(
        db::record_payment(
            &conn,
            None,
            user,
            PaymentProvider::Stripe,
            PaymentKind::Charge,
            &other_ref,
            500,
            PaymentStatus::Paid
        )
        .await
        .unwrap(),
        "a distinct provider_ref is newly recorded"
    );

    // The SAME ref under a DIFFERENT provider is distinct (dedup key is the pair).
    assert!(
        db::record_payment(
            &conn,
            None,
            user,
            PaymentProvider::Paypal,
            PaymentKind::Charge,
            &reference,
            1000,
            PaymentStatus::Paid
        )
        .await
        .unwrap(),
        "same ref, different provider is its own payment"
    );

    conn.execute(stmt(
        "DELETE FROM payments WHERE user_id = $1",
        vec![user.into()],
    ))
    .await
    .ok();
}
