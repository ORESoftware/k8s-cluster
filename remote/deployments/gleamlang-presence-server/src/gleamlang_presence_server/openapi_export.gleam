//// Deterministic native contract exporter used by CI and fleet tooling.
////
//// This module deliberately does not start telemetry, Postgres, NATS, the
//// BEAM cluster, or the HTTP listener. It renders the complete internal
//// contract directly from the executable route registry.

import gleam/io
import gleamlang_presence_server/route_contract

pub fn main() -> Nil {
  route_contract.internal_openapi_json()
  |> io.println
}
