//! Runtime composition for the separately deployable browser-facing service.

use std::{error::Error, time::Duration};

use axum::{extract::DefaultBodyLimit, response::Redirect, routing::get, Router};

use crate::{
    messaging, observability,
    persistence::Persistence,
    realtime::{EventHub, ServiceSurface},
    secrets::SecretOverlay,
    transport,
};

mod backend;
mod config;
mod http;
mod supabase;

const MAX_HTTP_BODY_BYTES: usize = 512 * 1024;

#[derive(Clone)]
pub(super) struct WebState {
    pub(super) persistence: Persistence,
    pub(super) realtime: EventHub,
    pub(super) nats_enabled: bool,
    pub(super) supabase_enabled: bool,
}

pub(crate) async fn run() -> Result<(), Box<dyn Error + Send + Sync>> {
    let _otel = observability::init_for(ServiceSurface::Web.service_name());
    let config = config::WebConfig::from_env()?;
    let persistence = Persistence::from_web_env().await?;
    let persistence_enabled = persistence.is_enabled();
    let secrets = SecretOverlay::load()
        .await
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error))?;
    let nats = messaging::connect_optional(&secrets, ServiceSurface::Web.service_name()).await;
    let nats_enabled = nats.is_some();
    let supabase_enabled = config.supabase.is_some();
    let hub = EventHub::new(ServiceSurface::Web, config.event_buffer);

    backend::spawn(config.backend_ws_url.clone(), hub.clone());
    supabase::spawn(config.supabase.clone(), hub.clone());
    transport::spawn_relay(
        nats.clone(),
        config.nats_result_subject.clone(),
        hub.clone(),
        ServiceSurface::Web,
    );
    transport::spawn_publisher(nats, config.nats_event_subject.clone(), hub.clone());

    let tcp_address = config.tcp_address()?;
    let tcp_listener = transport::bind_tcp(tcp_address).await?;
    let tcp_hub = hub.clone();
    tokio::spawn(async move {
        if let Err(error) = transport::serve_tcp(tcp_listener, tcp_hub, ServiceSurface::Web).await {
            tracing::error!(
                network.transport = "tcp",
                server.address = %tcp_address,
                error = %error,
                "fabrication web TCP server stopped"
            );
        }
    });

    let state = WebState {
        persistence,
        realtime: hub.clone(),
        nats_enabled,
        supabase_enabled,
    };
    let app = app(state, hub).merge(dd_runtime_config_client::router());
    tokio::spawn(dd_runtime_config_client::register_with_control_plane());

    let http_address = config.http_address()?;
    observability::web_server_listening(
        http_address,
        tcp_address,
        persistence_enabled,
        nats_enabled,
        supabase_enabled,
    );
    let listener = tokio::net::TcpListener::bind(http_address).await?;
    axum::serve(listener, app.layer(dd_telemetry::http_trace_layer()))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    tokio::time::sleep(Duration::from_millis(10)).await;
    Ok(())
}

fn app(state: WebState, hub: EventHub) -> Router {
    Router::new()
        .route("/", get(|| async { Redirect::temporary("/mash") }))
        .route("/healthz", get(http::healthz))
        .route("/readyz", get(http::readyz))
        .route("/metrics", get(http::metrics))
        .merge(transport::router(hub, ServiceSurface::Web))
        .layer(DefaultBodyLimit::max(MAX_HTTP_BODY_BYTES))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_runtime_composes_a_separate_router_from_shared_transports() {
        let hub = EventHub::new(ServiceSurface::Web, 8);
        let state = WebState {
            persistence: Persistence::Disabled,
            realtime: hub.clone(),
            nats_enabled: false,
            supabase_enabled: false,
        };

        let _: Router = app(state, hub);
    }
}
