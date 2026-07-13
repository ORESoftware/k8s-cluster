use serde_json::{json, Value};

use crate::{config::SERVICE_NAME, util::now_unix_nano_string};

pub fn log_info(event_name: &str, body: &str, attributes: Value) {
    log("INFO", 9, event_name, body, attributes);
}

pub fn log_error(event_name: &str, body: &str, attributes: Value) {
    log("ERROR", 17, event_name, body, attributes);
}

fn log(severity_text: &str, severity_number: u8, event_name: &str, body: &str, attributes: Value) {
    let line = json!({
        "schema": "dd.log.v1",
        "time_unix_nano": now_unix_nano_string(),
        "severity_text": severity_text,
        "severity_number": severity_number,
        "body": body,
        "resource_service_name": SERVICE_NAME,
        "resource_service_namespace": "remote-dev",
        "scope_name": "dd-compliance-rs",
        "event_name": event_name,
        "attributes": attributes,
    });
    if severity_number >= 13 {
        tracing::error!("{line}");
    } else {
        tracing::info!("{line}");
    }
}
