//// Cluster discovery loop.
////
//// On every tick:
////   1. Read the in-pod service account token from
////      `/var/run/secrets/kubernetes.io/serviceaccount/token`.
////   2. Hit `https://kubernetes.default.svc/api/v1/namespaces/{NS}/pods`
////      with `?labelSelector=app={LABEL}` and `Authorization: Bearer …`.
////   3. Parse pod names, build Erlang node names of the form
////      `presence@{podname}.{service}.{ns}.svc.cluster.local`.
////   4. For each that isn't already a connected node, call
////      `net_kernel:connect_node/1`.
////
//// We deliberately do NOT depend on `kubectl`, the k8s client libraries,
//// or anything else outside the BEAM. The k8s API surface used here is
//// small and stable.
////
//// Configuration (env vars):
////   `CLUSTER_PEERS`                Comma-separated list of full Erlang
////                                  node names (e.g.
////                                  `presence0@127.0.0.1,presence1@127.0.0.1`).
////                                  If set, the loop ONLY connects to these
////                                  peers and ignores k8s. Great for local
////                                  multi-node dev.
////   `CLUSTER_NAMESPACE`            (default `default`)
////   `CLUSTER_LABEL_SELECTOR`       (default `app=presence`)
////   `CLUSTER_NODE_PREFIX`          (default `presence`)
////   `CLUSTER_HEADLESS_SERVICE`     (default `presence-svc`)
////   `CLUSTER_DISCOVERY_INTERVAL_MS` (default `5000`)
////   `KUBERNETES_SERVICE_HOST` /
////   `KUBERNETES_SERVICE_PORT`      (set automatically by k8s; if absent
////                                  AND no `CLUSTER_PEERS` is set, the
////                                  loop short-circuits — useful for dev
////                                  on a single node.)

import gleam/erlang/atom
import gleam/erlang/process
import gleam/http.{Https}
import gleam/http/request
import gleam/httpc
import gleam/int
import gleam/io
import gleam/list
import gleam/otp/actor
import gleam/otp/supervision
import gleam/result
import gleam/string

@external(erlang, "gleamlang_presence_server_ffi", "env")
fn env(name: String) -> Result(String, Nil)

@external(erlang, "gleamlang_presence_server_ffi", "read_file_utf8")
fn read_file_utf8(path: String) -> Result(String, String)

@external(erlang, "net_kernel", "connect_node")
fn net_kernel_connect_node(node: atom.Atom) -> Bool

@external(erlang, "erlang", "nodes")
fn erlang_nodes() -> List(atom.Atom)

pub type Message {
  Tick
}

pub type Cluster =
  process.Subject(Message)

type State {
  State(
    self: process.Subject(Message),
    static_peers: List(String),
    namespace: String,
    label_selector: String,
    node_prefix: String,
    headless_service: String,
    interval_ms: Int,
    api_host: String,
    api_port: String,
    token: Result(String, String),
  )
}

const sa_token_path = "/var/run/secrets/kubernetes.io/serviceaccount/token"

pub fn supervised() -> supervision.ChildSpecification(Cluster) {
  supervision.worker(fn() { start() })
}

pub fn start() -> Result(actor.Started(Cluster), actor.StartError) {
  let static_peers =
    env("CLUSTER_PEERS")
    |> result.unwrap("")
    |> string.split(on: ",")
    |> list.map(string.trim)
    |> list.filter(fn(s) { s != "" })
  let namespace = env("CLUSTER_NAMESPACE") |> result.unwrap("default")
  let label_selector =
    env("CLUSTER_LABEL_SELECTOR") |> result.unwrap("app=presence")
  let node_prefix = env("CLUSTER_NODE_PREFIX") |> result.unwrap("presence")
  let headless_service =
    env("CLUSTER_HEADLESS_SERVICE") |> result.unwrap("presence-svc")
  let interval_ms =
    env("CLUSTER_DISCOVERY_INTERVAL_MS")
    |> result.try(int.parse)
    |> result.unwrap(5000)
  let api_host = env("KUBERNETES_SERVICE_HOST") |> result.unwrap("")
  let api_port =
    env("KUBERNETES_SERVICE_PORT")
    |> result.unwrap("443")

  actor.new_with_initialiser(1000, fn(self) {
    let token = read_file_utf8(sa_token_path)
    let _ = process.send_after(self, interval_ms, Tick)
    actor.initialised(State(
      self: self,
      static_peers: static_peers,
      namespace: namespace,
      label_selector: label_selector,
      node_prefix: node_prefix,
      headless_service: headless_service,
      interval_ms: interval_ms,
      api_host: api_host,
      api_port: api_port,
      token: token,
    ))
    |> actor.returning(self)
    |> Ok
  })
  |> actor.on_message(handle)
  |> actor.start()
}

fn handle(state: State, message: Message) -> actor.Next(State, Message) {
  case message {
    Tick -> {
      let _ = tick(state)
      let _ = process.send_after(state.self, state.interval_ms, Tick)
      actor.continue(state)
    }
  }
}

fn tick(state: State) -> Nil {
  // Static-peer mode wins if set. Useful for local multi-node bringup
  // where we don't want to talk to a k8s API server.
  case state.static_peers {
    [_, ..] -> {
      let known = erlang_nodes() |> list.map(atom.to_string)
      list.each(state.static_peers, fn(node_name) {
        case
          list.contains(known, node_name) || node_name == current_node_string()
        {
          True -> Nil
          False -> {
            let _ = net_kernel_connect_node(atom.create(node_name))
            io.println("cluster: static-peer connect → " <> node_name)
          }
        }
      })
      Nil
    }
    [] -> tick_k8s(state)
  }
}

@external(erlang, "erlang", "node")
fn current_node() -> atom.Atom

fn current_node_string() -> String {
  current_node() |> atom.to_string
}

fn tick_k8s(state: State) -> Nil {
  case state.api_host, state.token {
    "", _ -> {
      // Not inside a cluster and no static peers. Quietly do nothing.
      Nil
    }
    _, Error(reason) -> {
      io.println("cluster: serviceaccount token unreadable: " <> reason)
    }
    host, Ok(token) -> {
      case
        fetch_pods(
          host,
          state.api_port,
          state.namespace,
          state.label_selector,
          token,
        )
      {
        Error(reason) -> {
          io.println("cluster: pod list failed: " <> reason)
        }
        Ok(pod_names) -> {
          let known = erlang_nodes() |> list.map(atom.to_string)
          list.each(pod_names, fn(pod_name) {
            let node_name =
              state.node_prefix
              <> "@"
              <> pod_name
              <> "."
              <> state.headless_service
              <> "."
              <> state.namespace
              <> ".svc.cluster.local"
            case list.contains(known, node_name) {
              True -> Nil
              False -> {
                let _ = net_kernel_connect_node(atom.create(node_name))
                io.println("cluster: connect attempt → " <> node_name)
              }
            }
          })
        }
      }
    }
  }
}

fn fetch_pods(
  host: String,
  port: String,
  namespace: String,
  label_selector: String,
  token: String,
) -> Result(List(String), String) {
  let path =
    "/api/v1/namespaces/"
    <> namespace
    <> "/pods?labelSelector="
    <> percent_encode(label_selector)
  let port_int = int.parse(port) |> result.unwrap(443)

  let req =
    request.new()
    |> request.set_scheme(Https)
    |> request.set_host(host)
    |> request.set_port(port_int)
    |> request.set_path(path)
    |> request.set_header("authorization", "Bearer " <> token)
    |> request.set_header("accept", "application/json")
    |> request.set_body("")

  httpc.send(req)
  |> result.map_error(fn(e) { "httpc: " <> string.inspect(e) })
  |> result.try(fn(resp) {
    case resp.status {
      200 -> Ok(resp.body)
      code -> Error("k8s api status " <> int.to_string(code))
    }
  })
  |> result.try(extract_pod_names)
}

/// Pull pod names out of a k8s `PodList` JSON response. We don't need a
/// full JSON parser — names match `"name":"<podname>"` inside `metadata`,
/// and `metadata.name` is a string of bounded characters. A simple scan
/// over the body is enough and keeps us off any specific JSON dep
/// version. (gleam_json is available too, this is just less ceremony.)
fn extract_pod_names(body: String) -> Result(List(String), String) {
  // Trivial regex-style scan with string.split. The k8s response always
  // has each pod object with `"metadata":{"name":"<podname>",…}` somewhere
  // in it; we split on the literal `"name":"` and take what's before the
  // next quote, then filter by trimming garbage.
  let chunks = string.split(body, on: "\"name\":\"")
  let _first = list.first(chunks)
  let names =
    chunks
    |> list.drop(1)
    |> list.filter_map(fn(chunk) {
      case string.split_once(chunk, on: "\"") {
        Ok(#(name, _rest)) -> {
          case is_pod_name(name) {
            True -> Ok(name)
            False -> Error(Nil)
          }
        }
        Error(_) -> Error(Nil)
      }
    })
  Ok(names)
}

/// Heuristic — a k8s pod name is lowercase alphanumeric plus `-`. We use
/// it to drop occurrences of `"name":"…"` that appear inside container
/// specs, port names, etc.
fn is_pod_name(s: String) -> Bool {
  let len = string.length(s)
  len > 0
  && len < 254
  && string.contains(s, "-")
  && !string.contains(s, ".")
  && !string.contains(s, " ")
  && !string.contains(s, "/")
}

fn percent_encode(s: String) -> String {
  // Minimal: only `=` and space are likely in label selectors used by us;
  // a full URL encoder isn't necessary for `app=presence` style values.
  s
  |> string.replace(each: " ", with: "%20")
  |> string.replace(each: "=", with: "%3D")
  |> string.replace(each: ",", with: "%2C")
}
