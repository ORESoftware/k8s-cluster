//! Isolated HTTP fixture for Chromium/OpenAPI contract tests.

use std::net::SocketAddr;
use t2v_api::testkit::build_test_state;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let address = std::env::var("T2V_OPENAPI_FIXTURE_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:18130".to_string())
        .parse::<SocketAddr>()?;
    let app = build_test_state()
        .await
        .with_server_auth("browser-server-secret")
        .with_vapi_secret("browser-vapi-secret")
        .app();
    let listener = tokio::net::TcpListener::bind(address).await?;
    println!("t2v OpenAPI fixture listening on http://{address}");
    axum::serve(listener, app).await?;
    Ok(())
}
