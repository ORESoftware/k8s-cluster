//! B2B ERP-facing JSON API under /api/v1, authenticated with per-account API
//! keys (created on /account, stored as SHA-256 hashes). This is the surface
//! an EDI provider or ERP connector calls; docs/erp-integration.md maps X12
//! documents (850/855/856/810) onto these endpoints and tables.

use std::collections::HashMap;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::db::{self, CustomerProfile};
use crate::SharedState;

fn error_response(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({ "error": message }))).into_response()
}

fn hash_key(key: &str) -> String {
    Sha256::digest(key.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Resolve `Authorization: Bearer athk_...` to a B2B customer.
async fn authenticate(
    state: &SharedState,
    headers: &HeaderMap,
) -> Result<(Uuid, CustomerProfile), Response> {
    let Some(pool) = &state.pool else {
        return Err(error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "database not configured",
        ));
    };
    let token = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::trim)
        .unwrap_or_default();
    if !token.starts_with("athk_") {
        return Err(error_response(
            StatusCode::UNAUTHORIZED,
            "missing or malformed API key (expected 'Authorization: Bearer athk_...')",
        ));
    }
    let user_id = match db::api_key_user(pool, &hash_key(token)).await {
        Ok(Some(user_id)) => user_id,
        Ok(None) => {
            return Err(error_response(
                StatusCode::UNAUTHORIZED,
                "unknown or revoked API key",
            ))
        }
        Err(err) => {
            tracing::error!(error = %err, "api key lookup failed");
            return Err(error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "lookup failed",
            ));
        }
    };
    match db::get_profile(pool, user_id).await {
        Ok(Some(profile)) if profile.is_b2b_approved() => Ok((user_id, profile)),
        Ok(_) => Err(error_response(
            StatusCode::FORBIDDEN,
            "API access is for business accounts",
        )),
        Err(err) => {
            tracing::error!(error = %err, "profile lookup failed");
            Err(error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "lookup failed",
            ))
        }
    }
}

fn constant_time_eq(left: &str, right: &str) -> bool {
    left.len() == right.len()
        && left
            .bytes()
            .zip(right.bytes())
            .fold(0u8, |difference, (left, right)| difference | (left ^ right))
            == 0
}

// The handler must short-circuit with a fully formed HTTP response. Boxing it
// just to shrink the enum would allocate on each rejected request.
#[allow(clippy::result_large_err)]
fn operations_authorized(state: &SharedState, headers: &HeaderMap) -> Result<(), Response> {
    let Some(expected) = state.config.operations_api_key.as_deref() else {
        return Err(error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "fulfillment API is not configured",
        ));
    };
    let presented = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::trim)
        .unwrap_or_default();
    if constant_time_eq(presented, expected) {
        Ok(())
    } else {
        Err(error_response(
            StatusCode::UNAUTHORIZED,
            "warehouse credential required",
        ))
    }
}

/// GET /api/v1/products
pub async fn products(State(state): State<SharedState>, headers: HeaderMap) -> Response {
    if let Err(response) = authenticate(&state, &headers).await {
        return response;
    }
    let products = match &state.pool {
        Some(pool) => db::list_products(pool)
            .await
            .unwrap_or_else(|_| db::fallback_products()),
        None => db::fallback_products(),
    };
    let body: Vec<_> = products
        .iter()
        .map(|product| {
            json!({
                "id": product.id,
                "slug": product.slug,
                "name": product.name,
                "subname": product.subname,
                "format": product.format.label(),
                "calories": product.calories,
                "protein_g": product.protein_g,
                "unit_price_cents": product.price_cents,
            })
        })
        .collect();
    Json(json!({ "products": body })).into_response()
}

/// GET /api/v1/orders
pub async fn orders_list(State(state): State<SharedState>, headers: HeaderMap) -> Response {
    let (user_id, _) = match authenticate(&state, &headers).await {
        Ok(pair) => pair,
        Err(response) => return response,
    };
    // `authenticate` already proved the pool exists, but re-deriving it here
    // keeps a broken invariant a clean 503 instead of a request-path panic.
    let Some(pool) = state.pool.as_ref() else {
        return error_response(StatusCode::SERVICE_UNAVAILABLE, "database not configured");
    };

    let (orders, items) = match (
        db::list_orders(pool, user_id).await,
        db::order_items_for_user(pool, user_id).await,
    ) {
        (Ok(orders), Ok(items)) => (orders, items),
        (Err(err), _) | (_, Err(err)) => {
            tracing::error!(error = %err, "order listing failed");
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "order listing failed");
        }
    };
    let mut items_by_order: HashMap<Uuid, Vec<serde_json::Value>> = HashMap::new();
    for item in items {
        items_by_order
            .entry(item.order_id)
            .or_default()
            .push(json!({
                "name": item.name,
                "subname": item.subname,
                "format": item.format.label(),
                "qty": item.qty,
                "unit_price_cents": item.unit_price_cents,
            }));
    }
    let shipments = db::shipments_for_user(pool, user_id)
        .await
        .unwrap_or_default();
    let mut ship_by_order: HashMap<Uuid, db::Shipment> = HashMap::new();
    for row in &shipments {
        ship_by_order.insert(row.order_id, row.shipment());
    }

    let body: Vec<_> = orders
        .iter()
        .map(|order| {
            let (eta_earliest, eta_latest) = order.delivery_window();
            let tracking = ship_by_order.get(&order.id).map(|s| {
                json!({
                    "status": s.status.label(),
                    "carrier": s.carrier,
                    "tracking_number": s.tracking_number,
                    "tracking_url": s.tracking_url(),
                    "ship_date": s.ship_date,
                    "eta_earliest": s.eta_earliest,
                    "eta_latest": s.eta_latest,
                    "delivered_at": s.delivered_at,
                })
            });
            json!({
                "id": order.id,
                "kind": order.kind.as_str(),
                "frequency": order.frequency.map(|f| f.label()),
                "status": order.status.label(),
                "channel": order.channel.as_str(),
                "ship_method": order.ship_method.as_str(),
                "po_number": order.po_number,
                "subtotal_cents": order.subtotal_cents,
                "shipping_cents": order.shipping_cents,
                "tax_cents": order.tax_cents,
                "total_cents": order.total_cents,
                "estimated_delivery": { "earliest": eta_earliest, "latest": eta_latest },
                "shipment": tracking,
                "next_run_at": order.next_run_at,
                "created_at": order.created_at,
                "items": items_by_order.remove(&order.id).unwrap_or_default(),
            })
        })
        .collect();
    Json(json!({ "orders": body })).into_response()
}

#[derive(Debug, Deserialize)]
pub struct FulfillmentRequest {
    pub carrier: String,
    pub tracking_number: String,
    /// ISO date (YYYY-MM-DD); defaults to today when omitted.
    #[serde(default)]
    pub ship_date: Option<String>,
}

/// POST /api/v1/orders/{id}/fulfillment -- record a shipment with carrier +
/// tracking and mark the order fulfilled (the outbound "856 ASN" step).
pub async fn order_fulfill(
    State(state): State<SharedState>,
    headers: HeaderMap,
    axum::extract::Path(order_id): axum::extract::Path<Uuid>,
    Json(request): Json<FulfillmentRequest>,
) -> Response {
    if let Err(response) = operations_authorized(&state, &headers) {
        return response;
    }
    let Some(pool) = &state.pool else {
        return error_response(StatusCode::SERVICE_UNAVAILABLE, "database not configured");
    };

    if request.carrier.trim().is_empty() || request.tracking_number.trim().is_empty() {
        return error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            "carrier and tracking_number are required",
        );
    }
    let ship_date = match request.ship_date.as_deref() {
        Some(raw) => match chrono::NaiveDate::parse_from_str(raw, "%Y-%m-%d") {
            Ok(date) => date,
            Err(_) => {
                return error_response(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "ship_date must be YYYY-MM-DD",
                )
            }
        },
        None => chrono::Utc::now().date_naive(),
    };

    match db::record_fulfillment(
        pool,
        order_id,
        request.carrier.trim(),
        request.tracking_number.trim(),
        ship_date,
    )
    .await
    {
        Ok(Some(shipment_id)) => (
            StatusCode::CREATED,
            Json(json!({
                "shipment_id": shipment_id,
                "order_id": order_id,
                "status": "shipped",
                "carrier": request.carrier.trim(),
                "tracking_number": request.tracking_number.trim(),
            })),
        )
            .into_response(),
        Ok(None) => error_response(StatusCode::NOT_FOUND, "order not found"),
        Err(err) => {
            tracing::error!(error = %err, "fulfillment failed");
            error_response(StatusCode::INTERNAL_SERVER_ERROR, "fulfillment failed")
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct B2bApprovalRequest {
    /// true = approve (default), false = revoke.
    #[serde(default = "default_true")]
    pub approved: bool,
}

fn default_true() -> bool {
    true
}

/// POST /api/v1/ops/customers/{user_id}/approval -- ops approves (or revokes) a
/// business account after vetting. Gated by the operations credential, not a
/// customer API key. Until a B2B account is approved, `is_b2b_approved()` gates
/// off its ordering, ERP API, and API keys -- so this is the step that makes a
/// business account usable. Idempotent: re-approving keeps the original
/// timestamp.
pub async fn ops_b2b_approval(
    State(state): State<SharedState>,
    headers: HeaderMap,
    axum::extract::Path(user_id): axum::extract::Path<Uuid>,
    Json(request): Json<B2bApprovalRequest>,
) -> Response {
    if let Err(response) = operations_authorized(&state, &headers) {
        return response;
    }
    let Some(pool) = &state.pool else {
        return error_response(StatusCode::SERVICE_UNAVAILABLE, "database not configured");
    };

    match db::set_b2b_approval(pool, user_id, request.approved).await {
        Ok(Some(is_approved)) => Json(json!({
            "user_id": user_id,
            "approved": is_approved,
        }))
        .into_response(),
        Ok(None) => error_response(
            StatusCode::NOT_FOUND,
            "no business account for that user_id (must have completed B2B setup)",
        ),
        Err(err) => {
            tracing::error!(error = %err, "b2b approval failed");
            error_response(StatusCode::INTERNAL_SERVER_ERROR, "approval failed")
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct ApiOrderRequest {
    #[serde(default)]
    pub po_number: Option<String>,
    #[serde(default)]
    pub kind: Option<db::OrderKind>,
    #[serde(default)]
    pub frequency: Option<db::OrderFrequency>,
    pub items: Vec<ApiOrderItem>,
}

#[derive(Debug, Deserialize)]
pub struct ApiOrderItem {
    #[serde(default)]
    pub product_id: Option<i64>,
    #[serde(default)]
    pub slug: Option<String>,
    pub qty: i32,
}

/// POST /api/v1/orders -- place an order (the "850 in" path when an EDI
/// provider webhook is mapped onto this endpoint).
pub async fn orders_create(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(request): Json<ApiOrderRequest>,
) -> Response {
    let (user_id, _) = match authenticate(&state, &headers).await {
        Ok(pair) => pair,
        Err(response) => return response,
    };
    // See orders_list: a clean 503 rather than a panic if the invariant breaks.
    let Some(pool) = state.pool.as_ref() else {
        return error_response(StatusCode::SERVICE_UNAVAILABLE, "database not configured");
    };

    if request.items.is_empty() {
        return error_response(StatusCode::UNPROCESSABLE_ENTITY, "items must not be empty");
    }
    let kind = request.kind.unwrap_or(db::OrderKind::OneTime);
    if kind == db::OrderKind::Recurring && request.frequency.is_none() {
        return error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            "recurring orders need a frequency (weekly|biweekly|monthly|quarterly)",
        );
    }

    let catalog = match db::product_prices(pool).await {
        Ok(catalog) => catalog,
        Err(err) => {
            tracing::error!(error = %err, "catalog load failed");
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "catalog load failed");
        }
    };
    let by_id: HashMap<i64, i32> = catalog.iter().map(|(id, _, price)| (*id, *price)).collect();
    let by_slug: HashMap<&str, (i64, i32)> = catalog
        .iter()
        .map(|(id, slug, price)| (slug.as_str(), (*id, *price)))
        .collect();

    let mut lines = Vec::new();
    for item in &request.items {
        if item.qty <= 0 {
            return error_response(StatusCode::UNPROCESSABLE_ENTITY, "qty must be positive");
        }
        if item.qty > crate::cart::MAX_QTY_PER_LINE {
            // Match the storefront's per-line ceiling so an ERP/EDI caller can't
            // push an absurd quantity past the catalog/stock checks.
            return error_response(
                StatusCode::UNPROCESSABLE_ENTITY,
                "qty exceeds the per-line maximum",
            );
        }
        let resolved = match (item.product_id, item.slug.as_deref()) {
            (Some(id), _) => by_id.get(&id).map(|price| (id, *price)),
            (None, Some(slug)) => by_slug.get(slug).copied(),
            (None, None) => None,
        };
        let Some((product_id, unit_price_cents)) = resolved else {
            return error_response(
                StatusCode::UNPROCESSABLE_ENTITY,
                "each item needs a valid product_id or slug",
            );
        };
        lines.push(db::NewOrderLine {
            product_id,
            qty: item.qty,
            unit_price_cents,
        });
    }

    match db::place_order(
        pool,
        user_id,
        kind,
        request.frequency,
        db::OrderChannel::B2bApi,
        db::ShipMethod::Freight,
        request
            .po_number
            .as_deref()
            .map(str::trim)
            .filter(|po| !po.is_empty()),
        &lines,
        None,
    )
    .await
    {
        Ok(order_id) => {
            let total: i64 = lines
                .iter()
                .map(|line| i64::from(line.unit_price_cents) * i64::from(line.qty))
                .sum();
            (
                StatusCode::CREATED,
                Json(json!({
                    "id": order_id,
                    "status": "placed",
                    "kind": kind.as_str(),
                    "po_number": request.po_number,
                    "total_cents": total,
                })),
            )
                .into_response()
        }
        Err(db::OrderError::Insufficient(shortages)) => (
            StatusCode::CONFLICT,
            Json(json!({
                "error": "insufficient stock",
                "shortages": shortages
                    .iter()
                    .map(|s| json!({
                        "product_id": s.product_id,
                        "requested": s.requested,
                        "available": s.available,
                    }))
                    .collect::<Vec<_>>(),
            })),
        )
            .into_response(),
        Err(db::OrderError::AlreadyPlaced) => {
            // Not reachable on the API path (no cart_id), but keep the match
            // total and fail safe if that ever changes.
            error_response(StatusCode::CONFLICT, "order already placed")
        }
        Err(db::OrderError::Db(err)) => {
            tracing::error!(error = %err, "order placement failed");
            error_response(StatusCode::INTERNAL_SERVER_ERROR, "order placement failed")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The known SHA-256 of a fixed key; account::hash_api_key must agree with
    // this same vector, or keys minted on /account would never authenticate
    // here. Both tests assert the identical constant.
    const VECTOR_INPUT: &str = "athk_test_vector_001";
    const VECTOR_SHA256: &str = "66adca3c7ae7f126ff03b7cc7daba157a1b9705447faaabd4fc1c2995c0d308a";

    #[test]
    fn hash_key_matches_shared_vector_and_is_deterministic() {
        assert_eq!(hash_key(VECTOR_INPUT), VECTOR_SHA256);
        assert_eq!(hash_key("abc"), hash_key("abc"));
        assert_ne!(hash_key("abc"), hash_key("abd"));
        assert_eq!(hash_key(VECTOR_INPUT).len(), 64);
    }

    #[test]
    fn api_order_request_accepts_slug_or_id_forms() {
        let by_slug: ApiOrderRequest = serde_json::from_value(serde_json::json!({
            "po_number": "PO-1",
            "items": [{ "slug": "recover-o-cup", "qty": 24 }]
        }))
        .unwrap();
        assert_eq!(by_slug.items.len(), 1);
        assert_eq!(by_slug.items[0].slug.as_deref(), Some("recover-o-cup"));
        assert_eq!(by_slug.items[0].qty, 24);

        let by_id: ApiOrderRequest = serde_json::from_value(serde_json::json!({
            "kind": "recurring",
            "frequency": "monthly",
            "items": [{ "product_id": 3, "qty": 2 }]
        }))
        .unwrap();
        assert_eq!(by_id.items[0].product_id, Some(3));
        assert!(matches!(by_id.kind, Some(db::OrderKind::Recurring)));
        assert!(matches!(by_id.frequency, Some(db::OrderFrequency::Monthly)));
    }

    // The ops/warehouse credential is compared with constant_time_eq; a length
    // leak or early-return would let an attacker time-oracle the key.
    #[test]
    fn constant_time_eq_accepts_only_identical_strings() {
        assert!(constant_time_eq("athk_live_secret", "athk_live_secret"));
        assert!(!constant_time_eq("athk_live_secret", "athk_live_secreT")); // one byte, same length
        assert!(!constant_time_eq("short", "a-much-longer-secret")); // different lengths
        assert!(constant_time_eq("", "")); // both empty compare equal
        assert!(!constant_time_eq("", "x"));
    }

    // API keys are stored only as SHA-256 hashes. The hash must be lowercase hex,
    // reveal no plaintext, and avalanche so a near-miss key can't be inferred.
    #[test]
    fn hash_key_is_lowercase_hex_never_leaks_plaintext_and_avalanches() {
        let digest = hash_key("athk_live_super_secret_value");
        assert_eq!(digest.len(), 64);
        assert!(digest
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
        assert!(!digest.contains("secret"));
        let near = hash_key("athk_live_super_secret_valuf"); // one char different
        let differing = digest
            .chars()
            .zip(near.chars())
            .filter(|(a, b)| a != b)
            .count();
        assert!(
            differing > 20,
            "weak avalanche: only {differing} hex chars changed"
        );
    }

    // An order item may carry both product_id and slug (resolved by id first) or
    // neither (rejected 422 at resolve time) — both must at least deserialize so
    // the handler owns the semantics, not serde.
    #[test]
    fn api_order_item_tolerates_ambiguous_and_empty_id_forms() {
        let both: ApiOrderItem = serde_json::from_value(
            serde_json::json!({ "product_id": 7, "slug": "wobble-24", "qty": 3 }),
        )
        .unwrap();
        assert_eq!(both.product_id, Some(7));
        assert_eq!(both.slug.as_deref(), Some("wobble-24"));
        let neither: ApiOrderItem =
            serde_json::from_value(serde_json::json!({ "qty": 1 })).unwrap();
        assert!(neither.product_id.is_none() && neither.slug.is_none());
    }
}
