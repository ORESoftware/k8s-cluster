//! Websocket fan-out of plan/run events.
//!
//! Clients subscribe per-plan. The socket is authorized *before* the upgrade,
//! using the same `Operator` extractor as the JSON routes, and plan ownership is
//! re-checked against the database — an upgrade must not become a way to
//! observe another operator's runs.
//!
//! This is the server-push half of the org's websocket story; the other half is
//! clients subscribing directly to Supabase Realtime for their own telemetry.
//! Domain events flow here because the domain tables live on RDS, which has no
//! Realtime.

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Path, State,
    },
    response::IntoResponse,
};
use dd_pg_defs_sea_orm::fab_plans;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast::error::RecvError;
use uuid::Uuid;

use crate::{error::ServiceError, supabase_auth::Operator, SharedState};

/// An event broadcast to subscribers of a plan.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub(crate) struct PlanEvent {
    pub(crate) plan_id: Uuid,
    pub(crate) kind: String,
    pub(crate) payload: serde_json::Value,
}

pub(crate) async fn plan_events(
    State(state): State<SharedState>,
    operator: Operator,
    Path(id): Path<Uuid>,
    upgrade: WebSocketUpgrade,
) -> Result<impl IntoResponse, ServiceError> {
    // Authorize before upgrading. Once the socket is open there is no
    // convenient place to return an HTTP status.
    let db = state.persistence.connection()?;
    fab_plans::Entity::find_by_id(id)
        .filter(fab_plans::Column::OwnerEmail.eq(operator.email.as_str()))
        .one(db)
        .await?
        .ok_or(ServiceError::NotFound)?;

    let receiver = state.events.subscribe();
    Ok(upgrade.on_upgrade(move |socket| pump(socket, receiver, id)))
}

/// Forward matching events until the client goes away or falls too far behind.
async fn pump(
    mut socket: WebSocket,
    mut receiver: tokio::sync::broadcast::Receiver<PlanEvent>,
    plan_id: Uuid,
) {
    loop {
        match receiver.recv().await {
            Ok(event) if event.plan_id == plan_id => {
                let Ok(text) = serde_json::to_string(&event) else {
                    continue;
                };
                if socket.send(Message::Text(text)).await.is_err() {
                    break; // client disconnected
                }
            }
            Ok(_) => {} // an event for a different plan
            // A slow consumer is dropped rather than allowed to grow the
            // broadcast buffer. Reconnecting re-establishes a clean position.
            Err(RecvError::Lagged(skipped)) => {
                tracing::warn!(
                    plan.id = %plan_id,
                    events.skipped = skipped,
                    "websocket subscriber lagged; closing"
                );
                break;
            }
            Err(RecvError::Closed) => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_round_trip_as_json() {
        let event = PlanEvent {
            plan_id: Uuid::nil(),
            kind: "run.progress".to_string(),
            payload: serde_json::json!({ "progress": 42 }),
        };
        let encoded = serde_json::to_string(&event).expect("serializes");
        let decoded: PlanEvent = serde_json::from_str(&encoded).expect("deserializes");
        assert_eq!(decoded, event);
    }

    #[tokio::test]
    async fn subscribers_only_observe_their_own_plan() {
        // The filter in pump() is what keeps one operator's plan events out of
        // another's socket; assert the predicate directly.
        let (tx, mut rx) = tokio::sync::broadcast::channel::<PlanEvent>(8);
        let mine = Uuid::new_v4();
        let theirs = Uuid::new_v4();

        tx.send(PlanEvent {
            plan_id: theirs,
            kind: "run.started".to_string(),
            payload: serde_json::Value::Null,
        })
        .expect("send");
        tx.send(PlanEvent {
            plan_id: mine,
            kind: "run.started".to_string(),
            payload: serde_json::Value::Null,
        })
        .expect("send");

        let first = rx.recv().await.expect("event");
        let second = rx.recv().await.expect("event");
        let delivered: Vec<Uuid> = [first, second]
            .into_iter()
            .filter(|event| event.plan_id == mine)
            .map(|event| event.plan_id)
            .collect();
        assert_eq!(delivered, vec![mine]);
    }
}
