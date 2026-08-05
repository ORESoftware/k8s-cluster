use crate::inspect;
use crate::model::explain_code;
use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::env;
use std::io::{self, BufRead, Write};
use std::path::PathBuf;

const PROTOCOL_VERSION: &str = "2025-11-25";

type RpcResult = std::result::Result<Value, (i64, String)>;

pub fn serve() -> Result<()> {
    let stdin = io::stdin();
    let mut stdout = io::BufWriter::new(io::stdout().lock());

    for line in stdin.lock().lines() {
        let line = line.context("read MCP request")?;
        if line.trim().is_empty() {
            continue;
        }

        let response = match serde_json::from_str::<Value>(&line) {
            Ok(request) => handle(request),
            Err(error) => Some(rpc_error(
                Value::Null,
                -32700,
                format!("parse error: {error}"),
            )),
        };

        if let Some(response) = response {
            serde_json::to_writer(&mut stdout, &response)?;
            stdout.write_all(b"\n")?;
            stdout.flush()?;
        }
    }
    Ok(())
}

fn handle(request: Value) -> Option<Value> {
    let id = request.get("id").cloned();
    let method = request
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let params = request.get("params").cloned().unwrap_or_else(|| json!({}));

    let id = id?;
    let result: RpcResult = match method {
        "initialize" => Ok(json!({
            "protocolVersion": negotiated_protocol(&params),
            "capabilities": { "tools": { "listChanged": false } },
            "serverInfo": {
                "name": "zed-jetbrains-air",
                "version": env!("CARGO_PKG_VERSION"),
                "description": "Read-only Zed package diagnostics and recommended resolutions for JetBrains Air"
            }
        })),
        "server/discover" => Ok(json!({
            "protocolVersions": [PROTOCOL_VERSION],
            "capabilities": { "tools": {} },
            "serverInfo": {
                "name": "zed-jetbrains-air",
                "version": env!("CARGO_PKG_VERSION")
            }
        })),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tool_definitions() })),
        "tools/call" => call_tool(&params),
        _ => Err((-32601, format!("method not found: {method}"))),
    };

    Some(match result {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err((code, message)) => rpc_error(id, code, message),
    })
}

fn negotiated_protocol(params: &Value) -> String {
    params
        .get("protocolVersion")
        .and_then(Value::as_str)
        .filter(|version| *version == PROTOCOL_VERSION)
        .unwrap_or(PROTOCOL_VERSION)
        .to_owned()
}

fn tool_definitions() -> Vec<Value> {
    vec![
        json!({
            "name": "zed_project_status",
            "description": "Inspect real Zed package state without changing the project.",
            "inputSchema": root_schema()
        }),
        json!({
            "name": "zed_recommended_actions",
            "description": "Return prioritized commands and human fixes for detected Zed package problems. Commands are recommendations only and are never executed by this tool.",
            "inputSchema": root_schema()
        }),
        json!({
            "name": "zed_explain_diagnostic",
            "description": "Explain a stable Zed Air diagnostic code and its safe resolution.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "code": {
                        "type": "string",
                        "description": "Diagnostic code such as ZED003"
                    }
                },
                "required": ["code"],
                "additionalProperties": false
            }
        }),
    ]
}

fn root_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "root": {
                "type": "string",
                "description": "Workspace path to inspect. Defaults to the MCP server process working directory."
            }
        },
        "additionalProperties": false
    })
}

fn call_tool(params: &Value) -> RpcResult {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| (-32602, "tools/call requires `name`".to_owned()))?;
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));

    match name {
        "zed_project_status" => {
            let report = inspect_from_arguments(&arguments)?;
            let text = inspect::render(&report);
            let structured = serde_json::to_value(report).map_err(internal_error)?;
            Ok(tool_success(text, structured))
        }
        "zed_recommended_actions" => {
            let report = inspect_from_arguments(&arguments)?;
            let text = report
                .recommended_actions
                .iter()
                .map(|action| match &action.command {
                    Some(command) => {
                        format!("{}: `{}` — {}", action.title, command, action.rationale)
                    }
                    None => format!("{} — {}", action.title, action.rationale),
                })
                .collect::<Vec<_>>()
                .join("\n");
            Ok(tool_success(
                text,
                json!({
                    "root": report.root,
                    "summary": report.summary,
                    "recommended_actions": report.recommended_actions
                }),
            ))
        }
        "zed_explain_diagnostic" => {
            let code = arguments
                .get("code")
                .and_then(Value::as_str)
                .ok_or_else(|| (-32602, "zed_explain_diagnostic requires `code`".to_owned()))?;
            let explanation = explain_code(code);
            Ok(tool_success(
                explanation.clone(),
                json!({ "code": code, "explanation": explanation }),
            ))
        }
        _ => Err((-32602, format!("unknown tool `{name}`"))),
    }
}

fn inspect_from_arguments(
    arguments: &Value,
) -> std::result::Result<crate::model::ProjectReport, (i64, String)> {
    let root = arguments
        .get("root")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .or_else(|| env::current_dir().ok())
        .ok_or_else(|| (-32603, "cannot determine workspace root".to_owned()))?;
    inspect::project(&root).map_err(|error| (-32603, error.to_string()))
}

fn tool_success(text: String, structured: Value) -> Value {
    json!({
        "content": [{ "type": "text", "text": text }],
        "structuredContent": structured,
        "isError": false
    })
}

fn internal_error(error: serde_json::Error) -> (i64, String) {
    (-32603, error.to_string())
}

fn rpc_error(id: Value, code: i64, message: String) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_all_tools() {
        let response = handle(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/list",
            "params": {}
        }))
        .unwrap();

        assert_eq!(response["result"]["tools"].as_array().unwrap().len(), 3);
    }

    #[test]
    fn ignores_notifications() {
        let response = handle(json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        }));
        assert!(response.is_none());
    }
}
