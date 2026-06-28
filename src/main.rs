use std::net::SocketAddr;

mod app;
mod data;
mod routes;
mod views;

use app::{env_string, init_tracing, shutdown_signal, AppState};

#[tokio::main]
async fn main() {
    init_tracing();
    let state = AppState::from_env();
    let backend_url = state.backend_url.clone();
    let app = routes::router(state);

    let host = env_string("HOST").unwrap_or_else(|| "127.0.0.1".to_string());
    let port: u16 = env_string("PORT")
        .and_then(|value| value.parse().ok())
        .unwrap_or(8124);
    let addr: SocketAddr = format!("{host}:{port}")
        .parse()
        .unwrap_or_else(|err| panic!("invalid bind address {host}:{port}: {err}"));
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .unwrap_or_else(|err| panic!("akrion-web-server bind {addr}: {err}"));

    tracing::info!(
        event = "akrion_web_server_listening",
        %addr,
        backend_url = %backend_url
    );
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .expect("akrion-web-server serve");
}
