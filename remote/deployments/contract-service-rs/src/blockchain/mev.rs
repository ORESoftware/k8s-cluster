//! MEV / arbitrage **monitoring** — observation only. Callers submit observed
//! venue prices for an asset pair; the service computes the spread and flags an
//! opportunity above a threshold. There is deliberately **no execution path**:
//! this is a defensive/observability surface (spread + latency awareness), not a
//! trading bot. Alerts are returned to the caller and counted in metrics.

use axum::{extract::State, http::StatusCode, routing::post, Json, Router};
use serde::Deserialize;
use serde_json::json;
use std::sync::atomic::Ordering;

use super::{json_err, json_ok, record_request, require_enabled, validate_label};
use crate::AppState;

const MAX_VENUES: usize = 32;
/// Default spread (basis points) above which an opportunity is flagged.
const DEFAULT_THRESHOLD_BPS: f64 = 30.0;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct VenueQuote {
    venue: String,
    price: f64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OpportunityRequest {
    pair: String,
    quotes: Vec<VenueQuote>,
    #[serde(default)]
    threshold_bps: Option<f64>,
}

pub(super) fn routes() -> Router<AppState> {
    Router::new().route("/mev/opportunities", post(opportunities_http))
}

async fn opportunities_http(
    State(state): State<AppState>,
    Json(body): Json<OpportunityRequest>,
) -> axum::response::Response {
    let bc = &state.blockchain;
    record_request(bc);
    if let Err(resp) = require_enabled(bc, bc.config().mev_enabled, "BLOCKCHAIN_MEV_ENABLED") {
        return resp;
    }
    let pair = match validate_label(&body.pair, "pair") {
        Ok(value) => value,
        Err(error) => return json_err(StatusCode::BAD_REQUEST, &error),
    };
    if body.quotes.len() < 2 || body.quotes.len() > MAX_VENUES {
        return json_err(StatusCode::BAD_REQUEST, "quotes must contain 2..=32 venues");
    }
    if body
        .quotes
        .iter()
        .any(|q| !q.price.is_finite() || q.price <= 0.0)
    {
        return json_err(
            StatusCode::BAD_REQUEST,
            "each quote price must be a positive number",
        );
    }

    // Lowest ask vs highest bid across venues.
    let (low_venue, low) = body
        .quotes
        .iter()
        .min_by(|a, b| a.price.total_cmp(&b.price))
        .map(|q| (q.venue.clone(), q.price))
        .expect("non-empty checked above");
    let (high_venue, high) = body
        .quotes
        .iter()
        .max_by(|a, b| a.price.total_cmp(&b.price))
        .map(|q| (q.venue.clone(), q.price))
        .expect("non-empty checked above");

    let spread_bps = ((high - low) / low) * 10_000.0;
    let threshold = body
        .threshold_bps
        .filter(|t| t.is_finite() && *t >= 0.0)
        .unwrap_or(DEFAULT_THRESHOLD_BPS);
    let opportunity = spread_bps >= threshold;
    if opportunity {
        bc.metrics()
            .mev_alerts_total
            .fetch_add(1, Ordering::Relaxed);
        // Monitoring-only: emit an observation alert. There is no execution path.
        crate::publish_blockchain_event(
            &state,
            &bc.config().mev_alerts_subject,
            json!({
                "type": "blockchain.mev.alert",
                "pair": pair,
                "spreadBps": spread_bps,
                "thresholdBps": threshold,
                "buyAt": { "venue": low_venue, "price": low },
                "sellAt": { "venue": high_venue, "price": high },
                "mode": "monitoring-only",
            }),
        )
        .await;
    }

    json_ok(json!({
        "ok": true,
        "pair": pair,
        "spreadBps": spread_bps,
        "thresholdBps": threshold,
        "opportunity": opportunity,
        "buyAt": { "venue": low_venue, "price": low },
        "sellAt": { "venue": high_venue, "price": high },
        "mode": "monitoring-only",
        "note": "observation surface; no execution path exists",
    }))
}
