import gleam/int
import gleam/result
import gleam/string as gleam_string

pub type ValueSource {
  CliFlag
  EnvVar
  DefaultValue
  Missing
}

@external(erlang, "dd_cli_config_client_ffi", "load_once")
pub fn load_once() -> Nil

@external(erlang, "dd_cli_config_client_ffi", "reload")
pub fn reload() -> Nil

@external(erlang, "dd_cli_config_client_ffi", "env")
pub fn get(name: String) -> Result(String, Nil)

@external(erlang, "dd_cli_config_client_ffi", "source")
fn source_name(name: String) -> String

@external(erlang, "dd_cli_config_client_ffi", "snapshot_json")
pub fn snapshot_json() -> String

pub fn string(name: String, fallback: String) -> String {
  case get(name) {
    Ok(value) -> value
    Error(_) -> fallback
  }
}

pub fn integer(name: String, fallback: Int) -> Int {
  get(name)
  |> result.try(int.parse)
  |> result.unwrap(fallback)
}

pub fn positive_integer(name: String, fallback: Int) -> Int {
  let value = integer(name, fallback)
  case value > 0 {
    True -> value
    False -> fallback
  }
}

pub fn bool(name: String, fallback: Bool) -> Bool {
  case get(name) {
    Error(_) -> fallback
    Ok(raw) -> {
      case gleam_string.lowercase(raw) {
        "1" -> True
        "true" -> True
        "yes" -> True
        "on" -> True
        "0" -> False
        "false" -> False
        "no" -> False
        "off" -> False
        _ -> fallback
      }
    }
  }
}

pub fn source(name: String) -> ValueSource {
  case source_name(name) {
    "cli" -> CliFlag
    "env" -> EnvVar
    "default" -> DefaultValue
    _ -> Missing
  }
}
