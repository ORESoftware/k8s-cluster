import gleam/erlang/process
import gleam/io
import gleam/otp/static_supervisor as supervisor
import gleam_lambda_runner/http_server

pub fn main() -> Nil {
  let assert Ok(_started) =
    supervisor.new(supervisor.OneForOne)
    |> supervisor.add(http_server.supervised())
    |> supervisor.start

  io.println("dd gleam-lambda-runner listening on :8083")
  process.sleep_forever()
}
