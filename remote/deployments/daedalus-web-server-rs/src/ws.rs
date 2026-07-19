//! Websocket that pushes rendered **HTML fragments** to the browser.
//!
//! This is the htmx-ws pattern: the client's `<section hx-ext="ws"
//! ws-connect=...>` opens a socket, and each text frame we send is an HTML
//! fragment that htmx swaps in by matching element id (`#runs`). The server
//! renders with the same `views::runs_fragment` used for the initial paint, so
//! there is exactly one definition of how a run row looks.
//!
//! Unlike the API server's in-memory broadcast bus, this process learns about
//! run changes by polling the `daedalus` schema. The web tier is a separate
//! deployment from the writer, so a shared in-memory channel is not available;
//! polling the database is the honest cross-process mechanism. Clients that want
//! true low-latency push subscribe to Supabase Realtime directly — that is the
//! other half of the org's websocket story.

use std::time::Duration;

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Path, State,
    },
    response::IntoResponse,
};
use uuid::Uuid;

use crate::{error::ServiceError, routes, supabase_auth::Operator, views, SharedState};

/// How often the socket re-reads the plan's runs. Kept modest: this is a
/// convenience live-view, not a hard-real-time channel.
const POLL_INTERVAL: Duration = Duration::from_secs(2);

pub(crate) async fn plan_runs(
    State(state): State<SharedState>,
    operator: Operator,
    Path(id): Path<Uuid>,
    upgrade: WebSocketUpgrade,
) -> Result<impl IntoResponse, ServiceError> {
    // Authorize and ownership-check before upgrading — once the socket is open
    // there is no clean way to return an HTTP status.
    let db = state.persistence.connection()?;
    routes::owned_plan(db, id, &operator).await?;
    Ok(upgrade.on_upgrade(move |socket| pump(socket, state, id)))
}

/// Poll the plan's runs and push a fresh `#runs` fragment whenever it changes.
async fn pump(mut socket: WebSocket, state: SharedState, plan_id: Uuid) {
    let mut last_html: Option<String> = None;
    let mut ticker = tokio::time::interval(POLL_INTERVAL);
    loop {
        tokio::select! {
            // Client-initiated close (or any inbound frame error) ends the loop.
            inbound = socket.recv() => {
                match inbound {
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Err(_)) => break,
                    _ => {}
                }
            }
            _ = ticker.tick() => {
                let Ok(db) = state.persistence.connection() else { break; };
                let runs = match routes::runs_for_plan(db, plan_id).await {
                    Ok(runs) => runs,
                    // A transient DB error should not tear the socket down; the
                    // next tick retries.
                    Err(_) => continue,
                };
                let html = views::runs_fragment(&runs).into_string();
                // Only send when something actually changed, so an idle plan
                // does not stream identical frames every two seconds.
                if last_html.as_deref() != Some(html.as_str()) {
                    state.metrics.record_page();
                    if socket.send(Message::Text(html.clone())).await.is_err() {
                        break; // client disconnected
                    }
                    last_html = Some(html);
                }
            }
        }
    }
}
