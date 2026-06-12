import dd_cli_config_client
import dd_otel_client
import dd_runtime_config_client
import gleam/erlang/process
import gleam/int
import gleam/io
import gleam/otp/static_supervisor as supervisor
import gleam_lambda_runner/http_server
import gleam_lambda_runner/nats
import gleam_lambda_runner/workflow

pub fn main() -> Nil {
  let _ = dd_cli_config_client.load_once()
  // Start the OpenTelemetry SDK + OTLP exporter before the HTTP supervisor.
  let _ = dd_otel_client.init("dd-gleam-lambda-runner")
  let _ = nats.start()
  let _ = workflow.start()
  let _ = dd_runtime_config_client.start_registration_loop()

  let assert Ok(_started) =
    supervisor.new(supervisor.OneForOne)
    |> supervisor.add(http_server.supervised())
    |> supervisor.start

  io.println(
    "dd gleam-lambda-runner listening on "
    <> http_server.bind_host()
    <> ":"
    <> int.to_string(http_server.bind_port()),
  )
  process.sleep_forever()
}
