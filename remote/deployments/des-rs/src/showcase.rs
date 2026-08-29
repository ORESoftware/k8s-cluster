use axum::{extract::State, response::Html};
use serde_json::Value;

use des_engine::des::model::with_builtins;

use crate::output::html_escape;
use crate::state::AppState;

// =============================================================================
// Elevator showcase: the new FEL elevator sim + its MDP/POMDP dispatch models.
//
// All three are rendered once at startup into `AppState` (deterministic), so
// these routes just serve cached HTML — fast, lock-free, and always available.
// =============================================================================

/// `GET /elevator-fel` — the next-event (future-event-list) single-car elevator
/// under a LOOK policy, as a self-contained animated page.
pub(crate) async fn elevator_fel(State(state): State<AppState>) -> Html<String> {
    Html(state.elevator_fel_html.to_string())
}

/// `GET /elevator-mdp` — the fully-observed elevator-dispatch MDP, value-iterated
/// and rendered as an animated state-graph rollout player.
pub(crate) async fn elevator_mdp(State(state): State<AppState>) -> Html<String> {
    Html(state.elevator_mdp_html.to_string())
}

/// `GET /elevator-pomdp` — elevator dispatch under a noisy hall-call button,
/// rendered as a belief-tracking player.
pub(crate) async fn elevator_pomdp(State(state): State<AppState>) -> Html<String> {
    Html(state.elevator_pomdp_html.to_string())
}

/// `GET /bathrooms` — household bathroom occupancy Monte-Carlo, animated. A
/// blocking-loss DES (8 people, 2 bathrooms, 3×20-min visits/day) whose
/// time-weighted P(0)/P(1)/P(2)-occupied are checked against the closed-form
/// binomial. Deterministic and pre-rendered at startup.
pub(crate) async fn bathrooms(State(state): State<AppState>) -> Html<String> {
    Html(state.bathrooms_html.to_string())
}

/// `GET /two-bathrooms` — the same study built on the engine's entity +
/// animation frameworks (MovingEntity people, StationaryEntity bathrooms,
/// visual-block player). Deterministic, pre-rendered at startup.
pub(crate) async fn two_bathrooms(State(state): State<AppState>) -> Html<String> {
    Html(state.two_bathrooms_html.to_string())
}

/// Render the elevator MDP/POMDP players at startup, degrading to a small error
/// page (rather than panicking the server) if a solve ever fails.
pub(crate) fn render_model_player(kind: &str, spec: &Value) -> String {
    match with_builtins().run(kind, spec) {
        Ok(artifact) => artifact.to_player_html(),
        Err(err) => format!(
            "<!doctype html><html><head><meta charset=\"utf-8\"><title>{kind} unavailable</title>\
             </head><body style=\"font-family:system-ui;background:#0b1021;color:#e6edf3;padding:40px\">\
             <h1>elevator {kind} model unavailable</h1><p>{}</p></body></html>",
            html_escape(&err.to_string())
        ),
    }
}
