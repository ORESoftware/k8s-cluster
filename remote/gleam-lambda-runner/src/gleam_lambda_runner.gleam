import gleam/erlang/process
import gleam/int
import gleam/io
import gleam/otp/static_supervisor as supervisor
import gleam_lambda_runner/http_server
import gleam_lambda_runner/nats

pub fn main() -> Nil {
  let _ = nats.start()

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
