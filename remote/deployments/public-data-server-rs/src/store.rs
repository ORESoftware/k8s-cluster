use std::collections::{BTreeMap, BTreeSet};

use crate::state::{
    AppState, MAX_ANALYSIS_STORE, MAX_LONG_TEXT_LEN, MAX_PIPELINE_JOBS, MAX_RECEIPT_STORE,
    MAX_RECORD_STORE, MAX_TEXT_LEN, MAX_TOKEN_LEN,
};
use crate::types::{AnalysisResult, DataRecord, IncomingRecord, PipelineJob, WebhookReceipt};
use crate::util::{
    clean_required, clean_tags, clean_text, durable_token, now_ms, validate_public_url,
};

pub(crate) fn normalize_record(
    incoming: IncomingRecord,
    fallback_source: &str,
    fallback_dataset: &str,
    fallback_url: Option<&String>,
    inherited_tags: &[String],
    index: usize,
) -> Result<DataRecord, String> {
    let source = incoming
        .source
        .as_deref()
        .unwrap_or(fallback_source)
        .trim()
        .to_string();
    let source = clean_required(&source, "source")?;
    let dataset_id = incoming
        .dataset_id
        .as_deref()
        .unwrap_or(fallback_dataset)
        .trim()
        .to_string();
    let dataset_id = clean_required(&dataset_id, "datasetId")?;
    let record_id = incoming
        .record_id
        .unwrap_or_else(|| durable_token("record", &source, &format!("{}-{index}", now_ms())));
    let source_url = incoming
        .source_url
        .or_else(|| fallback_url.cloned())
        .filter(|url| validate_public_url(url).is_ok());
    let mut tags = inherited_tags.to_vec();
    tags.extend(incoming.tags.unwrap_or_default());
    if let Some(grant) = incoming.grant.as_ref() {
        tags.extend(grant.topics.iter().cloned());
        tags.push("grant".to_string());
    }
    let metrics = incoming
        .metrics
        .unwrap_or_default()
        .into_iter()
        .filter(|(_, value)| value.is_finite())
        .map(|(key, value)| (key.chars().take(80).collect(), value))
        .collect::<BTreeMap<_, _>>();
    Ok(DataRecord {
        record_id: clean_required(&record_id, "recordId")?,
        dataset_id,
        source,
        source_url,
        title: clean_text(incoming.title.as_ref(), MAX_TEXT_LEN),
        summary: clean_text(incoming.summary.as_ref(), MAX_LONG_TEXT_LEN),
        published_at: clean_text(incoming.published_at.as_ref(), MAX_TOKEN_LEN),
        collected_at_ms: now_ms(),
        authors: incoming
            .authors
            .unwrap_or_default()
            .into_iter()
            .filter_map(|author| clean_text(Some(&author), MAX_TOKEN_LEN))
            .take(64)
            .collect(),
        tags: clean_tags(tags),
        metrics,
        grant: incoming.grant,
        raw: incoming.raw,
    })
}

pub(crate) fn store_records(state: &AppState, records: Vec<DataRecord>) {
    let mut store = state.store.write().unwrap_or_else(|lock| lock.into_inner());
    store.records.extend(records);
    if store.records.len() > MAX_RECORD_STORE {
        let overflow = store.records.len() - MAX_RECORD_STORE;
        store.records.drain(0..overflow);
    }
}

pub(crate) fn store_receipt(state: &AppState, receipt: WebhookReceipt) {
    let mut store = state.store.write().unwrap_or_else(|lock| lock.into_inner());
    store.webhook_receipts.push(receipt);
    if store.webhook_receipts.len() > MAX_RECEIPT_STORE {
        let overflow = store.webhook_receipts.len() - MAX_RECEIPT_STORE;
        store.webhook_receipts.drain(0..overflow);
    }
}

pub(crate) fn store_analysis(state: &AppState, result: AnalysisResult) {
    let mut store = state.store.write().unwrap_or_else(|lock| lock.into_inner());
    store.analyses.push(result);
    if store.analyses.len() > MAX_ANALYSIS_STORE {
        let overflow = store.analyses.len() - MAX_ANALYSIS_STORE;
        store.analyses.drain(0..overflow);
    }
}

pub(crate) fn store_pipeline_job(state: &AppState, job: PipelineJob) {
    let mut store = state.store.write().unwrap_or_else(|lock| lock.into_inner());
    store.pipeline_jobs.push(job);
    if store.pipeline_jobs.len() > MAX_PIPELINE_JOBS {
        let overflow = store.pipeline_jobs.len() - MAX_PIPELINE_JOBS;
        store.pipeline_jobs.drain(0..overflow);
    }
}

pub(crate) fn records_snapshot(state: &AppState) -> Vec<DataRecord> {
    state
        .store
        .read()
        .unwrap_or_else(|lock| lock.into_inner())
        .records
        .clone()
}

pub(crate) fn filter_records(
    records: &[DataRecord],
    dataset_ids: &Option<Vec<String>>,
    tags: &Option<Vec<String>>,
) -> Vec<DataRecord> {
    let dataset_filter = dataset_ids.as_ref().map(|values| {
        values
            .iter()
            .map(|value| value.trim().to_string())
            .collect::<BTreeSet<_>>()
    });
    let tag_filter = tags.as_ref().map(|values| {
        values
            .iter()
            .map(|value| value.trim().to_ascii_lowercase())
            .collect::<BTreeSet<_>>()
    });
    records
        .iter()
        .filter(|record| {
            dataset_filter
                .as_ref()
                .map(|filter| filter.contains(&record.dataset_id))
                .unwrap_or(true)
        })
        .filter(|record| {
            tag_filter
                .as_ref()
                .map(|filter| record.tags.iter().any(|tag| filter.contains(tag)))
                .unwrap_or(true)
        })
        .cloned()
        .collect()
}
