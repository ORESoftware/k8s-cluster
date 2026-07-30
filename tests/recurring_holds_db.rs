//! DB-backed test for the hold-awareness added to `db::run_due_recurring_orders`
//! during the fiducia audit (ignored by default; needs a real DATABASE_URL).
//!
//! The runner used to compare a recurring line's qty against raw `on_hand`,
//! which let a recurring cycle consume stock a shopper was actively holding in
//! their cart -- turning that shopper's checkout into a shortage on stock they
//! had reserved. It now subtracts live `stock_holds` the same way `place_order`
//! and `ensure_hold` do. This proves both sides of that boundary:
//!
//!   * a live cross-cart hold that leaves too little free stock makes the runner
//!     SKIP the child (no decrement) while still advancing the cursor so the
//!     subscription isn't wedged;
//!   * once that hold expires, the next run fires the child and decrements.
//!
//!   DATABASE_URL=... cargo test --test recurring_holds_db -- --ignored --nocapture
use athleto_app_rs::db::{self, CartOwner};
use sea_orm::{ConnectionTrait, DbBackend, Statement};
use uuid::Uuid;

fn stmt(sql: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DbBackend::Postgres, sql, values)
}

async fn seed_product(conn: &sea_orm::DatabaseConnection, on_hand: i32) -> i64 {
    let slug = format!("test-{}", Uuid::new_v4().simple());
    let pid: i64 = conn
        .query_one(stmt(
            "INSERT INTO products (slug, name, description, format, calories, protein_g, price_cents) \
             VALUES ($1, 'Recurring Test', 'db test', 'powder', 100, 20, 500) RETURNING id",
            vec![slug.into()],
        ))
        .await.unwrap().unwrap().try_get("", "id").unwrap();
    conn.execute(stmt(
        "INSERT INTO inventory (product_id, on_hand) VALUES ($1, $2)",
        vec![pid.into(), on_hand.into()],
    ))
    .await
    .unwrap();
    pid
}

async fn on_hand(conn: &sea_orm::DatabaseConnection, pid: i64) -> i32 {
    conn.query_one(stmt(
        "SELECT on_hand FROM inventory WHERE product_id = $1",
        vec![pid.into()],
    ))
    .await
    .unwrap()
    .unwrap()
    .try_get::<i32>("", "on_hand")
    .unwrap()
}

async fn children(conn: &sea_orm::DatabaseConnection, parent: Uuid) -> i64 {
    conn.query_one(stmt(
        "SELECT count(*)::bigint AS n FROM orders WHERE recurs_from = $1",
        vec![parent.into()],
    ))
    .await
    .unwrap()
    .unwrap()
    .try_get::<i64>("", "n")
    .unwrap()
}

async fn due(conn: &sea_orm::DatabaseConnection, id: Uuid) -> bool {
    conn.query_one(stmt(
        "SELECT next_run_at <= now() AS d FROM orders WHERE id = $1",
        vec![id.into()],
    ))
    .await
    .unwrap()
    .unwrap()
    .try_get::<bool>("", "d")
    .unwrap()
}

#[tokio::test]
#[ignore]
async fn recurring_runner_skips_stock_a_shopper_is_holding_then_fires_after_expiry() {
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
    let conn = db::build_pool(&url).await.expect("pool");
    let owner = Uuid::new_v4();
    let shopper = Uuid::new_v4();
    // Exactly 2 units on hand.
    let pid = seed_product(&conn, 2).await;

    // A recurring order for 2 units, owned (no provider subscription), due now.
    let order: Uuid = conn
        .query_one(stmt(
            "INSERT INTO orders (user_id, kind, frequency, channel, ship_method, \
                 subtotal_cents, shipping_cents, tax_cents, total_cents, next_run_at) \
             VALUES ($1, 'recurring', 'weekly', 'b2b_portal', 'freight', 1000, 0, 0, 1000, \
                     now() - interval '1 day') RETURNING id",
            vec![owner.into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "id")
        .unwrap();
    conn.execute(stmt(
        "INSERT INTO order_items (order_id, product_id, qty, unit_price_cents) VALUES ($1, $2, 2, 500)",
        vec![order.into(), pid.into()],
    ))
    .await
    .unwrap();

    // A shopper holds 1 of the 2 units in their cart (live). Now only 1 is free,
    // but the recurring cycle needs 2 -> it must SKIP the child this cycle.
    let cart = db::find_or_create_cart(&conn, &CartOwner::User(shopper))
        .await
        .unwrap();
    conn.execute(stmt(
        "INSERT INTO stock_holds (cart_id, product_id, qty, held_until) \
         VALUES ($1, $2, 1, now() + interval '60 minutes')",
        vec![cart.into(), pid.into()],
    ))
    .await
    .unwrap();

    assert!(due(&conn, order).await, "order is due before the first run");
    let created = db::run_due_recurring_orders(&conn)
        .await
        .expect("runner ok");
    println!("run 1 created {created} child(ren)");

    assert_eq!(
        children(&conn, order).await,
        0,
        "no child while the shopper holds stock"
    );
    assert_eq!(
        on_hand(&conn, pid).await,
        2,
        "stock untouched -- the hold was respected"
    );
    // The cursor still advanced so the subscription isn't wedged (skip-but-advance).
    assert!(
        !due(&conn, order).await,
        "cursor advanced past now despite the skip"
    );

    // The shopper's hold expires; the units are free again. Rewind the cursor so
    // the order is due once more, then re-run: now it fires and decrements.
    conn.execute(stmt(
        "UPDATE stock_holds SET held_until = now() - interval '1 minute' WHERE cart_id = $1",
        vec![cart.into()],
    ))
    .await
    .unwrap();
    conn.execute(stmt(
        "UPDATE orders SET next_run_at = now() - interval '1 day' WHERE id = $1",
        vec![order.into()],
    ))
    .await
    .unwrap();

    let created2 = db::run_due_recurring_orders(&conn)
        .await
        .expect("runner ok");
    println!("run 2 created {created2} child(ren)");
    assert_eq!(
        children(&conn, order).await,
        1,
        "fires once the hold is gone"
    );
    assert_eq!(on_hand(&conn, pid).await, 0, "2 - 2 = 0 after the cycle");

    // Cleanup: children first (FK recurs_from), then the parent, cart, stock, product.
    conn.execute(stmt(
        "DELETE FROM orders WHERE recurs_from = $1",
        vec![order.into()],
    ))
    .await
    .ok();
    conn.execute(stmt(
        "DELETE FROM orders WHERE user_id = $1",
        vec![owner.into()],
    ))
    .await
    .ok();
    conn.execute(stmt(
        "DELETE FROM carts WHERE user_id = $1",
        vec![shopper.into()],
    ))
    .await
    .ok();
    conn.execute(stmt(
        "DELETE FROM stock_holds WHERE product_id = $1",
        vec![pid.into()],
    ))
    .await
    .ok();
    conn.execute(stmt(
        "DELETE FROM inventory WHERE product_id = $1",
        vec![pid.into()],
    ))
    .await
    .ok();
    conn.execute(stmt("DELETE FROM products WHERE id = $1", vec![pid.into()]))
        .await
        .ok();
}
