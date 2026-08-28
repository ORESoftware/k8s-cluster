use oresoftware_durable_worker::{
    Cancellation, Client, ClientOptions, Handler, JsonObject, TaskContext, Worker, WorkerConfig,
};
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let base_url =
        std::env::var("DURABLE_WORKER_URL").unwrap_or_else(|_| "http://127.0.0.1:8152".to_owned());
    let secret = std::env::var("DURABLE_WORKER_AUTH_SECRET")?;
    let client = Arc::new(Client::new(&base_url, secret, ClientOptions::default())?);

    let echo: Handler = Arc::new(|context: TaskContext| {
        Box::pin(async move {
            context.emit("accepted", "progress", false).await?;
            context.check_cancelled()?;
            let mut result = JsonObject::new();
            result.insert("echo".to_owned(), json!(context.input()));
            result.insert("fencingToken".to_owned(), json!(context.fencing_token()));
            Ok(result)
        })
    });
    let handlers = HashMap::from([("example.echo".to_owned(), echo)]);
    let worker = Worker::new(
        client,
        handlers,
        WorkerConfig {
            worker_id: std::env::var("WORKER_ID")
                .unwrap_or_else(|_| "rust-example-worker".to_owned()),
            queues: vec!["examples".to_owned()],
            capabilities: vec!["rust".to_owned()],
            slots: 4,
            ..WorkerConfig::default()
        },
    )?;

    let summary = worker.run(Cancellation::default()).await?;
    println!(
        "accepted={} completed={} failed={} lease_lost={}",
        summary.accepted, summary.completed, summary.failed, summary.lease_lost
    );
    Ok(())
}
