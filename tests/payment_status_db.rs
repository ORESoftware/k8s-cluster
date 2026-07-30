//! DB-backed tests for `db::set_order_payment_status` (ignored by default; needs
//! a real DATABASE_URL). Two invariants on the settlement write:
//!
//!   1. `paid_at` is stamped exactly once (the first `Paid`), never moved by a
//!      replay -- the pre-existing idempotency guard.
//!   2. A `Paid` order NEVER regresses. Providers redeliver webhooks out of
//!      order and the customer's return-URL landing races the webhook, so a late
//!      `Processing`/`Pending` write must be ignored; only `Refunded` may
//!      supersede `Paid`. This is the guard added alongside the fiducia audit.
//!
//!   DATABASE_URL=... cargo test --test payment_status_db -- --ignored --nocapture
use athleto_app_rs::db::{self, PaymentStatus};
use sea_orm::{ConnectionTrait, DbBackend, Statement};
use uuid::Uuid;

fn stmt(sql: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DbBackend::Postgres, sql, values)
}

/// Insert a bare order for `user` and return its id. Payment starts `pending`.
async fn seed_order(conn: &sea_orm::DatabaseConnection, user: Uuid) -> Uuid {
    conn.query_one(stmt(
        "INSERT INTO orders (user_id, channel, ship_method, subtotal_cents, \
             shipping_cents, tax_cents, total_cents) \
         VALUES ($1, 'd2c_web', 'standard', 1000, 0, 0, 1000) RETURNING id",
        vec![user.into()],
    ))
    .await
    .unwrap()
    .unwrap()
    .try_get("", "id")
    .unwrap()
}

async fn payment_state(
    conn: &sea_orm::DatabaseConnection,
    order_id: Uuid,
) -> (String, Option<chrono::DateTime<chrono::Utc>>) {
    let row = conn
        .query_one(stmt(
            "SELECT payment_status::text AS ps, paid_at FROM orders WHERE id = $1",
            vec![order_id.into()],
        ))
        .await
        .unwrap()
        .unwrap();
    (
        row.try_get::<String>("", "ps").unwrap(),
        row.try_get::<Option<chrono::DateTime<chrono::Utc>>>("", "paid_at")
            .unwrap(),
    )
}

async fn cleanup(conn: &sea_orm::DatabaseConnection, user: Uuid) {
    conn.execute(stmt(
        "DELETE FROM orders WHERE user_id = $1",
        vec![user.into()],
    ))
    .await
    .ok();
}

#[tokio::test]
#[ignore]
async fn paid_at_is_stamped_once_and_never_moved_by_a_replay() {
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
    let conn = db::build_pool(&url).await.expect("pool");
    let user = Uuid::new_v4();
    let order = seed_order(&conn, user).await;

    db::set_order_payment_status(&conn, order, PaymentStatus::Paid)
        .await
        .unwrap();
    let (status1, paid_at1) = payment_state(&conn, order).await;
    assert_eq!(status1, "paid");
    let stamped = paid_at1.expect("first Paid stamps paid_at");

    // Replay the same Paid: status stays paid, paid_at must NOT move.
    db::set_order_payment_status(&conn, order, PaymentStatus::Paid)
        .await
        .unwrap();
    let (status2, paid_at2) = payment_state(&conn, order).await;
    assert_eq!(status2, "paid");
    assert_eq!(paid_at2, Some(stamped), "paid_at must not move on replay");

    cleanup(&conn, user).await;
}

#[tokio::test]
#[ignore]
async fn a_paid_order_never_regresses_but_a_refund_supersedes_it() {
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
    let conn = db::build_pool(&url).await.expect("pool");
    let user = Uuid::new_v4();
    let order = seed_order(&conn, user).await;

    db::set_order_payment_status(&conn, order, PaymentStatus::Paid)
        .await
        .unwrap();
    let (_, paid_at) = payment_state(&conn, order).await;
    let stamped = paid_at.expect("paid_at set");

    // A late/out-of-order webhook or a second return-URL landing tries to walk
    // the order backwards. Each must be a no-op; paid_at stays put.
    for regress in [
        PaymentStatus::Processing,
        PaymentStatus::Pending,
        PaymentStatus::Failed,
    ] {
        db::set_order_payment_status(&conn, order, regress)
            .await
            .unwrap();
        let (status, paid_at_now) = payment_state(&conn, order).await;
        assert_eq!(status, "paid", "Paid must not regress to {regress:?}");
        assert_eq!(
            paid_at_now,
            Some(stamped),
            "paid_at unchanged on a rejected regression"
        );
    }

    // Only a refund may supersede a paid order.
    db::set_order_payment_status(&conn, order, PaymentStatus::Refunded)
        .await
        .unwrap();
    let (status, _) = payment_state(&conn, order).await;
    assert_eq!(status, "refunded", "a refund supersedes Paid");

    cleanup(&conn, user).await;
}

#[tokio::test]
#[ignore]
async fn non_paid_statuses_advance_normally() {
    // The guard only pins Paid. Below Paid, the pending -> processing -> failed
    // transitions a provider drives must still apply.
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
    let conn = db::build_pool(&url).await.expect("pool");
    let user = Uuid::new_v4();
    let order = seed_order(&conn, user).await;

    // (status, expected DB string_value) pairs -- PaymentStatus has no as_str().
    for (next, expected) in [
        (PaymentStatus::Processing, "processing"),
        (PaymentStatus::Failed, "failed"),
        (PaymentStatus::Processing, "processing"),
    ] {
        db::set_order_payment_status(&conn, order, next)
            .await
            .unwrap();
        let (status, paid_at) = payment_state(&conn, order).await;
        assert_eq!(status, expected);
        assert!(paid_at.is_none(), "never paid, so paid_at stays NULL");
    }

    cleanup(&conn, user).await;
}
