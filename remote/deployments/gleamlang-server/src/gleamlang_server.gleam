import dd_runtime_config_client
import gleam/erlang/process
import gleam/io
import gleam/otp/static_supervisor as supervisor
import gleam/otp/supervision
import gleamlang_server/broadcaster
import gleamlang_server/http_server
import gleamlang_server/pg_contract

const tick_interval_ms = 2000

pub fn main() -> Nil {
  let broker_name = process.new_name(prefix: "dd_gleamlang_broker")

  let _ = pg_contract.app_config_table()

  let assert Ok(_started) =
    supervisor.new(supervisor.OneForOne)
    |> supervisor.add(
      supervision.worker(fn() {
        broadcaster.start(named_as: broker_name, interval_ms: tick_interval_ms)
      }),
    )
    |> supervisor.add(http_server.supervised(broker_name))
    |> supervisor.start

  let _ = dd_runtime_config_client.start_registration_loop()

  io.println("dd gleamlang-server listening on :8081")
  process.sleep_forever()
}
