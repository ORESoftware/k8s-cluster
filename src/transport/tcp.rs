//! Newline-delimited JSON stream for non-browser realtime consumers.

use std::{io, net::SocketAddr};

use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader},
    net::TcpListener,
    sync::broadcast,
};

use crate::realtime::{EventEnvelope, EventHub, ServiceSurface};

const MAX_COMMAND_BYTES: usize = 8 * 1024;

pub(crate) async fn serve(
    listener: TcpListener,
    hub: EventHub,
    surface: ServiceSurface,
) -> io::Result<()> {
    loop {
        let (stream, peer) = listener.accept().await?;
        let connection_hub = hub.clone();
        tokio::spawn(async move {
            if let Err(error) = serve_connection(stream, connection_hub, surface).await {
                tracing::debug!(
                    network.transport = "tcp",
                    network.peer.address = %peer,
                    error = %error,
                    "realtime TCP client disconnected"
                );
            }
        });
    }
}

#[tracing::instrument(
    name = "network.tcp.realtime",
    skip_all,
    fields(otel.kind = "server", network.transport = "tcp", service.name = surface.service_name())
)]
async fn serve_connection<S>(stream: S, hub: EventHub, surface: ServiceSurface) -> io::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (reader, mut writer) = tokio::io::split(stream);
    let mut commands = BufReader::new(reader).lines();
    let mut events = hub.subscribe();

    write_event(&mut writer, &hub.latest()).await?;

    loop {
        tokio::select! {
            event = events.recv() => match event {
                Ok(event) => write_event(&mut writer, &event).await?,
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    let event = EventEnvelope::new(
                        surface.service_name(),
                        "transport.lagged",
                        serde_json::json!({"skipped": skipped, "transport": "tcp"}),
                    );
                    write_event(&mut writer, &event).await?;
                }
                Err(broadcast::error::RecvError::Closed) => return Ok(()),
            },
            command = commands.next_line() => match command? {
                Some(command) if command.len() > MAX_COMMAND_BYTES => return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "realtime TCP command exceeds 8 KiB",
                )),
                Some(command) if is_refresh_command(&command) => {
                    write_event(&mut writer, &hub.latest()).await?;
                }
                Some(_) => {
                    let error = EventEnvelope::new(
                        surface.service_name(),
                        "transport.command-rejected",
                        serde_json::json!({"acceptedActions": ["refresh", "ping"]}),
                    );
                    write_event(&mut writer, &error).await?;
                }
                None => return Ok(()),
            }
        }
    }
}

async fn write_event<W>(writer: &mut W, event: &EventEnvelope) -> io::Result<()>
where
    W: AsyncWrite + Unpin,
{
    let mut encoded = serde_json::to_vec(event)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    encoded.push(b'\n');
    writer.write_all(&encoded).await
}

fn is_refresh_command(command: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(command)
        .ok()
        .and_then(|value| {
            value
                .get("action")
                .and_then(|action| action.as_str())
                .map(str::to_owned)
        })
        .is_some_and(|action| action == "refresh" || action == "ping")
}

pub(crate) async fn bind(address: SocketAddr) -> io::Result<TcpListener> {
    TcpListener::bind(address).await
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn tcp_commands_are_a_small_read_only_protocol() {
        assert!(is_refresh_command(r#"{"action":"refresh"}"#));
        assert!(is_refresh_command(r#"{"action":"ping"}"#));
        assert!(!is_refresh_command(r#"{"action":"publish"}"#));
        assert!(!is_refresh_command("refresh"));
    }

    #[tokio::test]
    async fn tcp_clients_receive_initial_and_fanout_envelopes() {
        let hub = EventHub::new(ServiceSurface::Fabrication, 8);
        let (client, server_stream) = tokio::io::duplex(16 * 1024);
        let server = tokio::spawn(serve_connection(
            server_stream,
            hub.clone(),
            ServiceSurface::Fabrication,
        ));
        let mut lines = BufReader::new(client).lines();

        let initial = lines
            .next_line()
            .await
            .expect("read initial")
            .expect("line");
        let initial: EventEnvelope = serde_json::from_str(&initial).expect("initial envelope");
        assert_eq!(initial.kind, "transport.connected");

        hub.publish_payload("test", "printer.layer-completed", json!({"layer": 12}));
        let update = lines.next_line().await.expect("read update").expect("line");
        let update: EventEnvelope = serde_json::from_str(&update).expect("update envelope");
        assert_eq!(update.kind, "printer.layer-completed");
        assert_eq!(update.payload["layer"], 12);

        server.abort();
    }
}
