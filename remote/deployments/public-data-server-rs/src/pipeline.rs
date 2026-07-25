use std::sync::atomic::Ordering;

use serde_json::json;

use crate::nats::{publish_json, publish_runtime_event};
use crate::state::{AppState, MAX_TOKEN_LEN, SERVICE_NAME};
use crate::store::store_pipeline_job;
use crate::types::{PipelineJob, PipelineOptions, PipelineRequest};
use crate::util::{durable_token, now_ms, request_id};

pub(crate) async fn maybe_submit_pipeline_job(
    state: &AppState,
    request_id: &str,
    dataset_ids: Vec<String>,
    analysis_ids: Vec<String>,
    options: Option<PipelineOptions>,
) -> Option<PipelineJob> {
    let Some(options) = options else {
        return None;
    };
    if options.enabled == Some(false) {
        return None;
    }
    let request = PipelineRequest {
        request_id: Some(request_id.to_string()),
        job_type: options.job_type,
        dataset_ids: Some(dataset_ids),
        analysis_ids: Some(analysis_ids),
        sink: options.sink,
        airflow_dag: options.airflow_dag,
        spark_app: options.spark_app,
        parameters: options.parameters,
    };
    match create_pipeline_job(state, request).await {
        Ok(job) => Some(job),
        Err(error) => {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            tracing::error!("public-data pipeline job creation failed: {error}");
            None
        }
    }
}

pub(crate) async fn create_pipeline_job(
    state: &AppState,
    request: PipelineRequest,
) -> Result<PipelineJob, String> {
    let request_id = request_id(request.request_id.as_ref(), "pipeline");
    let job_id = durable_token("public-data-job", &request_id, &now_ms().to_string());
    let job = PipelineJob {
        job_id,
        request_id,
        job_type: request
            .job_type
            .unwrap_or_else(|| "spark-etl".to_string())
            .chars()
            .take(MAX_TOKEN_LEN)
            .collect(),
        status: "queued".to_string(),
        dataset_ids: request.dataset_ids.unwrap_or_default(),
        analysis_ids: request.analysis_ids.unwrap_or_default(),
        sink: request
            .sink
            .unwrap_or_else(|| "minio://public-data/bronze".to_string())
            .chars()
            .take(MAX_TOKEN_LEN)
            .collect(),
        airflow_dag: request
            .airflow_dag
            .map(|value| value.chars().take(MAX_TOKEN_LEN).collect()),
        spark_app: request
            .spark_app
            .map(|value| value.chars().take(MAX_TOKEN_LEN).collect()),
        parameters: request.parameters.unwrap_or_else(|| json!({})),
        submitted_at_ms: now_ms(),
    };
    store_pipeline_job(state, job.clone());
    state
        .metrics
        .pipeline_jobs_total
        .fetch_add(1, Ordering::Relaxed);
    publish_json(
        state,
        &state.config.pipeline_job_subject,
        &json!({
            "schemaVersion": "public_data.pipeline.job.v1",
            "source": SERVICE_NAME,
            "job": job
        }),
    )
    .await;
    publish_runtime_event(
        state,
        "public_data.pipeline.job_queued",
        json!({ "jobId": job.job_id, "jobType": job.job_type }),
    )
    .await;
    Ok(job)
}
