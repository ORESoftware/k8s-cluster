//! DB-backed concurrency test for `db::place_order` (ignored by default; needs a
//! real DATABASE_URL). Proves the stock-safety invariant that unit logic can't:
//! two shoppers checking out the LAST unit at the same instant cannot both
//! succeed. The guard is the per-line `SELECT ... FOR UPDATE` on the inventory
//! row (place_order locks inventory rows in product-id order), which serializes
//! the two transactions: the loser blocks until the winner commits, then
//! re-reads `on_hand = 0` and fails with `Insufficient` instead of driving stock
//! negative.
//!
//! Uses TWO independent pools so the two checkouts run on genuinely separate
//! backend connections, the way two app replicas would.
//!
//!   DATABASE_URL=... cargo test --test concurrent_order_db -- --ignored --nocapture
use athleto_app_rs::db::{self, CartOwner, NewOrderLine, OrderChannel, OrderError, OrderKind, ShipMethod};
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
             VALUES ($1, 'Concurrency Test', 'db test', 'cup', 100, 20, 500) RETURNING id",
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

async fn cart_with_line(conn: &sea_orm::DatabaseConnection, user: Uuid, pid: i64, qty: i32) -> Uuid {
    let cart = db::find_or_create_cart(conn, &CartOwner::User(user)).await.unwrap();
    db::add_cart_item(conn, cart, pid, qty).await.unwrap();
    cart
}

#[tokio::test]
#[ignore]
async fn two_shoppers_racing_for_the_last_unit_do_not_oversell() {
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
    // Two independent pools -> two real backend connections that can run at once.
    let conn_a = db::build_pool(&url).await.expect("pool a");
    let conn_b = db::build_pool(&url).await.expect("pool b");
    let alice = Uuid::new_v4();
    let bob = Uuid::new_v4();

    // Exactly ONE unit on hand; each shopper's cart wants that one unit.
    let pid = seed_product(&conn_a, 1).await;
    let alice_cart = cart_with_line(&conn_a, alice, pid, 1).await;
    let bob_cart = cart_with_line(&conn_b, bob, pid, 1).await;

    let line = [NewOrderLine { product_id: pid, qty: 1, unit_price_cents: 500 }];

    // Fire both checkouts concurrently and collect both results.
    let fut_a = db::place_order(
        &conn_a, alice, OrderKind::OneTime, None, OrderChannel::D2cWeb, ShipMethod::Standard, None, &line, Some(alice_cart),
    );
    let fut_b = db::place_order(
        &conn_b, bob, OrderKind::OneTime, None, OrderChannel::D2cWeb, ShipMethod::Standard, None, &line, Some(bob_cart),
    );
    let (res_a, res_b) = tokio::join!(fut_a, fut_b);

    let a_ok = res_a.is_ok();
    let b_ok = res_b.is_ok();
    println!("alice_ok={a_ok} bob_ok={b_ok}");

    // Exactly one winner.
    assert!(a_ok ^ b_ok, "exactly one checkout may win the last unit (a={a_ok}, b={b_ok})");
    // The loser failed cleanly with a per-line shortage, not a DB error.
    let loser = if a_ok { res_b } else { res_a };
    match loser {
        Err(OrderError::Insufficient(lines)) => {
            assert_eq!(lines.len(), 1);
            assert_eq!(lines[0].available, 0, "nothing left for the loser");
        }
        other => panic!("loser must be a clean Insufficient, got {other:?}"),
    }

    // Stock landed at exactly zero -- never negative, never still 1.
    let on_hand: i32 = conn_a
        .query_one(stmt("SELECT on_hand FROM inventory WHERE product_id = $1", vec![pid.into()]))
        .await.unwrap().unwrap().try_get::<i32>("", "on_hand").unwrap();
    assert_eq!(on_hand, 0, "the single unit was sold exactly once");

    // Exactly one order exists across the two users.
    let orders: i64 = conn_a
        .query_one(stmt(
            "SELECT count(*)::bigint AS n FROM orders WHERE user_id = ANY($1)",
            vec![vec![alice, bob].into()],
        ))
        .await.unwrap().unwrap().try_get::<i64>("", "n").unwrap();
    assert_eq!(orders, 1, "one unit sold => one order");

    // Cleanup.
    for u in [alice, bob] {
        conn_a.execute(stmt("DELETE FROM orders WHERE user_id = $1", vec![u.into()])).await.ok();
        conn_a.execute(stmt("DELETE FROM carts WHERE user_id = $1", vec![u.into()])).await.ok();
    }
    conn_a.execute(stmt("DELETE FROM stock_holds WHERE product_id = $1", vec![pid.into()])).await.ok();
    conn_a.execute(stmt("DELETE FROM inventory WHERE product_id = $1", vec![pid.into()])).await.ok();
    conn_a.execute(stmt("DELETE FROM products WHERE id = $1", vec![pid.into()])).await.ok();
}
