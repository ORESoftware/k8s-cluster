use std::{
    collections::{BTreeMap, BTreeSet},
    sync::atomic::Ordering,
};

use serde_json::{json, Value};

use crate::nats::{publish_json, publish_runtime_event};
use crate::pipeline::maybe_submit_pipeline_job;
use crate::state::{
    AppState, MAX_LONG_TEXT_LEN, MAX_RECORDS_PER_REQUEST, SCHEMA_VERSION, SERVICE_NAME,
};
use crate::store::{normalize_record, store_records, store_receipt};
use crate::types::{
    IncomingRecord, IngestRequest, ScrapeRequest, ScraperResponse, WebhookIngestRequest,
    WebhookReceipt,
};
use crate::util::{
    clean_required, clean_tags, durable_token, now_ms, request_id, validate_public_url,
};

pub(crate) async fn process_ingest_request(state: &AppState, request: IngestRequest) -> Result<Value, String> {
    if request.records.len() > MAX_RECORDS_PER_REQUEST {
        return Err(format!(
            "records length must be at most {MAX_RECORDS_PER_REQUEST}"
        ));
    }
    let source = clean_required(&request.source, "source")?;
    if let Some(url) = request.source_url.as_ref() {
        validate_public_url(url)?;
    }
    let request_id = request_id(request.request_id.as_ref(), "ingest");
    let dataset_id = request
        .dataset_id
        .clone()
        .unwrap_or_else(|| durable_token("dataset", &source, &request_id));
    let inherited_tags = clean_tags(request.tags.unwrap_or_default());
    let mut records = Vec::new();
    for (index, incoming) in request.records.into_iter().enumerate() {
        records.push(normalize_record(
            incoming,
            &source,
            &dataset_id,
            request.source_url.as_ref(),
            &inherited_tags,
            index,
        )?);
    }
    let dataset_ids = records
        .iter()
        .map(|record| record.dataset_id.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let record_count = records.len();
    store_records(state, records.clone());
    state
        .metrics
        .records_ingested_total
        .fetch_add(record_count as u64, Ordering::Relaxed);
    let pipeline_job = maybe_submit_pipeline_job(
        state,
        &request_id,
        dataset_ids.clone(),
        Vec::new(),
        request.pipeline,
    )
    .await;
    let response = json!({
        "ok": true,
        "requestId": request_id,
        "schemaVersion": SCHEMA_VERSION,
        "source": source,
        "datasetIds": dataset_ids,
        "recordCount": record_count,
        "pipelineJob": pipeline_job,
        "ingestedAtMs": now_ms()
    });
    publish_json(
        state,
        &state.config.ingest_result_subject,
        &json!({
            "type": "public_data.ingest",
            "source": SERVICE_NAME,
            "result": response
        }),
    )
    .await;
    publish_runtime_event(
        state,
        "public_data.ingest",
        json!({ "recordCount": record_count, "source": source }),
    )
    .await;
    Ok(response)
}

pub(crate) async fn process_webhook(state: &AppState, request: WebhookIngestRequest) -> Result<Value, String> {
    let request_id = request_id(request.request_id.as_ref(), "webhook");
    let provider = clean_required(&request.provider, "provider")?;
    if let Some(url) = request.source_url.as_ref() {
        validate_public_url(url)?;
    }
    let records = request.records.unwrap_or_default();
    if records.len() > MAX_RECORDS_PER_REQUEST {
        return Err(format!(
            "records length must be at most {MAX_RECORDS_PER_REQUEST}"
        ));
    }
    let dataset_id = request
        .dataset_id
        .clone()
        .unwrap_or_else(|| durable_token("webhook-dataset", &provider, &request_id));
    let mut normalized = Vec::new();
    for (index, incoming) in records.into_iter().enumerate() {
        normalized.push(normalize_record(
            incoming,
            &provider,
            &dataset_id,
            request.source_url.as_ref(),
            &["webhook".to_string(), provider.clone()],
            index,
        )?);
    }
    let record_count = normalized.len();
    if record_count > 0 {
        store_records(state, normalized);
        state
            .metrics
            .records_ingested_total
            .fetch_add(record_count as u64, Ordering::Relaxed);
    }
    let event_type = request
        .event_type
        .unwrap_or_else(|| "provider.push".to_string());
    let receipt = WebhookReceipt {
        receipt_id: durable_token("public-data-webhook", &provider, &request_id),
        provider: provider.clone(),
        event_type: event_type.clone(),
        dataset_id: Some(dataset_id.clone()),
        source_url: request.source_url,
        received_at_ms: now_ms(),
        record_count,
        payload_shape: payload_shape(&request.payload),
    };
    store_receipt(state, receipt.clone());
    state
        .metrics
        .webhook_receipts_total
        .fetch_add(1, Ordering::Relaxed);
    let pipeline_job = maybe_submit_pipeline_job(
        state,
        &request_id,
        vec![dataset_id.clone()],
        Vec::new(),
        request.pipeline,
    )
    .await;
    let response = json!({
        "ok": true,
        "requestId": request_id,
        "receipt": receipt,
        "recordCount": record_count,
        "pipelineJob": pipeline_job
    });
    publish_json(
        state,
        &state.config.webhook_event_subject,
        &json!({
            "type": "public_data.webhook",
            "source": SERVICE_NAME,
            "receipt": response["receipt"]
        }),
    )
    .await;
    publish_json(
        state,
        &state.config.ingest_result_subject,
        &json!({
            "type": "public_data.webhook_ingest",
            "source": SERVICE_NAME,
            "result": response
        }),
    )
    .await;
    publish_runtime_event(
        state,
        "public_data.webhook",
        json!({ "provider": provider, "eventType": event_type, "recordCount": record_count }),
    )
    .await;
    Ok(response)
}

pub(crate) fn payload_shape(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let keys = map.keys().take(64).cloned().collect::<Vec<_>>();
            json!({ "type": "object", "keys": keys, "keyCount": map.len() })
        }
        Value::Array(values) => json!({ "type": "array", "length": values.len() }),
        Value::String(text) => json!({ "type": "string", "length": text.len() }),
        Value::Number(_) => json!({ "type": "number" }),
        Value::Bool(_) => json!({ "type": "boolean" }),
        Value::Null => json!({ "type": "null" }),
    }
}

pub(crate) async fn process_scrape_request(state: &AppState, request: ScrapeRequest) -> Result<Value, String> {
    validate_public_url(&request.url)?;
    let request_id = request_id(request.request_id.as_ref(), "scrape");
    let scrape_url = format!(
        "{}/scrape",
        state.config.scraper_base_url.trim_end_matches('/')
    );
    let mut body = json!({
        "requestId": request_id,
        "url": request.url.clone(),
        "strategy": request.strategy.clone().unwrap_or_else(|| "auto".to_string()),
        "renderJavaScript": request.render_javascript,
        "selector": request.selector.clone(),
        "selectors": request.selectors.clone(),
        "includeText": true,
        "includeLinks": request.include_links.unwrap_or(true),
        "maxTextChars": MAX_LONG_TEXT_LEN,
        "timeoutMs": 60000
    });
    strip_null_fields(&mut body);
    let mut builder = state.http.post(scrape_url).json(&body);
    if let Some(secret) = state.config.scraper_auth_secret.as_ref() {
        builder = builder.header("x-server-auth", secret);
    }
    let response = builder
        .send()
        .await
        .map_err(|error| format!("scraper request failed: {error}"))?;
    let status = response.status();
    let value = response
        .json::<Value>()
        .await
        .map_err(|error| format!("scraper response was not JSON: {error}"))?;
    if !status.is_success() {
        return Err(format!(
            "scraper returned {status}: {}",
            compact_json(&value)
        ));
    }
    let scraper_response: ScraperResponse = serde_json::from_value(value.clone())
        .map_err(|error| format!("scraper response shape mismatch: {error}"))?;
    let extraction = scraper_response.extraction.clone();
    let dataset_id = request
        .dataset_id
        .clone()
        .unwrap_or_else(|| durable_token("scrape-dataset", &request.source, &request_id));
    let mut metrics = BTreeMap::new();
    if let Some(extraction) = extraction.as_ref() {
        metrics.insert(
            "linkCount".to_string(),
            extraction
                .links
                .as_ref()
                .map(|links| links.len())
                .unwrap_or(0) as f64,
        );
        metrics.insert(
            "textLength".to_string(),
            extraction.text.as_ref().map(|text| text.len()).unwrap_or(0) as f64,
        );
    }
    let incoming = IncomingRecord {
        record_id: Some(durable_token("scrape-record", &request.source, &request_id)),
        dataset_id: Some(dataset_id.clone()),
        source: Some(request.source.clone()),
        source_url: scraper_response
            .final_url
            .clone()
            .or_else(|| Some(request.url.clone())),
        title: extraction.as_ref().and_then(|item| item.title.clone()),
        summary: extraction.as_ref().and_then(|item| item.text.clone()),
        published_at: None,
        authors: None,
        tags: request.tags.clone(),
        metrics: Some(metrics),
        grant: None,
        raw: Some(value.clone()),
    };
    let record = normalize_record(
        incoming,
        &request.source,
        &dataset_id,
        Some(&request.url),
        &clean_tags(vec!["scrape".to_string(), request.source.clone()]),
        0,
    )?;
    store_records(state, vec![record.clone()]);
    state
        .metrics
        .records_ingested_total
        .fetch_add(1, Ordering::Relaxed);
    state
        .metrics
        .scrape_requests_total
        .fetch_add(1, Ordering::Relaxed);
    let pipeline_job = maybe_submit_pipeline_job(
        state,
        &request_id,
        vec![dataset_id.clone()],
        Vec::new(),
        request.pipeline,
    )
    .await;
    let result = json!({
        "ok": true,
        "requestId": request_id,
        "source": request.source,
        "datasetId": dataset_id,
        "record": record,
        "scraper": scraper_response,
        "pipelineJob": pipeline_job
    });
    publish_json(
        state,
        &state.config.ingest_result_subject,
        &json!({
            "type": "public_data.scrape",
            "source": SERVICE_NAME,
            "result": result
        }),
    )
    .await;
    publish_runtime_event(
        state,
        "public_data.scrape",
        json!({ "datasetId": dataset_id }),
    )
    .await;
    Ok(result)
}

pub(crate) fn strip_null_fields(value: &mut Value) {
    if let Value::Object(map) = value {
        map.retain(|_, nested| !nested.is_null());
    }
}

pub(crate) fn compact_json(value: &Value) -> String {
    serde_json::to_string(value)
        .unwrap_or_else(|_| "{}".to_string())
        .chars()
        .take(500)
        .collect()
}
