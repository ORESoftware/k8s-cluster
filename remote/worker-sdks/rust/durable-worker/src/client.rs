use crate::error::{DurableWorkerError, ProtocolError};
use crate::transport::{ReqwestTransport, Transport, TransportRequest, TransportResponse};
use reqwest::header::{HeaderName, HeaderValue};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

pub type JsonObject = Map<String, Value>;

const DEFAULT_AUTH_HEADER: &str = "X-Worker-Auth";
const DEFAULT_USER_AGENT: &str = "oresoftware-durable-worker-rust/0.1.0";
const DEFAULT_MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
const TRANSIENT_STATUSES: &[u16] = &[408, 425, 429, 500, 502, 503, 504];

#[derive(Clone)]
pub struct ClientOptions {
    pub auth_header: String,
    pub timeout: Duration,
    pub max_retries: usize,
    pub initial_backoff: Duration,
    pub max_backoff: Duration,
    pub max_response_bytes: usize,
    pub transport: Option<Arc<dyn Transport>>,
}

impl Default for ClientOptions {
    fn default() -> Self {
        Self {
            auth_header: DEFAULT_AUTH_HEADER.to_owned(),
            timeout: Duration::from_secs(30),
            max_retries: 3,
            initial_backoff: Duration::from_millis(200),
            max_backoff: Duration::from_secs(5),
            max_response_bytes: DEFAULT_MAX_RESPONSE_BYTES,
            transport: None,
        }
    }
}

#[derive(Clone)]
pub struct Client {
    base_url: Url,
    auth_secret: String,
    auth_header: String,
    timeout: Duration,
    max_retries: usize,
    initial_backoff: Duration,
    max_backoff: Duration,
    max_response_bytes: usize,
    transport: Arc<dyn Transport>,
}

impl Client {
    pub fn new(
        base_url: &str,
        auth_secret: impl Into<String>,
        options: ClientOptions,
    ) -> Result<Self, DurableWorkerError> {
        let mut parsed = Url::parse(base_url)
            .map_err(|error| DurableWorkerError::Configuration(error.to_string()))?;
        if parsed.scheme() != "http" && parsed.scheme() != "https" {
            return Err(DurableWorkerError::Configuration(
                "base URL must use http or https".to_owned(),
            ));
        }
        if parsed.host_str().is_none()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
        {
            return Err(DurableWorkerError::Configuration(
                "base URL must be absolute and must not contain credentials, query, or fragment"
                    .to_owned(),
            ));
        }
        let path = parsed.path().trim_end_matches('/').to_owned();
        parsed.set_path(&path);

        let auth_secret = auth_secret.into();
        if auth_secret.trim().is_empty() || auth_secret.contains('\r') || auth_secret.contains('\n')
        {
            return Err(DurableWorkerError::Configuration(
                "auth secret must be a non-empty single-line value".to_owned(),
            ));
        }
        HeaderName::from_bytes(options.auth_header.as_bytes()).map_err(|_| {
            DurableWorkerError::Configuration("auth header must be a valid HTTP token".to_owned())
        })?;
        HeaderValue::from_str(&auth_secret).map_err(|_| {
            DurableWorkerError::Configuration(
                "auth secret contains characters invalid in an HTTP header".to_owned(),
            )
        })?;
        if options.timeout.is_zero() {
            return Err(DurableWorkerError::Configuration(
                "timeout must be positive".to_owned(),
            ));
        }
        if options.max_backoff < options.initial_backoff {
            return Err(DurableWorkerError::Configuration(
                "max backoff must be greater than or equal to initial backoff".to_owned(),
            ));
        }
        if options.max_response_bytes == 0 {
            return Err(DurableWorkerError::Configuration(
                "max response bytes must be positive".to_owned(),
            ));
        }

        let transport = match options.transport {
            Some(transport) => transport,
            None => Arc::new(ReqwestTransport::new().map_err(DurableWorkerError::Transport)?),
        };

        Ok(Self {
            base_url: parsed,
            auth_secret,
            auth_header: options.auth_header,
            timeout: options.timeout,
            max_retries: options.max_retries,
            initial_backoff: options.initial_backoff,
            max_backoff: options.max_backoff,
            max_response_bytes: options.max_response_bytes,
            transport,
        })
    }

    async fn request(
        &self,
        method: &str,
        url: Url,
        payload: Option<Value>,
        idempotent: bool,
        lease_sensitive: bool,
    ) -> Result<JsonObject, DurableWorkerError> {
        let body = payload
            .map(|payload| serde_json::to_vec(&payload))
            .transpose()
            .map_err(|error| DurableWorkerError::Serialization(error.to_string()))?;
        let attempts = 1 + if idempotent { self.max_retries } else { 0 };

        for attempt in 0..attempts {
            let mut headers = BTreeMap::from([
                (self.auth_header.clone(), self.auth_secret.clone()),
                ("accept".to_owned(), "application/json".to_owned()),
                ("user-agent".to_owned(), DEFAULT_USER_AGENT.to_owned()),
            ]);
            if body.is_some() {
                headers.insert("content-type".to_owned(), "application/json".to_owned());
            }
            let response = self
                .transport
                .execute(TransportRequest {
                    method: method.to_owned(),
                    url: url.to_string(),
                    headers,
                    body: body.clone(),
                    timeout: self.timeout,
                    max_response_bytes: self.max_response_bytes,
                })
                .await;

            let response = match response {
                Ok(response) => response,
                Err(error) => {
                    if idempotent && error.retryable && attempt + 1 < attempts {
                        tokio::time::sleep(self.backoff(attempt, None)).await;
                        continue;
                    }
                    return Err(DurableWorkerError::Transport(error));
                }
            };

            let decoded = decode_response(&response, (200..300).contains(&response.status))?;
            if (200..300).contains(&response.status) {
                return Ok(decoded);
            }
            let error = protocol_error(response.status, &decoded, lease_sensitive);
            if idempotent
                && attempt + 1 < attempts
                && TRANSIENT_STATUSES.contains(&response.status)
                && error.retryable()
            {
                tokio::time::sleep(self.backoff(attempt, response.headers.get("retry-after")))
                    .await;
                continue;
            }
            return Err(error);
        }
        Err(DurableWorkerError::Configuration(
            "retry loop exhausted unexpectedly".to_owned(),
        ))
    }

    fn backoff(&self, attempt: usize, retry_after: Option<&String>) -> Duration {
        if let Some(retry_after) = retry_after {
            if let Ok(seconds) = retry_after.trim().parse::<f64>() {
                if seconds.is_finite() && seconds >= 0.0 {
                    return Duration::from_secs_f64(seconds.min(self.max_backoff.as_secs_f64()));
                }
            }
        }
        let multiplier = 1u32.checked_shl(attempt.min(31) as u32).unwrap_or(u32::MAX);
        self.initial_backoff
            .checked_mul(multiplier)
            .unwrap_or(self.max_backoff)
            .min(self.max_backoff)
    }

    fn endpoint(&self, segments: &[&str]) -> Result<Url, DurableWorkerError> {
        let mut url = self.base_url.clone();
        {
            let mut path = url.path_segments_mut().map_err(|_| {
                DurableWorkerError::Configuration(
                    "base URL cannot be used as a hierarchical endpoint".to_owned(),
                )
            })?;
            path.pop_if_empty();
            for segment in segments {
                if segment.is_empty() {
                    return Err(DurableWorkerError::Configuration(
                        "path identifiers must be non-empty".to_owned(),
                    ));
                }
                path.push(segment);
            }
        }
        Ok(url)
    }

    pub async fn submit_task(&self, task: JsonObject) -> Result<JsonObject, DurableWorkerError> {
        let idempotent = non_empty_string(&task, "idempotencyKey");
        self.request(
            "POST",
            self.endpoint(&["api", "v1", "tasks"])?,
            Some(Value::Object(task)),
            idempotent,
            false,
        )
        .await
    }

    pub async fn submit_run(&self, run: JsonObject) -> Result<JsonObject, DurableWorkerError> {
        let idempotent = non_empty_string(&run, "idempotencyKey");
        self.request(
            "POST",
            self.endpoint(&["api", "v1", "runs"])?,
            Some(Value::Object(run)),
            idempotent,
            false,
        )
        .await
    }

    pub async fn get_run(&self, run_id: &str) -> Result<JsonObject, DurableWorkerError> {
        self.request(
            "GET",
            self.endpoint(&["api", "v1", "runs", run_id])?,
            None,
            true,
            false,
        )
        .await
    }

    pub async fn signal_run(
        &self,
        run_id: &str,
        signal_name: &str,
        payload: JsonObject,
    ) -> Result<JsonObject, DurableWorkerError> {
        self.request(
            "POST",
            self.endpoint(&["api", "v1", "runs", run_id, "signals", signal_name])?,
            Some(serde_json::json!({ "payload": payload })),
            false,
            false,
        )
        .await
    }

    pub async fn pause_run(&self, run_id: &str) -> Result<JsonObject, DurableWorkerError> {
        self.run_mutation(run_id, "pause").await
    }

    pub async fn resume_run(&self, run_id: &str) -> Result<JsonObject, DurableWorkerError> {
        self.run_mutation(run_id, "resume").await
    }

    pub async fn cancel_run(&self, run_id: &str) -> Result<JsonObject, DurableWorkerError> {
        self.run_mutation(run_id, "cancel").await
    }

    async fn run_mutation(
        &self,
        run_id: &str,
        operation: &str,
    ) -> Result<JsonObject, DurableWorkerError> {
        self.request(
            "POST",
            self.endpoint(&["api", "v1", "runs", run_id, operation])?,
            Some(Value::Object(JsonObject::new())),
            true,
            false,
        )
        .await
    }

    pub async fn register_worker(
        &self,
        registration: WorkerRegistration,
    ) -> Result<JsonObject, DurableWorkerError> {
        self.request(
            "POST",
            self.endpoint(&["api", "v1", "workers", "register"])?,
            Some(to_value(registration)?),
            true,
            false,
        )
        .await
    }

    pub async fn heartbeat_worker(
        &self,
        worker_id: &str,
        drain: Option<bool>,
    ) -> Result<JsonObject, DurableWorkerError> {
        self.request(
            "POST",
            self.endpoint(&["api", "v1", "workers", worker_id, "heartbeat"])?,
            Some(to_value(WorkerHeartbeat { drain })?),
            true,
            false,
        )
        .await
    }

    pub async fn poll_worker(
        &self,
        worker_id: &str,
        wait_ms: u64,
    ) -> Result<WorkerPoll, DurableWorkerError> {
        let mut url = self.endpoint(&["api", "v1", "workers", worker_id, "poll"])?;
        url.query_pairs_mut()
            .append_pair("waitMs", &wait_ms.to_string());
        let response = self
            .request(
                "POST",
                url,
                Some(Value::Object(JsonObject::new())),
                false,
                false,
            )
            .await?;
        from_object(response)
    }

    pub async fn start_step(
        &self,
        step_id: &str,
        lease: Lease,
    ) -> Result<JsonObject, DurableWorkerError> {
        self.lease_mutation(step_id, "start", lease).await
    }

    pub async fn heartbeat_step(
        &self,
        step_id: &str,
        lease: Lease,
    ) -> Result<JsonObject, DurableWorkerError> {
        self.lease_mutation(step_id, "heartbeat", lease).await
    }

    async fn lease_mutation(
        &self,
        step_id: &str,
        operation: &str,
        lease: Lease,
    ) -> Result<JsonObject, DurableWorkerError> {
        self.request(
            "POST",
            self.endpoint(&["api", "v1", "steps", step_id, operation])?,
            Some(to_value(lease)?),
            true,
            true,
        )
        .await
    }

    pub async fn append_step_output(
        &self,
        step_id: &str,
        request: StepOutput,
    ) -> Result<JsonObject, DurableWorkerError> {
        self.request(
            "POST",
            self.endpoint(&["api", "v1", "steps", step_id, "output"])?,
            Some(to_value(request)?),
            true,
            true,
        )
        .await
    }

    pub async fn complete_step(
        &self,
        step_id: &str,
        request: StepCompletion,
    ) -> Result<JsonObject, DurableWorkerError> {
        self.request(
            "POST",
            self.endpoint(&["api", "v1", "steps", step_id, "complete"])?,
            Some(to_value(request)?),
            true,
            true,
        )
        .await
    }

    pub async fn fail_step(
        &self,
        step_id: &str,
        request: StepFailure,
    ) -> Result<JsonObject, DurableWorkerError> {
        self.request(
            "POST",
            self.endpoint(&["api", "v1", "steps", step_id, "fail"])?,
            Some(to_value(request)?),
            true,
            true,
        )
        .await
    }
}

fn non_empty_string(object: &JsonObject, key: &str) -> bool {
    object
        .get(key)
        .and_then(Value::as_str)
        .is_some_and(|value| !value.is_empty())
}

fn to_value<T: Serialize>(value: T) -> Result<Value, DurableWorkerError> {
    serde_json::to_value(value)
        .map_err(|error| DurableWorkerError::Serialization(error.to_string()))
}

fn from_object<T: for<'de> Deserialize<'de>>(object: JsonObject) -> Result<T, DurableWorkerError> {
    serde_json::from_value(Value::Object(object))
        .map_err(|error| DurableWorkerError::Serialization(error.to_string()))
}

fn decode_response(
    response: &TransportResponse,
    strict_json: bool,
) -> Result<JsonObject, DurableWorkerError> {
    if response.body.is_empty() {
        return Ok(JsonObject::new());
    }
    match serde_json::from_slice::<Value>(&response.body) {
        Ok(Value::Object(object)) => Ok(object),
        Ok(_) | Err(_) if !strict_json => Ok(JsonObject::new()),
        Ok(_) => Err(DurableWorkerError::Protocol(ProtocolError::new(
            "invalid_response",
            "durable-worker returned a non-object JSON response",
            Some(response.status),
            false,
        ))),
        Err(error) => Err(DurableWorkerError::Protocol(ProtocolError::new(
            "invalid_response",
            format!("durable-worker returned invalid JSON: {error}"),
            Some(response.status),
            false,
        ))),
    }
}

fn protocol_error(status: u16, body: &JsonObject, lease_sensitive: bool) -> DurableWorkerError {
    let message = body
        .get("message")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| format!("durable-worker returned HTTP {status}"));
    let code = body
        .get("code")
        .and_then(Value::as_str)
        .unwrap_or("http_error")
        .to_owned();
    let retryable = body
        .get("retryable")
        .and_then(Value::as_bool)
        .unwrap_or_else(|| TRANSIENT_STATUSES.contains(&status));
    let error = ProtocolError::new(code, message, Some(status), retryable);
    if lease_sensitive && matches!(status, 404 | 409) {
        DurableWorkerError::LeaseLost(error)
    } else {
        DurableWorkerError::Protocol(error)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Lease {
    pub worker_id: String,
    pub lease_token: String,
    pub lease_generation: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Assignment {
    pub run_id: String,
    pub step_id: String,
    pub step_key: String,
    pub task_type: String,
    pub queue: String,
    #[serde(default)]
    pub input: JsonObject,
    pub attempt: u32,
    pub lease_token: String,
    pub lease_generation: i64,
    pub fencing_token: i64,
    pub lease_expires_at_ms: i64,
    pub timeout_ms: u64,
    #[serde(default)]
    pub affinity_key: Option<String>,
}

impl Assignment {
    pub fn lease(&self, worker_id: impl Into<String>) -> Lease {
        Lease {
            worker_id: worker_id.into(),
            lease_token: self.lease_token.clone(),
            lease_generation: self.lease_generation,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerRegistration {
    pub worker_id: String,
    pub queues: Vec<String>,
    pub capabilities: Vec<String>,
    pub labels: JsonObject,
    pub slots: usize,
    pub ttl_ms: u64,
    pub drain: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkerHeartbeat {
    #[serde(skip_serializing_if = "Option::is_none")]
    drain: Option<bool>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WorkerPoll {
    #[serde(default)]
    pub assignment: Option<Assignment>,
    #[serde(default)]
    pub retry_after_ms: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StepOutput {
    #[serde(flatten)]
    pub lease: Lease,
    pub chunk_id: String,
    pub chunk: String,
    pub stream: String,
    pub final_chunk: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StepCompletion {
    #[serde(flatten)]
    pub lease: Lease,
    pub result: JsonObject,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StepFailure {
    #[serde(flatten)]
    pub lease: Lease,
    pub code: String,
    pub message: String,
    pub retryable: bool,
}
