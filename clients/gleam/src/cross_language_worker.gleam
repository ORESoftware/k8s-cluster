import dd_rust_network_mutex_client as nm
import gleam/dict
import gleam/int
import gleam/io
import gleam/json.{type Json}
import gleam/list
import gleam/option.{None, Some}
import gleam/result

@external(erlang, "dd_rust_network_mutex_client_ffi_helpers", "getenv")
fn ffi_getenv(name: String) -> Result(String, Nil)

@external(erlang, "dd_rust_network_mutex_client_ffi_helpers", "emit_event")
fn ffi_emit_event(line: String) -> Result(Nil, String)

@external(erlang, "dd_rust_network_mutex_client_ffi_helpers", "sleep_ms")
fn ffi_sleep_ms(ms: Int) -> Nil

type Rng {
  Rng(state: Int)
}

pub fn main() {
  let host = env_or("LIVE_MUTEX_HOST", "127.0.0.1")
  let port = parse_int(env_or("LIVE_MUTEX_PORT", "6970"), 6970)
  let lang = env_or("LMX_WORKER_LANG", "gleam")
  let worker = env_or("LMX_WORKER_ID", lang <> "-0")
  let seed = parse_int(env_or("LMX_WORKER_SEED", "1"), 1)
  let ops = parse_int(env_or("LMX_WORKER_OPS", "50"), 50)
  let key_prefix = env_or("LMX_FUZZ_KEY_PREFIX", "cross")
  let key_count = parse_int(env_or("LMX_FUZZ_KEY_COUNT", "5"), 5)
  let keys = make_keys(key_prefix, key_count)

  let assert Ok(client) = nm.connect(host, port, None)
  let assert Ok(_) = run_loop(client, Rng(seed), ops, keys, lang, worker)
  nm.close(client)
  io.println(done_event(lang, worker, ops))
}

fn run_loop(
  client: nm.Client,
  rng: Rng,
  ops: Int,
  keys: List(String),
  lang: String,
  worker: String,
) -> Result(Nil, String) {
  case ops <= 0 {
    True -> Ok(Nil)
    False -> {
      let #(roll, rng) = below(rng, 100)
      use _ <- result.try(run_op(client, rng, roll, keys, lang, worker))
      run_loop(client, rng, ops - 1, keys, lang, worker)
    }
  }
}

fn run_op(
  client: nm.Client,
  rng: Rng,
  roll: Int,
  keys: List(String),
  lang: String,
  worker: String,
) -> Result(Nil, String) {
  case roll {
    n if n < 30 -> {
      let #(idx, _) = below(rng, list.length(keys))
      let key = key_at(keys, idx)
      case nm.try_acquire(client, key, 60_000) {
        Ok(Some(handle)) -> grant_release_single(client, handle, lang, worker)
        Ok(None) -> Ok(Nil)
        Error(e) -> Error(e)
      }
    }
    n if n < 50 -> {
      let #(idx, _) = below(rng, list.length(keys))
      let key = key_at(keys, idx)
      use handle <- result.try(nm.acquire(client, key, 60_000))
      grant_release_single(client, handle, lang, worker)
    }
    n if n < 65 -> {
      let #(chosen, _) = choose_keys(rng, keys)
      case nm.try_acquire_many(client, chosen, 60_000) {
        Ok(Some(handle)) ->
          grant_release_composite(client, handle, lang, worker)
        Ok(None) -> Ok(Nil)
        Error(e) -> Error(e)
      }
    }
    n if n < 75 -> {
      let #(chosen, _) = choose_keys(rng, keys)
      use handle <- result.try(nm.acquire_many(client, chosen, 60_000))
      grant_release_composite(client, handle, lang, worker)
    }
    n if n < 92 -> {
      let #(idx, _) = below(rng, list.length(keys))
      let key = key_at(keys, idx)
      use pair <- result.try(nm.acquire_read(client, key))
      let #(lock_uuid, token) = pair
      use _ <- result.try(
        emit_grant(lang, worker, lock_uuid, "read", [key], [#(key, token)]),
      )
      ffi_sleep_ms(1)
      use _ <- result.try(emit_release(lang, worker, lock_uuid))
      nm.release_read(client, key)
    }
    _ -> {
      let #(idx, _) = below(rng, list.length(keys))
      let key = key_at(keys, idx)
      use pair <- result.try(nm.acquire_write(client, key))
      let #(lock_uuid, token) = pair
      use _ <- result.try(
        emit_grant(lang, worker, lock_uuid, "write", [key], [#(key, token)]),
      )
      ffi_sleep_ms(1)
      use _ <- result.try(emit_release(lang, worker, lock_uuid))
      nm.release_write(client, key)
    }
  }
}

fn grant_release_single(
  client: nm.Client,
  handle: nm.SingleLockHandle,
  lang: String,
  worker: String,
) -> Result(Nil, String) {
  use _ <- result.try(
    emit_grant(lang, worker, handle.lock_uuid, "exclusive", [handle.key], [
      #(handle.key, handle.fencing_token),
    ]),
  )
  ffi_sleep_ms(1)
  use _ <- result.try(emit_release(lang, worker, handle.lock_uuid))
  nm.release_single(client, handle)
}

fn grant_release_composite(
  client: nm.Client,
  handle: nm.CompositeLockHandle,
  lang: String,
  worker: String,
) -> Result(Nil, String) {
  use _ <- result.try(emit_grant(
    lang,
    worker,
    handle.lock_uuid,
    "exclusive",
    handle.keys,
    dict.to_list(handle.fencing_tokens),
  ))
  ffi_sleep_ms(1)
  use _ <- result.try(emit_release(lang, worker, handle.lock_uuid))
  nm.release_composite(client, handle)
}

fn emit_grant(
  lang: String,
  worker: String,
  lock_uuid: String,
  kind: String,
  keys: List(String),
  tokens: List(#(String, Int)),
) -> Result(Nil, String) {
  json.object([
    #("event", json.string("grant")),
    #("lang", json.string(lang)),
    #("worker", json.string(worker)),
    #("lockUuid", json.string(lock_uuid)),
    #("kind", json.string(kind)),
    #("keys", json.array(keys, json.string)),
    #("tokens", json.object(token_json(tokens))),
  ])
  |> json.to_string
  |> ffi_emit_event
}

fn emit_release(
  lang: String,
  worker: String,
  lock_uuid: String,
) -> Result(Nil, String) {
  json.object([
    #("event", json.string("release")),
    #("lang", json.string(lang)),
    #("worker", json.string(worker)),
    #("lockUuid", json.string(lock_uuid)),
  ])
  |> json.to_string
  |> ffi_emit_event
}

fn done_event(lang: String, worker: String, ops: Int) -> String {
  json.object([
    #("event", json.string("done")),
    #("lang", json.string(lang)),
    #("worker", json.string(worker)),
    #("ops", json.int(ops)),
  ])
  |> json.to_string
}

fn token_json(tokens: List(#(String, Int))) -> List(#(String, Json)) {
  list.map(tokens, fn(pair) {
    let #(key, token) = pair
    #(key, json.int(token))
  })
}

fn make_keys(prefix: String, count: Int) -> List(String) {
  int.range(from: 0, to: count, with: [], run: fn(acc, i) {
    [prefix <> "-" <> int.to_string(i), ..acc]
  })
  |> list.reverse
}

fn choose_keys(rng: Rng, keys: List(String)) -> #(List(String), Rng) {
  let len = list.length(keys)
  let #(start, rng) = below(rng, len)
  let max_extra = case len > 4 {
    True -> 3
    False -> len - 1
  }
  let #(extra, rng) = below(rng, max_extra)
  let want = 2 + extra
  #(choose_loop(keys, start, want, len, []), rng)
}

fn choose_loop(
  keys: List(String),
  start: Int,
  remaining: Int,
  len: Int,
  acc: List(String),
) -> List(String) {
  case remaining <= 0 {
    True -> list.reverse(acc)
    False -> {
      let idx = modulo(start + remaining - 1, len)
      choose_loop(keys, start, remaining - 1, len, [key_at(keys, idx), ..acc])
    }
  }
}

fn key_at(keys: List(String), idx: Int) -> String {
  case keys, idx {
    [first, ..], 0 -> first
    [_, ..rest], _ -> key_at(rest, idx - 1)
    [], _ -> ""
  }
}

fn below(rng: Rng, n: Int) -> #(Int, Rng) {
  let Rng(state) = rng
  let next = modulo(state * 1_103_515_245 + 12_345, 2_147_483_647)
  #(modulo(next, n), Rng(next))
}

fn modulo(a: Int, n: Int) -> Int {
  case int.modulo(a, by: n) {
    Ok(v) -> v
    Error(_) -> 0
  }
}

fn parse_int(s: String, default: Int) -> Int {
  case int.parse(s) {
    Ok(n) -> n
    Error(_) -> default
  }
}

fn env_or(name: String, default: String) -> String {
  case ffi_getenv(name) {
    Ok(v) -> v
    Error(_) -> default
  }
}
