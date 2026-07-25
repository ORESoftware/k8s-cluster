//! DB-backed tests for `db::place_order` and `db::ensure_hold` (ignored by
//! default; needs a real DATABASE_URL). Covers the stock/oversell heart of the
//! system:
//!
//!   * a successful order decrements `on_hand` by exactly the ordered qty and
//!     clears the cart's items + holds;
//!   * a live hold in ANOTHER cart reduces availability, so an order that
//!     exceeds the free stock fails with `Insufficient` and does not decrement;
//!   * `ensure_hold` sees cross-cart holds and refuses when they exhaust stock,
//!     but an EXPIRED hold does not block (lazy expiry), and re-holding the same
//!     cart+product upserts rather than stacks.
//!
//! Each test creates its OWN product + inventory row (fresh slug) so parallel
//! runs and the seeded catalog never interfere, and cleans up after itself.
//!
//!   DATABASE_URL=... cargo test --test place_order_db -- --ignored --nocapture
use athleto_app_rs::db::{self, CartOwner, HoldOutcome, NewOrderLine, OrderChannel, OrderError, OrderKind, ShipMethod};
use sea_orm::{ConnectionTrait, DbBackend, Statement};
use uuid::Uuid;

fn stmt(sql: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DbBackend::Postgres, sql, values)
}

/// Create a fresh tracked product with `on_hand` stock; return its product id.
async fn seed_product(conn: &sea_orm::DatabaseConnection, on_hand: i32) -> i64 {
    let slug = format!("test-{}", Uuid::new_v4().simple());
    let pid: i64 = conn
        .query_one(stmt(
            "INSERT INTO products (slug, name, description, format, calories, protein_g, price_cents) \
             VALUES ($1, 'Test Product', 'db test', 'cup', 100, 20, 500) RETURNING id",
            vec![slug.into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "id")
        .unwrap();
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

/// Insert a hold for `cart` on `pid` that expires `mins` from now (negative =
/// already expired).
async fn seed_hold(conn: &sea_orm::DatabaseConnection, cart: Uuid, pid: i64, qty: i32, mins: i64) {
    conn.execute(stmt(
        "INSERT INTO stock_holds (cart_id, product_id, qty, held_until) \
         VALUES ($1, $2, $3, now() + ($4 || ' minutes')::interval)",
        vec![cart.into(), pid.into(), qty.into(), mins.to_string().into()],
    ))
    .await
    .unwrap();
}

async fn cleanup(conn: &sea_orm::DatabaseConnection, users: &[Uuid], pid: i64) {
    for u in users {
        conn.execute(stmt(
            "DELETE FROM orders WHERE user_id = $1",
            vec![(*u).into()],
        ))
        .await
        .ok();
        conn.execute(stmt("DELETE FROM carts WHERE user_id = $1", vec![(*u).into()]))
            .await
            .ok(); // cascades cart_items + stock_holds
    }
    conn.execute(stmt("DELETE FROM stock_holds WHERE product_id = $1", vec![pid.into()]))
        .await
        .ok();
    conn.execute(stmt("DELETE FROM inventory WHERE product_id = $1", vec![pid.into()]))
        .await
        .ok();
    conn.execute(stmt("DELETE FROM products WHERE id = $1", vec![pid.into()]))
        .await
        .ok();
}

#[tokio::test]
#[ignore]
async fn place_order_decrements_stock_and_clears_the_cart() {
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
    let conn = db::build_pool(&url).await.expect("pool");
    let user = Uuid::new_v4();
    let pid = seed_product(&conn, 10).await;

    // Build a cart with a hold + item, like the storefront does.
    let cart = db::find_or_create_cart(&conn, &CartOwner::User(user)).await.unwrap();
    assert!(matches!(
        db::ensure_hold(&conn, cart, pid, 3).await.unwrap(),
        HoldOutcome::Held
    ));
    db::add_cart_item(&conn, cart, pid, 3).await.unwrap();

    let lines = [NewOrderLine { product_id: pid, qty: 3, unit_price_cents: 500 }];
    let order = db::place_order(
        &conn,
        user,
        OrderKind::OneTime,
        None,
        OrderChannel::D2cWeb,
        ShipMethod::Standard,
        None,
        &lines,
        Some(cart),
    )
    .await
    .expect("order placed");

    assert_eq!(on_hand(&conn, pid).await, 7, "10 - 3 = 7");

    // Cart emptied: its items and holds are gone.
    let items = conn
        .query_one(stmt("SELECT count(*)::bigint AS n FROM cart_items WHERE cart_id = $1", vec![cart.into()]))
        .await.unwrap().unwrap().try_get::<i64>("", "n").unwrap();
    let holds = conn
        .query_one(stmt("SELECT count(*)::bigint AS n FROM stock_holds WHERE cart_id = $1", vec![cart.into()]))
        .await.unwrap().unwrap().try_get::<i64>("", "n").unwrap();
    assert_eq!(items, 0, "cart_items cleared on success");
    assert_eq!(holds, 0, "cart holds consumed on success");

    // The order + its line exist.
    let got = db::get_order(&conn, user, order).await.unwrap();
    assert!(got.is_some(), "order readable by its owner");

    cleanup(&conn, &[user], pid).await;
}

#[tokio::test]
#[ignore]
async fn a_hold_in_another_cart_blocks_oversell() {
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
    let conn = db::build_pool(&url).await.expect("pool");
    let alice = Uuid::new_v4();
    let bob = Uuid::new_v4();
    let pid = seed_product(&conn, 5).await;

    // Alice holds 4 of the 5 units in her cart (live, 60 min).
    let alice_cart = db::find_or_create_cart(&conn, &CartOwner::User(alice)).await.unwrap();
    seed_hold(&conn, alice_cart, pid, 4, 60).await;

    // Bob tries to buy 3: on_hand is 5 but only 1 is free -> Insufficient, and
    // NO decrement happens (the whole tx rolls back). Bob's cart must carry the
    // line so place_order gets past its empty-cart idempotency guard and reaches
    // the availability re-check (an empty cart short-circuits to AlreadyPlaced).
    let bob_cart = db::find_or_create_cart(&conn, &CartOwner::User(bob)).await.unwrap();
    db::add_cart_item(&conn, bob_cart, pid, 3).await.unwrap();
    let lines = [NewOrderLine { product_id: pid, qty: 3, unit_price_cents: 500 }];
    let err = db::place_order(
        &conn, bob, OrderKind::OneTime, None, OrderChannel::D2cWeb, ShipMethod::Standard, None, &lines, Some(bob_cart),
    )
    .await
    .expect_err("must be refused");
    match err {
        OrderError::Insufficient(lines) => {
            assert_eq!(lines.len(), 1);
            assert_eq!(lines[0].available, 1, "5 on hand - 4 held elsewhere = 1 free");
        }
        other => panic!("expected Insufficient, got {other:?}"),
    }
    assert_eq!(on_hand(&conn, pid).await, 5, "no decrement on a refused order");

    cleanup(&conn, &[alice, bob], pid).await;
}

#[tokio::test]
#[ignore]
async fn ensure_hold_respects_live_holds_expiry_and_upserts() {
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
    let conn = db::build_pool(&url).await.expect("pool");
    let alice = Uuid::new_v4();
    let bob = Uuid::new_v4();
    let pid = seed_product(&conn, 5).await;

    // A live hold of 5 by Alice leaves nothing for Bob.
    let alice_cart = db::find_or_create_cart(&conn, &CartOwner::User(alice)).await.unwrap();
    seed_hold(&conn, alice_cart, pid, 5, 60).await;
    let bob_cart = db::find_or_create_cart(&conn, &CartOwner::User(bob)).await.unwrap();
    match db::ensure_hold(&conn, bob_cart, pid, 1).await.unwrap() {
        HoldOutcome::Insufficient { available } => assert_eq!(available, 0),
        other => panic!("expected Insufficient, got {other:?}"),
    }

    // Expire Alice's hold: the stock is free again the instant the clock passes
    // it (lazy expiry), so Bob's hold now succeeds.
    conn.execute(stmt(
        "UPDATE stock_holds SET held_until = now() - interval '1 minute' WHERE cart_id = $1",
        vec![alice_cart.into()],
    ))
    .await
    .unwrap();
    assert!(matches!(
        db::ensure_hold(&conn, bob_cart, pid, 1).await.unwrap(),
        HoldOutcome::Held
    ));

    // Re-holding the same cart+product UPSERTS (does not stack): asking for 2
    // replaces the 1, and a third of the 5 units is still free for it.
    assert!(matches!(
        db::ensure_hold(&conn, bob_cart, pid, 2).await.unwrap(),
        HoldOutcome::Held
    ));
    let held: i64 = conn
        .query_one(stmt(
            "SELECT COALESCE(SUM(qty),0)::bigint AS q FROM stock_holds WHERE cart_id = $1 AND product_id = $2",
            vec![bob_cart.into(), pid.into()],
        ))
        .await.unwrap().unwrap().try_get::<i64>("", "q").unwrap();
    assert_eq!(held, 2, "hold qty upserted to 2, not stacked to 3");

    cleanup(&conn, &[alice, bob], pid).await;
}
