import gleam/erlang/process
import gleam/io
import gleam/otp/static_supervisor as supervisor
import gleam/otp/supervision
import gleam_mcp_server/http_server
import gleam_mcp_server/metrics

pub fn main() -> Nil {
  let metrics_name = process.new_name(prefix: "dd_gleam_mcp_metrics")

  let assert Ok(_started) =
    supervisor.new(supervisor.OneForOne)
    |> supervisor.add(
      supervision.worker(fn() { metrics.start(named_as: metrics_name) }),
    )
    |> supervisor.add(http_server.supervised(metrics_name))
    |> supervisor.start

  io.println("dd gleam MCP server listening on :8090")
  process.sleep_forever()
}
