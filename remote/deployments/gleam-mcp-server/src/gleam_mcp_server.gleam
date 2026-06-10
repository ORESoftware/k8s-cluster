import dd_cli_config_client
import dd_runtime_config_client
import gleam/erlang/process
import gleam/int
import gleam/io
import gleam/option.{type Option, None, Some}
import gleam/otp/static_supervisor as supervisor
import gleam/otp/supervision
import gleam_mcp_server/http_server
import gleam_mcp_server/metrics
import gleam_mcp_server/nats
import gleam_mcp_server/pg_contract

@external(erlang, "gleam_mcp_runtime_env", "getenv")
fn env_get(name: String) -> String

pub fn main() -> Nil {
  let _ = dd_cli_config_client.load_once()
  let metrics_name = process.new_name(prefix: "dd_gleam_mcp_metrics")
  let nats_name = process.new_name(prefix: "dd_gleam_mcp_nats")

  let _ = pg_contract.app_config_table()

  // NATS transport is optional — only wired when NATS_URL is set, so the
  // HTTP surface is unchanged in environments without messaging.
  let nats_url = env_get("NATS_URL")
  let nats_handle: Option(nats.Name) = case nats_url {
    "" -> None
    _ -> Some(nats_name)
  }

  let builder =
    supervisor.new(supervisor.OneForOne)
    |> supervisor.add(
      supervision.worker(fn() { metrics.start(named_as: metrics_name) }),
    )

  // Start NATS before the HTTP server so the named transport is registered
  // by the time the first tools/call wants to publish an audit event.
  let builder = case nats_handle {
    Some(_) ->
      supervisor.add(builder, nats.supervised(name: nats_name, url: nats_url))
    None -> builder
  }

  let assert Ok(_started) =
    builder
    |> supervisor.add(http_server.supervised(metrics_name, nats_handle))
    |> supervisor.start

  let _ = dd_runtime_config_client.start_registration_loop()

  io.println(
    "dd gleam MCP server listening on "
    <> http_server.bind_host()
    <> ":"
    <> int.to_string(http_server.bind_port())
    <> nats_status(nats_handle),
  )
  process.sleep_forever()
}

fn nats_status(handle: Option(nats.Name)) -> String {
  case handle {
    Some(_) -> " (NATS transport enabled)"
    None -> " (NATS transport disabled; set NATS_URL to enable)"
  }
}
