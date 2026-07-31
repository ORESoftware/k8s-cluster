//! Vapi.ai server webhook + REST passthrough.
//!
//! Vapi posts call-lifecycle events here. Because this service is a
//! translation/voice platform, the assistant we expose is a live phone
//! translator: it calls back a `translate_text` server-tool that runs our
//! LLM translation, and we persist call status, transcript, and summary.
//!
//! Auth and the `{ "results": [...] }` tool-call response shape follow the
//! fleet's existing Vapi screener.

use crate::error::ApiError;
use crate::handlers_speech::run_translation;
use crate::metrics::Metrics;
use crate::state::AppState;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::Utc;
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use serde_json::{json, Value};
use t2v_entity::{vapi_call, vapi_event};
use uuid::Uuid;

const VAPI_SECRET_HEADER: &str = "x-vapi-secret";
/// Cap on the raw payload we persist to vapi_events (bytes of serialized JSON).
const MAX_EVENT_PAYLOAD: usize = 200_000;

fn json_response(status: StatusCode, body: Value) -> Response {
    (status, Json(body)).into_response()
}

fn webhook_authorized(headers: &HeaderMap, state: &AppState) -> bool {
    match &state.vapi_webhook_secret {
        // Fail closed unless an operator explicitly opted into insecure dev mode.
        None => state.allow_insecure_webhook,
        Some(expected) => headers
            .get(VAPI_SECRET_HEADER)
            .and_then(|v| v.to_str().ok())
            .map(|got| crate::auth::constant_time_eq(got, expected))
            .unwrap_or(false),
    }
}

pub async fn webhook(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    Metrics::bump(&state.metrics.vapi_webhook_events_total);

    if !webhook_authorized(&headers, &state) {
        Metrics::bump(&state.metrics.vapi_webhook_unauthorized_total);
        return json_response(
            StatusCode::UNAUTHORIZED,
            json!({ "ok": false, "error": "invalid x-vapi-secret header" }),
        );
    }

    let payload: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            Metrics::bump(&state.metrics.errors_total);
            return json_response(
                StatusCode::BAD_REQUEST,
                json!({ "ok": false, "error": format!("invalid JSON body: {e}") }),
            );
        }
    };

    // Vapi nests the interesting fields under `message`.
    let message = payload.get("message").cloned().unwrap_or(payload);
    let event_type = message
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();
    let call_id = extract_call_id(&message);

    // Best-effort audit; never fail the webhook because the audit insert failed.
    if let Err(e) = persist_event(&state, call_id.as_deref(), &event_type, &message).await {
        tracing::error!("t2v vapi event persist failed: {e}");
    }

    match event_type.as_str() {
        "assistant-request" => json_response(
            StatusCode::OK,
            json!({
                "assistant": translator_assistant(
                    &state.vapi_assistant,
                    state.vapi_webhook_secret.as_deref(),
                )
            }),
        ),
        "tool-calls" => handle_tool_calls(&state, &message).await,
        "status-update" => {
            if let (Some(id), Some(status)) = (
                call_id.as_deref(),
                message.get("status").and_then(Value::as_str),
            ) {
                if let Err(e) = upsert_call_status(&state, id, status).await {
                    tracing::error!("t2v vapi status upsert failed: {e}");
                }
            }
            json_response(StatusCode::OK, json!({ "ok": true }))
        }
        "end-of-call-report" => {
            if let Some(id) = call_id.as_deref() {
                if let Err(e) = persist_end_of_call(&state, id, &message).await {
                    tracing::error!("t2v vapi end-of-call persist failed: {e}");
                }
            }
            json_response(StatusCode::OK, json!({ "ok": true }))
        }
        _ => json_response(StatusCode::OK, json!({ "ok": true })),
    }
}

fn extract_call_id(message: &Value) -> Option<String> {
    message
        .get("call")
        .and_then(|c| c.get("id"))
        .and_then(Value::as_str)
        .or_else(|| message.get("callId").and_then(Value::as_str))
        .map(str::to_string)
}

async fn persist_event(
    state: &AppState,
    call_id: Option<&str>,
    event_type: &str,
    message: &Value,
) -> Result<(), ApiError> {
    // Keep the audit row bounded regardless of provider payload size.
    let payload = match serde_json::to_vec(message) {
        Ok(bytes) if bytes.len() <= MAX_EVENT_PAYLOAD => message.clone(),
        _ => json!({ "truncated": true, "type": event_type }),
    };
    vapi_event::ActiveModel {
        id: Set(Uuid::new_v4()),
        vapi_call_id: Set(call_id.map(str::to_string)),
        event_type: Set(event_type.to_string()),
        payload: Set(payload),
        created_at: Set(Utc::now()),
    }
    .insert(&state.db)
    .await?;
    Ok(())
}

async fn upsert_call_status(state: &AppState, call_id: &str, status: &str) -> Result<(), ApiError> {
    let existing = vapi_call::Entity::find()
        .filter(vapi_call::Column::VapiCallId.eq(call_id))
        .one(&state.db)
        .await?;
    let now = Utc::now();
    match existing {
        Some(row) => {
            let mut active: vapi_call::ActiveModel = row.into();
            active.status = Set(status.to_string());
            active.updated_at = Set(now);
            active.update(&state.db).await?;
        }
        None => {
            vapi_call::ActiveModel {
                id: Set(Uuid::new_v4()),
                vapi_call_id: Set(call_id.to_string()),
                status: Set(status.to_string()),
                ended_reason: Set(None),
                transcript: Set(None),
                summary: Set(None),
                created_at: Set(now),
                updated_at: Set(now),
            }
            .insert(&state.db)
            .await?;
        }
    }
    Ok(())
}

async fn persist_end_of_call(
    state: &AppState,
    call_id: &str,
    message: &Value,
) -> Result<(), ApiError> {
    let ended_reason = message.get("endedReason").and_then(Value::as_str);
    let transcript = message.get("transcript").and_then(Value::as_str);
    let summary = message.get("summary").and_then(Value::as_str).or_else(|| {
        message
            .get("analysis")
            .and_then(|a| a.get("summary"))
            .and_then(Value::as_str)
    });

    let existing = vapi_call::Entity::find()
        .filter(vapi_call::Column::VapiCallId.eq(call_id))
        .one(&state.db)
        .await?;
    let now = Utc::now();
    match existing {
        Some(row) => {
            let mut active: vapi_call::ActiveModel = row.into();
            active.status = Set("ended".to_string());
            active.ended_reason = Set(ended_reason.map(str::to_string));
            if let Some(t) = transcript {
                active.transcript = Set(Some(t.to_string()));
            }
            if let Some(s) = summary {
                active.summary = Set(Some(s.to_string()));
            }
            active.updated_at = Set(now);
            active.update(&state.db).await?;
        }
        None => {
            vapi_call::ActiveModel {
                id: Set(Uuid::new_v4()),
                vapi_call_id: Set(call_id.to_string()),
                status: Set("ended".to_string()),
                ended_reason: Set(ended_reason.map(str::to_string)),
                transcript: Set(transcript.map(str::to_string)),
                summary: Set(summary.map(str::to_string)),
                created_at: Set(now),
                updated_at: Set(now),
            }
            .insert(&state.db)
            .await?;
        }
    }
    Ok(())
}

/// A tool call as Vapi presents it, normalized across the two list shapes.
struct ToolCall {
    id: String,
    name: String,
    arguments: Value,
}

fn extract_tool_calls(message: &Value) -> Vec<ToolCall> {
    let mut calls = Vec::new();
    for key in ["toolCallList", "toolWithToolCallList"] {
        if let Some(list) = message.get(key).and_then(Value::as_array) {
            for item in list {
                // Either the item itself is the tool call, or it wraps `toolCall`.
                let tc = item.get("toolCall").unwrap_or(item);
                let id = tc
                    .get("id")
                    .and_then(Value::as_str)
                    .or_else(|| item.get("id").and_then(Value::as_str));
                let func = tc.get("function").or_else(|| item.get("function"));
                let name = func
                    .and_then(|f| f.get("name"))
                    .and_then(Value::as_str)
                    .or_else(|| tc.get("name").and_then(Value::as_str));
                if let (Some(id), Some(name)) = (id, name) {
                    let arguments = normalize_arguments(func.and_then(|f| f.get("arguments")));
                    calls.push(ToolCall {
                        id: id.to_string(),
                        name: name.to_string(),
                        arguments,
                    });
                }
            }
        }
    }
    calls
}

/// Vapi sends tool arguments either as a JSON object or a JSON-encoded string.
fn normalize_arguments(raw: Option<&Value>) -> Value {
    match raw {
        Some(Value::String(s)) => serde_json::from_str(s).unwrap_or(Value::Null),
        Some(other) => other.clone(),
        None => Value::Null,
    }
}

async fn handle_tool_calls(state: &AppState, message: &Value) -> Response {
    let calls = extract_tool_calls(message);
    if calls.is_empty() {
        return json_response(
            StatusCode::BAD_REQUEST,
            json!({ "ok": false, "error": "tool-calls message did not include tool calls" }),
        );
    }

    let mut results = Vec::with_capacity(calls.len());
    for call in calls {
        Metrics::bump(&state.metrics.vapi_tool_calls_total);
        let result = match call.name.as_str() {
            "translate_text" => run_translate_tool(state, &call.arguments).await,
            other => Err(format!("unknown tool '{other}'")),
        };
        let result_value = match result {
            Ok(v) => v,
            Err(e) => {
                Metrics::bump(&state.metrics.errors_total);
                json!({ "ok": false, "error": e })
            }
        };
        results.push(json!({
            "toolCallId": call.id,
            "name": call.name,
            "result": result_value,
        }));
    }

    json_response(StatusCode::OK, json!({ "results": results }))
}

async fn run_translate_tool(state: &AppState, args: &Value) -> Result<Value, String> {
    let text = args
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if text.is_empty() {
        return Err("translate_text requires a non-empty 'text' argument".to_string());
    }
    let target_lang = args
        .get("target_lang")
        .or_else(|| args.get("targetLang"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if target_lang.is_empty() {
        return Err("translate_text requires a 'target_lang' argument".to_string());
    }
    let source_lang = args
        .get("source_lang")
        .or_else(|| args.get("sourceLang"))
        .and_then(Value::as_str);
    let provider = args.get("provider").and_then(Value::as_str);

    let row = run_translation(state, text, target_lang, source_lang, provider)
        .await
        .map_err(|e| e.message)?;
    Ok(json!({
        "ok": true,
        "translatedText": row.translated_text,
        "targetLang": row.target_lang,
        "provider": row.provider,
    }))
}

/// The live-translator assistant config returned for `assistant-request`.
/// The model calls back our `translate_text` server tool; `{{SERVER_URL}}` is
/// filled in by Vapi from the assistant's configured server, so the tool posts
/// to this same webhook.
fn translator_assistant() -> Value {
    json!({
        "name": "t2v Live Translator",
        "firstMessage": "Hi! I can translate between languages in real time. What would you like translated, and into which language?",
        "firstMessageMode": "assistant-speaks-first",
        "serverMessages": ["assistant-request", "tool-calls", "status-update", "end-of-call-report"],
        "model": {
            "provider": "openai",
            "model": "gpt-4o",
            "temperature": 0.2,
            "messages": [{
                "role": "system",
                "content": "You are a real-time voice translator. When the caller gives you text to translate and a target language, call the translate_text tool and then read back ONLY the translation clearly. Ask for the target language if it is missing. Keep spoken output concise."
            }],
            "tools": [{
                "type": "function",
                "function": {
                    "name": "translate_text",
                    "description": "Translate text into a target language using the platform's translation engine.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "text": { "type": "string", "description": "The text to translate." },
                            "target_lang": { "type": "string", "description": "Target language, e.g. 'Spanish' or 'es'." },
                            "source_lang": { "type": "string", "description": "Optional source language; omit to auto-detect." },
                            "provider": { "type": "string", "enum": ["openai", "gemini", "anthropic"], "description": "Optional translation provider." }
                        },
                        "required": ["text", "target_lang"]
                    }
                }
            }]
        }
    })
}

// ---------------------------------------------------------------------------
// Operator-facing REST passthrough to Vapi (server-auth via VAPI_API_KEY).
// ---------------------------------------------------------------------------

pub async fn create_call(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let call = state.vapi.create_call(&body).await?;
    Ok(Json(json!({ "ok": true, "call": call })))
}

pub async fn get_call(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let call = state.vapi.get_call(&id).await?;
    Ok(Json(json!({ "ok": true, "call": call })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_tool_call_from_toolcalllist() {
        let msg = json!({
            "toolCallList": [{
                "id": "call_1",
                "function": { "name": "translate_text", "arguments": { "text": "hi", "target_lang": "es" } }
            }]
        });
        let calls = extract_tool_calls(&msg);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "call_1");
        assert_eq!(calls[0].name, "translate_text");
        assert_eq!(calls[0].arguments.get("target_lang").unwrap(), "es");
    }

    #[test]
    fn normalizes_string_encoded_arguments() {
        let msg = json!({
            "toolWithToolCallList": [{
                "toolCall": {
                    "id": "call_2",
                    "function": { "name": "translate_text", "arguments": "{\"text\":\"hola\",\"target_lang\":\"en\"}" }
                }
            }]
        });
        let calls = extract_tool_calls(&msg);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "call_2");
        assert_eq!(calls[0].arguments.get("text").unwrap(), "hola");
    }

    #[test]
    fn extract_call_id_handles_both_shapes() {
        assert_eq!(
            extract_call_id(&json!({ "call": { "id": "abc" } })).as_deref(),
            Some("abc")
        );
        assert_eq!(
            extract_call_id(&json!({ "callId": "xyz" })).as_deref(),
            Some("xyz")
        );
        assert_eq!(extract_call_id(&json!({})), None);
    }

    #[test]
    fn translator_assistant_declares_translate_tool() {
        let a = translator_assistant();
        let tool_name = a
            .pointer("/model/tools/0/function/name")
            .and_then(Value::as_str);
        assert_eq!(tool_name, Some("translate_text"));
    }
}
