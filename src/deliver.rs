//! NATS→HTTP fan-out: one subscription task per configured route; each message
//! is POSTed to the route's webhook with the concrete subject in a header.
//! At-most-once (core NATS); failures are counted, logged, and dropped —
//! JetStream durable consumers are the upgrade path for guaranteed delivery.

use futures_util::StreamExt;

use crate::config::DeliveryRoute;
use crate::metrics::Metrics;
use crate::publisher::Publisher;

pub fn spawn_delivery_loops(
    publisher: Publisher,
    routes: Vec<DeliveryRoute>,
    http: reqwest::Client,
    metrics: Metrics,
) {
    for route in routes {
        let publisher = publisher.clone();
        let http = http.clone();
        let metrics = metrics.clone();
        tokio::spawn(async move {
            loop {
                let Some(client) = publisher.client().await else {
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    continue;
                };
                let mut sub = match client.subscribe(route.subject.clone()).await {
                    Ok(sub) => sub,
                    Err(error) => {
                        tracing::warn!(subject = %route.subject, %error, "subscribe failed; retrying");
                        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                        continue;
                    }
                };
                tracing::info!(subject = %route.subject, webhook = %route.webhook, "delivery route active");
                while let Some(message) = sub.next().await {
                    let outcome =
                        forward(&http, &route.webhook, &message.subject, &message.payload).await;
                    let label = if outcome.is_ok() { "ok" } else { "failed" };
                    metrics
                        .deliveries
                        .with_label_values(&[route.subject.as_str(), label])
                        .inc();
                    if let Err(error) = outcome {
                        tracing::warn!(subject = %message.subject, webhook = %route.webhook, %error, "delivery failed");
                    }
                }
                tracing::warn!(subject = %route.subject, "subscription ended; resubscribing");
            }
        });
    }
}

/// POST one message to a webhook; 2xx is success.
pub async fn forward(
    http: &reqwest::Client,
    webhook: &str,
    subject: &str,
    payload: &[u8],
) -> anyhow::Result<()> {
    let response = http
        .post(webhook)
        .header("content-type", "application/json")
        .header("x-bridge-subject", subject)
        .body(payload.to_vec())
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await?;
    anyhow::ensure!(
        response.status().is_success(),
        "webhook returned {}",
        response.status()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{routing::post, Router};

    #[tokio::test]
    async fn forward_posts_payload_with_subject_header() {
        // Local webhook that asserts on what arrives.
        let app = Router::new().route(
            "/hook",
            post(
                |headers: axum::http::HeaderMap, body: axum::body::Bytes| async move {
                    assert_eq!(
                        headers.get("x-bridge-subject").unwrap(),
                        "shared-auth.events.identity"
                    );
                    assert_eq!(&body[..], br#"{"k":1}"#);
                    "ok"
                },
            ),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let http = reqwest::Client::new();
        forward(
            &http,
            &format!("http://{addr}/hook"),
            "shared-auth.events.identity",
            br#"{"k":1}"#,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn forward_reports_non_2xx_as_error() {
        let app = Router::new().route(
            "/hook",
            post(|| async { (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "boom") }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let http = reqwest::Client::new();
        assert!(forward(&http, &format!("http://{addr}/hook"), "s", b"{}")
            .await
            .is_err());
    }
}
