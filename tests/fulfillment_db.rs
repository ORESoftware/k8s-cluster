//! DB-backed test for `db::record_fulfillment` (ignored by default; needs a real
//! DATABASE_URL). Recording a shipment must:
//!
//!   * insert a `shipped` shipment with a carrier/tracking and an ETA window
//!     derived from the order's ship method, and flip the order to `fulfilled`;
//!   * return `None` for an unknown order id (nothing to ship);
//!   * refuse to ship a `cancelled` order -- the whole tx rolls back so no
//!     shipment (and no 856 ASN) is ever persisted against it.
//!
//!   DATABASE_URL=... cargo test --test fulfillment_db -- --ignored --nocapture
use athleto_app_rs::db;
use chrono::NaiveDate;
use sea_orm::{ConnectionTrait, DbBackend, Statement};
use uuid::Uuid;

fn stmt(sql: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DbBackend::Postgres, sql, values)
}

async fn seed_order(conn: &sea_orm::DatabaseConnection, user: Uuid, status: &str) -> Uuid {
    conn.query_one(stmt(
        "INSERT INTO orders (user_id, status, channel, ship_method, subtotal_cents, \
             shipping_cents, tax_cents, total_cents) \
         VALUES ($1, $2::order_status, 'b2b_portal', 'standard', 1000, 0, 0, 1000) RETURNING id",
        vec![user.into(), status.into()],
    ))
    .await.unwrap().unwrap().try_get("", "id").unwrap()
}

async fn order_status(conn: &sea_orm::DatabaseConnection, id: Uuid) -> String {
    conn.query_one(stmt("SELECT status::text AS s FROM orders WHERE id = $1", vec![id.into()]))
        .await.unwrap().unwrap().try_get::<String>("", "s").unwrap()
}

async fn cleanup(conn: &sea_orm::DatabaseConnection, user: Uuid) {
    conn.execute(stmt("DELETE FROM orders WHERE user_id = $1", vec![user.into()])).await.ok();
}

#[tokio::test]
#[ignore]
async fn record_fulfillment_ships_a_placed_order_and_marks_it_fulfilled() {
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
    let conn = db::build_pool(&url).await.expect("pool");
    let user = Uuid::new_v4();
    let order = seed_order(&conn, user, "placed").await;

    let ship_date = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();
    let shipment = db::record_fulfillment(&conn, order, "ups", "1Z-TEST-001", ship_date)
        .await
        .expect("query ok");
    let shipment_id = shipment.expect("a placed order ships");

    assert_eq!(order_status(&conn, order).await, "fulfilled", "order flips to fulfilled");

    let row = conn
        .query_one(stmt(
            "SELECT status::text AS s, carrier, tracking_number, eta_earliest, eta_latest \
             FROM shipments WHERE id = $1",
            vec![shipment_id.into()],
        ))
        .await.unwrap().unwrap();
    assert_eq!(row.try_get::<String>("", "s").unwrap(), "shipped");
    assert_eq!(row.try_get::<String>("", "carrier").unwrap(), "ups");
    assert_eq!(row.try_get::<String>("", "tracking_number").unwrap(), "1Z-TEST-001");
    let earliest = row.try_get::<NaiveDate>("", "eta_earliest").unwrap();
    let latest = row.try_get::<NaiveDate>("", "eta_latest").unwrap();
    assert!(earliest >= ship_date, "ETA earliest is on/after the ship date");
    assert!(latest >= earliest, "ETA window is well-ordered");

    cleanup(&conn, user).await;
}

#[tokio::test]
#[ignore]
async fn record_fulfillment_returns_none_for_an_unknown_order() {
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
    let conn = db::build_pool(&url).await.expect("pool");
    let ship_date = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();
    let got = db::record_fulfillment(&conn, Uuid::new_v4(), "ups", "X", ship_date)
        .await
        .expect("query ok");
    assert!(got.is_none(), "unknown order id yields None, no shipment");
}

#[tokio::test]
#[ignore]
async fn record_fulfillment_refuses_to_ship_a_cancelled_order() {
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
    let conn = db::build_pool(&url).await.expect("pool");
    let user = Uuid::new_v4();
    let order = seed_order(&conn, user, "cancelled").await;

    let ship_date = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();
    let got = db::record_fulfillment(&conn, order, "ups", "X", ship_date)
        .await
        .expect("query ok");
    assert!(got.is_none(), "a cancelled order cannot be shipped");
    // And no shipment leaked (the tx rolled back).
    let n: i64 = conn
        .query_one(stmt("SELECT count(*)::bigint AS n FROM shipments WHERE order_id = $1", vec![order.into()]))
        .await.unwrap().unwrap().try_get::<i64>("", "n").unwrap();
    assert_eq!(n, 0, "no shipment persisted against a cancelled order");
    assert_eq!(order_status(&conn, order).await, "cancelled", "status unchanged");

    cleanup(&conn, user).await;
}
