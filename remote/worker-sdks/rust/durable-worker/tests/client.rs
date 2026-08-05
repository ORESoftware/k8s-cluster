use oresoftware_durable_worker::{
    Client, ClientOptions, DurableWorkerError, JsonObject, Transport, TransportError,
    TransportFuture, TransportRequest, TransportResponse,
};
use serde_json::json;
use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Clone, Debug)]
enum ScriptedResponse {
    Response(TransportResponse),
    Error(TransportError),
}

#[derive(Clone, Default)]
struct ScriptedTransport {
    responses: Arc<Mutex<VecDeque<ScriptedResponse>>>,
    requests: Arc<Mutex<Vec<TransportRequest>>>,
}

impl ScriptedTransport {
    fn new(responses: Vec<ScriptedResponse>) -> Self {
        Self {
            responses: Arc::new(Mutex::new(responses.into())),
            requests: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn request_count(&self) -> usize {
        self.requests.lock().expect("requests lock").len()
    }
}

impl Transport for ScriptedTransport {
    fn execute(&self, request: TransportRequest) -> TransportFuture<'_> {
        Box::pin(async move {
            self.requests
                .lock()
                .expect("requests lock")
                .push(request);
            match self
                .responses
                .lock()
                .expect("responses lock")
                .pop_front()
                .expect("scripted response")
            {
                ScriptedResponse::Response(response) => Ok(response),
                ScriptedResponse::Error(error) => Err(error),
            }
        })
    }
}

fn response(status: u16, payload: serde_json::Value) -> TransportResponse {
    TransportResponse {
        status,
        headers: BTreeMap::new(),
        body: serde_json::to_vec(&payload).expect("encode response"),
    }
}

fn client(transport: ScriptedTransport) -> Client {
    Client::new(
        "https://workers.example.test",
        "test-secret",
        ClientOptions {
            max_retries: 2,
            initial_backoff: Duration::ZERO,
            max_backoff: Duration::ZERO,
            transport: Some(Arc::new(transport)),
            ..ClientOptions::default()
        },
    )
    .expect("client")
}

#[tokio::test]
async fn bound_submission_retries_but_unbound_submission_does_not() {
    let transient = response(
        503,
        json!({"code":"busy","message":"busy","retryable":true}),
    );
    let accepted = response(202, json!({"runId":"run-1","status":"pending"}));
    let transport = ScriptedTransport::new(vec![
        ScriptedResponse::Response(transient.clone()),
        ScriptedResponse::Response(accepted),
    ]);
    let sdk = client(transport.clone());
    let mut task = JsonObject::new();
    task.insert("idempotencyKey".to_owned(), json!("stable"));
    task.insert("taskType".to_owned(), json!("demo"));
    sdk.submit_task(task).await.expect("bound submission");
    assert_eq!(transport.request_count(), 2);

    let transport = ScriptedTransport::new(vec![ScriptedResponse::Response(transient)]);
    let sdk = client(transport.clone());
    let mut task = JsonObject::new();
    task.insert("taskType".to_owned(), json!("demo"));
    assert!(sdk.submit_task(task).await.is_err());
    assert_eq!(transport.request_count(), 1);
}

#[tokio::test]
async fn ambiguous_worker_poll_is_never_retried() {
    let transport = ScriptedTransport::new(vec![ScriptedResponse::Error(
        TransportError::new("connection reset", true),
    )]);
    let sdk = client(transport.clone());
    let error = sdk.poll_worker("worker-1", 30_000).await.unwrap_err();
    assert!(error.retryable());
    assert_eq!(transport.request_count(), 1);
}

#[tokio::test]
async fn fenced_step_mutation_is_reported_as_lease_loss() {
    let transport = ScriptedTransport::new(vec![ScriptedResponse::Response(response(
        409,
        json!({
            "code":"state_conflict",
            "message":"stale lease generation",
            "retryable":false
        }),
    ))]);
    let sdk = client(transport);
    let error = sdk
        .complete_step(
            "step-1",
            oresoftware_durable_worker::StepCompletion {
                lease: oresoftware_durable_worker::Lease {
                    worker_id: "worker-1".to_owned(),
                    lease_token: "lease-1".to_owned(),
                    lease_generation: 3,
                },
                result: JsonObject::new(),
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(error, DurableWorkerError::LeaseLost(_)));
}

#[tokio::test]
async fn redirect_status_is_not_treated_as_success() {
    let transport = ScriptedTransport::new(vec![ScriptedResponse::Response(
        TransportResponse {
            status: 302,
            headers: BTreeMap::from([(
                "location".to_owned(),
                "https://untrusted.example.test".to_owned(),
            )]),
            body: serde_json::to_vec(&json!({"message":"redirect refused"}))
                .expect("encode response"),
        },
    )]);
    let sdk = client(transport);
    let error = sdk.get_run("run-1").await.unwrap_err();
    assert_eq!(error.status(), Some(302));
}

#[test]
fn rejects_credentials_in_base_url_and_multiline_secrets() {
    assert!(Client::new(
        "https://user:password@workers.example.test",
        "secret",
        ClientOptions::default()
    )
    .is_err());
    assert!(Client::new(
        "https://workers.example.test",
        "secret\nsecond-line",
        ClientOptions::default()
    )
    .is_err());
}
