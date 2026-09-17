//! Web tier handlers: server-rendered pages, htmx action proxies to t2v-api,
//! and the live-stats websocket.

use crate::state::AppState;
use crate::views::{self, DashboardStats};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::{Html, IntoResponse, Response};
use axum::Form;
use base64::Engine;
use maud::Markup;
use sea_orm::{EntityTrait, PaginatorTrait, QueryOrder, QuerySelect};
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::Duration;
use t2v_entity::{synthesis, transcription, translation, vapi_call};
use tokio::time;

pub async fn dashboard(State(state): State<AppState>) -> Markup {
    let stats = load_stats(&state).await;
    views::dashboard(&stats)
}

pub async fn translate_page() -> Markup {
    views::translate_page()
}

pub async fn speak_page() -> Markup {
    views::speak_page()
}

pub async fn history_page(State(state): State<AppState>) -> Markup {
    let translations = translation::Entity::find()
        .order_by_desc(translation::Column::CreatedAt)
        .limit(25)
        .all(&state.db)
        .await
        .unwrap_or_default();
    let calls = vapi_call::Entity::find()
        .order_by_desc(vapi_call::Column::UpdatedAt)
        .limit(25)
        .all(&state.db)
        .await
        .unwrap_or_default();
    views::history_page(&translations, &calls)
}

pub async fn healthz() -> &'static str {
    "ok\n"
}

/// Response-header hardening applied to every route.
///
/// The CSP is strict: everything loads from our own origin (htmx is vendored,
/// see [`crate::assets`]), the live-stats websocket is same-origin
/// (`connect-src 'self'`), and synthesized speech is inlined as `data:` audio
/// (`media-src data:`). No external hosts, no inline/eval scripts, no framing.
pub async fn security_headers(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    use axum::http::HeaderValue;
    // `script-src 'self'` stays strict — the important one (htmx is vendored
    // same-origin, no CDN, no inline/eval scripts). `style-src` additionally
    // allows 'unsafe-inline' because htmx injects a small inline <style> for
    // its indicator CSS at runtime; inline styles are low-risk and blocking
    // them breaks htmx's UI. Everything else is same-origin only.
    const CSP: &str = "default-src 'self'; \
         script-src 'self'; \
         style-src 'self' 'unsafe-inline'; \
         img-src 'self' data:; \
         media-src data:; \
         connect-src 'self'; \
         base-uri 'none'; \
         form-action 'self'; \
         frame-ancestors 'none'; \
         object-src 'none'";

    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(
        axum::http::header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(CSP),
    );
    headers.insert(
        axum::http::header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert("x-frame-options", HeaderValue::from_static("DENY"));
    headers.insert(
        axum::http::header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        "permissions-policy",
        HeaderValue::from_static("geolocation=(), microphone=(), camera=()"),
    );
    response
}

pub async fn readyz(State(state): State<AppState>) -> Response {
    use axum::http::StatusCode;
    match state.db.ping().await {
        Ok(_) => (StatusCode::OK, "ready\n").into_response(),
        Err(e) => {
            tracing::error!("readiness DB ping failed: {e}");
            (StatusCode::SERVICE_UNAVAILABLE, "not ready\n").into_response()
        }
    }
}

// ---------------------------------------------------------------------------
// htmx form actions — proxied to the t2v-api server.
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct TranslateForm {
    pub text: String,
    pub target_lang: String,
    pub provider: Option<String>,
}

pub async fn translate_action(
    State(state): State<AppState>,
    Form(form): Form<TranslateForm>,
) -> Html<String> {
    let body = json!({
        "text": form.text,
        "target_lang": form.target_lang,
        "provider": form.provider.unwrap_or_else(|| "openai".to_string()),
    });
    let markup = match post_api(&state, "/v1/translate", &body).await {
        Ok(v) => {
            let t = &v["translation"];
            views::translate_result(
                t["translatedText"].as_str().unwrap_or(""),
                t["provider"].as_str().unwrap_or(""),
                t["model"].as_str().unwrap_or(""),
                t["latencyMs"].as_i64().unwrap_or(0),
            )
        }
        Err(e) => views::error_fragment(&e),
    };
    Html(markup.into_string())
}

#[derive(Debug, Deserialize)]
pub struct SpeakForm {
    pub text: String,
    pub voice: Option<String>,
}

pub async fn speak_action(
    State(state): State<AppState>,
    Form(form): Form<SpeakForm>,
) -> Html<String> {
    let voice = form.voice.filter(|v| !v.trim().is_empty());
    let body = json!({
        "text": form.text,
        "voice": voice,
        "format": "mp3",
    });
    // TTS returns raw audio bytes; fetch them and inline as a data: URL.
    let markup = match post_api_bytes(&state, "/v1/tts", &body).await {
        Ok((bytes, content_type)) => {
            let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
            let data_url = format!("data:{content_type};base64,{b64}");
            views::tts_result(&data_url, voice_label(&body), bytes.len())
        }
        Err(e) => views::error_fragment(&e),
    };
    Html(markup.into_string())
}

fn voice_label(body: &Value) -> &str {
    body.get("voice")
        .and_then(Value::as_str)
        .unwrap_or("default")
}

/// POST JSON to the API server, expect a JSON envelope `{ok, ...}`.
async fn post_api(state: &AppState, path: &str, body: &Value) -> Result<Value, String> {
    let url = format!("{}{}", state.api_base, path);
    let resp = state
        .http
        .post(&url)
        .json(body)
        .send()
        .await
        .map_err(|e| format!("could not reach the API server: {e}"))?;
    let status = resp.status();
    let value: Value = resp
        .json()
        .await
        .map_err(|e| format!("bad response from API server: {e}"))?;
    if !status.is_success() || value.get("ok").and_then(Value::as_bool) == Some(false) {
        let msg = value
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("the API server returned an error");
        return Err(msg.to_string());
    }
    Ok(value)
}

/// POST JSON to the API server, expect raw bytes back (audio).
async fn post_api_bytes(
    state: &AppState,
    path: &str,
    body: &Value,
) -> Result<(Vec<u8>, String), String> {
    let url = format!("{}{}", state.api_base, path);
    let resp = state
        .http
        .post(&url)
        .json(body)
        .send()
        .await
        .map_err(|e| format!("could not reach the API server: {e}"))?;
    let status = resp.status();
    let content_type = resp
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("audio/mpeg")
        .to_string();
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("bad response from API server: {e}"))?;
    if !status.is_success() {
        // The error path returns JSON even though we asked for bytes.
        if let Ok(v) = serde_json::from_slice::<Value>(&bytes) {
            if let Some(msg) = v.get("error").and_then(Value::as_str) {
                return Err(msg.to_string());
            }
        }
        return Err(format!("API server returned {status}"));
    }
    Ok((bytes.to_vec(), content_type))
}

// ---------------------------------------------------------------------------
// Live stats websocket — pushes htmx out-of-band swaps every 2s.
// ---------------------------------------------------------------------------

pub async fn stats_ws(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    ws.on_upgrade(move |socket| stream_stats(socket, state))
}

async fn stream_stats(mut socket: WebSocket, state: AppState) {
    let mut interval = time::interval(Duration::from_secs(2));
    loop {
        interval.tick().await;
        let stats = load_stats(&state).await;
        // Each card carries hx-swap-oob="true" itself, so htmx replaces the
        // matching #stat-* node in place — no wrapper element sharing the id.
        let frame = maud::html! {
            (views::metric_card("stat-transcriptions", stats.transcriptions, "transcriptions", true))
            (views::metric_card("stat-translations", stats.translations, "translations", true))
            (views::metric_card("stat-syntheses", stats.syntheses, "syntheses", true))
            (views::metric_card("stat-vapi", stats.vapi_calls, "vapi calls", true))
        }
        .into_string();

        if socket.send(Message::Text(frame)).await.is_err() {
            break; // client went away
        }
    }
}

async fn load_stats(state: &AppState) -> DashboardStats {
    DashboardStats {
        transcriptions: transcription::Entity::find()
            .count(&state.db)
            .await
            .unwrap_or(0),
        translations: translation::Entity::find()
            .count(&state.db)
            .await
            .unwrap_or(0),
        syntheses: synthesis::Entity::find()
            .count(&state.db)
            .await
            .unwrap_or(0),
        vapi_calls: vapi_call::Entity::find()
            .count(&state.db)
            .await
            .unwrap_or(0),
    }
}
