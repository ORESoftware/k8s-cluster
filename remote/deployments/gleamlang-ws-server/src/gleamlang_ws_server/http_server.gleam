//// mist HTTP / websocket server.
////
//// Routes (presence subsystem):
////   GET    /                                            Help text.
////   GET    /healthz                                     JSON health check.
////   GET    /nodes                                       Plain text list of connected BEAM peers.
////   GET    /ws?user=<id>                                Upgrade to a user-scoped ws.
////   GET    /ws?user=<id>&conv=<id>                      Upgrade to a conv-scoped ws (must be a member).
////                                                       Both variants accept optional `&device=<id>`.
////   POST   /conv/<conv_id>/members/<user_id>            Add user to conv (PG + cache + cluster broadcast).
////   DELETE /conv/<conv_id>/members/<user_id>            Remove user from conv.
////   GET    /conv/<conv_id>/members                      List members (PG-backed).
////   POST   /conv/<conv_id>/broadcast                    Body broadcast cluster-wide to every conv-scoped
////                                                       ws of every current member.
////   POST   /user/<user_id>/broadcast                    Body broadcast to every user-scoped ws
////                                                       of `<user_id>` on every node.
////   POST   /user/<user_id>/devices/<device_id>/logout   Closes every ws of one device of one user.
////
//// Routes (legacy broadcaster subsystem):
////   GET    /home                                        Lightweight HTML page that opens a worker ws.
////   GET    /metrics                                     Prometheus metrics for the broadcaster.
////   POST   /broadcast                                   Internal localhost fanout endpoint
////                                                       (NATS-bridge sidecar -> broadcaster).
////   GET    /worker-ws/<secret>                          Secret-gated worker ws on the broadcaster
////                                                       tick stream; can also publish frames.

import gleam/bit_array
import gleam/bytes_tree
import gleam/erlang/atom
import gleam/erlang/process.{type Name}
import gleam/http.{Delete, Get, Options, Post}
import gleam/http/request.{type Request}
import gleam/http/response.{type Response}
import gleam/int
import gleam/list
import gleam/option.{type Option, None, Some}
import gleam/otp/static_supervisor.{type Supervisor}
import gleam/otp/supervision.{type ChildSpecification}
import gleam/result
import gleam/string
import gleam/string_tree
import mist.{type Connection, type ResponseData, Bytes}
import gleamlang_ws_server/api_docs
import gleamlang_ws_server/broadcaster
import gleamlang_ws_server/connection.{type ConnScope, ConvScope, UserScope}
import gleamlang_ws_server/conversations.{type Conversations}
import gleamlang_ws_server/fanout.{type Fanout}
import gleamlang_ws_server/groups.{
  type ConnGroup, type ConnMsg, type DeviceId, ByConv, ByUser, ByUserDevice,
  Kick, Outbound,
}
import gleamlang_ws_server/nats_transport.{type Nats}
import gleamlang_ws_server/registry.{type Registry}
import gleamlang_ws_server/store.{type Store}

@external(erlang, "gleamlang_ws_server_ffi", "getenv")
fn env_get(name: String) -> Result(String, Nil)

@external(erlang, "gleamlang_ws_server_ffi", "publish_nats")
fn nats_publish_via_sidecar(payload: String) -> Result(Nil, Nil)

pub type Deps {
  Deps(
    registry: Registry(ConnMsg, ConnGroup),
    conversations: Conversations,
    fanout: Fanout,
    store: Store,
    /// Optional NATS handle. When present, conv broadcasts are mirrored
    /// to `presence.broadcast.conv.<conv_id>`. Same-cluster receivers
    /// drop the NATS copy (Source-Node header dedup) so locals don't
    /// double-fire.
    nats: Option(Nats),
    /// Legacy broadcaster ticker (named). Drives `/worker-ws/<secret>`,
    /// `/broadcast`, and `/metrics`.
    broadcaster: Name(broadcaster.Message),
  )
}

@external(erlang, "erlang", "node")
fn erlang_node() -> atom.Atom

@external(erlang, "erlang", "nodes")
fn erlang_nodes() -> List(atom.Atom)

pub fn supervised(
  port port: Int,
  deps deps: Deps,
) -> ChildSpecification(Supervisor) {
  mist.new(fn(req) { with_cors(route(deps, req)) })
  |> mist.port(port)
  |> mist.bind("0.0.0.0")
  |> mist.supervised
}

/// Stamp CORS headers onto every response. Permissive on purpose so
/// browsers loading the test UI from a different origin (e.g. the
/// `web-home-rs` page on :8080) can hit `/conv/...` and `/user/...`
/// directly. No auth lives on this endpoint yet anyway.
fn with_cors(resp: Response(ResponseData)) -> Response(ResponseData) {
  resp
  |> response.set_header("access-control-allow-origin", "*")
  |> response.set_header(
    "access-control-allow-methods",
    "GET, POST, DELETE, OPTIONS",
  )
  |> response.set_header("access-control-allow-headers", "content-type")
  |> response.set_header("access-control-max-age", "600")
}

fn route(deps: Deps, req: Request(Connection)) -> Response(ResponseData) {
  let path = request.path_segments(req)
  case req.method, path {
    // CORS preflight. Browsers send this before any cross-origin
    // POST/DELETE; respond 204 No Content with the allow-* headers
    // (those are added by `with_cors`).
    Options, _ ->
      response.new(204)
      |> response.set_body(Bytes(bytes_tree.from_string("")))

    Get, [] -> help()
    Get, ["home"] -> home_page()
    Get, ["docs", "api"] -> api_docs.html()
    Get, ["api", "docs"] -> api_docs.html()
    Get, ["api", "docs.json"] -> api_docs.json()
    Get, ["healthz"] -> healthz(deps)
    Get, ["nodes"] -> nodes_text()
    Get, ["metrics"] -> metrics(deps)
    Get, ["ws"] -> handle_ws_upgrade(deps, req)
    Get, ["worker-ws", secret] -> worker_websocket(deps, req, secret)
    Post, ["broadcast"] -> broadcast(deps, req)

    Post, ["conv", conv_id, "members", user_id] -> {
      conversations.add_member(deps.conversations, conv_id, user_id)
      ok_text("joined " <> user_id <> " -> " <> conv_id)
    }

    Delete, ["conv", conv_id, "members", user_id] -> {
      conversations.remove_member(deps.conversations, conv_id, user_id)
      ok_text("left " <> user_id <> " -> " <> conv_id)
    }

    Get, ["conv", conv_id, "members"] -> {
      let users = conversations.members_of(deps.conversations, conv_id)
      ok_text(string.join(users, "\n"))
    }

    Post, ["conv", conv_id, "broadcast"] -> {
      case mist.read_body(req, 1024 * 64) {
        Ok(req_with_body) -> {
          let payload =
            req_with_body.body
            |> bit_array.to_string
            |> result.unwrap("")
          // One call: cluster-wide fan-out to every conv-scoped ws of
          // every current member of `conv_id`. Local conv-ws's get the
          // message via ETS dispatch; remote nodes' conv-ws's get it via
          // the fanout relay (O(peer nodes) cross-node sends).
          fanout.broadcast(
            deps.fanout,
            deps.registry,
            ByConv(conv_id),
            Outbound(payload),
          )
          // Mirror to NATS for cross-cluster / external subscribers.
          // Same-cluster receivers drop on Source-Node match.
          case deps.nats {
            Some(nats) ->
              nats_transport.publish(
                nats,
                subject: nats_transport.conv_subject(conv_id),
                payload: bit_array.from_string(payload),
                headers: [],
              )
            None -> Nil
          }
          ok_text("broadcast queued for " <> conv_id)
        }
        Error(_) -> bad_request("could not read body")
      }
    }

    Post, ["user", user_id, "broadcast"] -> {
      case mist.read_body(req, 1024 * 64) {
        Ok(req_with_body) -> {
          let payload =
            req_with_body.body
            |> bit_array.to_string
            |> result.unwrap("")
          // Per-user system message: fans out to every user-scoped ws
          // of this user on every node. Conv-scoped ws's of the same
          // user are NOT addressed by this — for that, send to each
          // conv via /conv/<id>/broadcast. NATS mirroring left to a
          // follow-up if/when external subscribers need it.
          fanout.broadcast(
            deps.fanout,
            deps.registry,
            ByUser(user_id),
            Outbound(payload),
          )
          ok_text("user-broadcast queued for " <> user_id)
        }
        Error(_) -> bad_request("could not read body")
      }
    }

    Post, ["user", user_id, "devices", device_id, "logout"] -> {
      // Device-targeted kick: closes every ws (user-scoped AND conv-
      // scoped) belonging to this device of this user, cluster-wide.
      // The connection's `Kick` handler sends a JSON
      // `{"type":"kick","reason":"<reason>"}` frame and stops the ws.
      // Body is the optional reason (defaults to "logout").
      let reason = case mist.read_body(req, 1024) {
        Ok(req_with_body) ->
          req_with_body.body
          |> bit_array.to_string
          |> result.unwrap("")
          |> default_if_blank("logout")
        Error(_) -> "logout"
      }
      fanout.broadcast(
        deps.fanout,
        deps.registry,
        ByUserDevice(user_id, device_id),
        Kick(reason),
      )
      ok_text(
        "logout queued for user=" <> user_id <> " device=" <> device_id,
      )
    }

    _, _ -> not_found()
  }
}

fn default_if_blank(s: String, default: String) -> String {
  case s {
    "" -> default
    _ -> s
  }
}

fn handle_ws_upgrade(
  deps: Deps,
  req: Request(Connection),
) -> Response(ResponseData) {
  let queries = request.get_query(req) |> result.unwrap([])
  let user_id = queries |> list.key_find("user") |> result.unwrap("anonymous")
  let conv_id =
    queries
    |> list.key_find("conv")
    |> option.from_result
    |> nonempty_option
  let device_id: Option(DeviceId) =
    queries
    |> list.key_find("device")
    |> option.from_result
    |> nonempty_option

  case conv_id {
    Some(cid) ->
      case is_member(deps, cid, user_id) {
        True ->
          upgrade(deps, req, ConvScope(user_id, cid, device_id))
        False -> forbidden("user is not a member of conv " <> cid)
      }
    None -> upgrade(deps, req, UserScope(user_id, device_id))
  }
}

fn upgrade(
  deps: Deps,
  req: Request(Connection),
  scope: ConnScope,
) -> Response(ResponseData) {
  mist.websocket(
    request: req,
    handler: connection.handle,
    on_init: connection.make_on_init(scope, deps.registry, deps.conversations),
    on_close: connection.on_close,
  )
}

fn is_member(deps: Deps, conv_id: String, user_id: String) -> Bool {
  conversations.members_of(deps.conversations, conv_id)
  |> list.any(fn(u) { u == user_id })
}

fn nonempty_option(o: Option(String)) -> Option(String) {
  case o {
    Some(s) ->
      case s {
        "" -> None
        _ -> Some(s)
      }
    None -> None
  }
}

fn healthz(deps: Deps) -> Response(ResponseData) {
  let node = erlang_node() |> atom.to_string
  let body =
    "{\"status\":\"ok\",\"node\":\""
    <> node
    <> "\",\"store\":\""
    <> store.describe(deps.store)
    <> "\",\"peers\":"
    <> int.to_string(list.length(erlang_nodes()))
    <> "}\n"
  response.new(200)
  |> response.set_header("content-type", "application/json")
  |> response.set_body(Bytes(bytes_tree.from_string(body)))
}

fn nodes_text() -> Response(ResponseData) {
  let self = erlang_node() |> atom.to_string
  let peers =
    erlang_nodes()
    |> list.map(atom.to_string)
  let lines = [self <> "  (self)", ..peers]
  ok_text(string.join(lines, "\n"))
}

fn ok_text(body: String) -> Response(ResponseData) {
  response.new(200)
  |> response.set_header("content-type", "text/plain; charset=utf-8")
  |> response.set_body(Bytes(bytes_tree.from_string(body <> "\n")))
}

fn bad_request(body: String) -> Response(ResponseData) {
  response.new(400)
  |> response.set_header("content-type", "text/plain; charset=utf-8")
  |> response.set_body(Bytes(bytes_tree.from_string(body <> "\n")))
}

fn forbidden(body: String) -> Response(ResponseData) {
  response.new(403)
  |> response.set_header("content-type", "text/plain; charset=utf-8")
  |> response.set_body(Bytes(bytes_tree.from_string(body <> "\n")))
}

fn not_found() -> Response(ResponseData) {
  response.new(404)
  |> response.set_header("content-type", "text/plain; charset=utf-8")
  |> response.set_body(Bytes(bytes_tree.from_string("not found\n")))
}

fn help() -> Response(ResponseData) {
  let body =
    string_tree.from_strings([
      "gleamlang-ws-server\n", "-------------------\n",
      "GET    /healthz                          health JSON\n",
      "GET    /nodes                            BEAM cluster peers\n",
      "GET    /home                             debug HTML page (worker-ws)\n",
      "GET    /metrics                          prometheus metrics (broadcaster)\n",
      "GET    /ws?user=alice                    open a user-scoped ws as 'alice'\n",
      "GET    /ws?user=alice&conv=c1            open a conv-scoped ws (must be a member)\n",
      "                                         both accept optional &device=<id>\n",
      "GET    /worker-ws/<secret>               broadcaster tick stream (secret-gated)\n",
      "POST   /broadcast                        internal localhost fanout (sidecar)\n",
      "POST   /conv/<id>/members/<user>         add user to conv\n",
      "DELETE /conv/<id>/members/<user>         remove user from conv\n",
      "GET    /conv/<id>/members                list members\n",
      "POST   /conv/<id>/broadcast              body broadcast cluster-wide\n",
      "                                         to every conv-scoped ws of every member\n",
      "                                         via local ETS + fanout relay\n",
      "POST   /user/<id>/broadcast              body broadcast to every user-scoped\n",
      "                                         ws of <id> on every node\n",
      "POST   /user/<u>/devices/<d>/logout      close every ws of one device of one user\n",
      "                                         (body = optional kick reason)\n",
    ])
    |> string_tree.to_string
  response.new(200)
  |> response.set_header("content-type", "text/plain; charset=utf-8")
  |> response.set_body(Bytes(bytes_tree.from_string(body)))
}

// ─── legacy broadcaster routes ─────────────────────────────────────────

type WorkerWsState {
  WorkerWsState(
    tick_subject: process.Subject(broadcaster.StreamMessage),
    broadcaster_subject: process.Subject(broadcaster.Message),
    can_broadcast: Bool,
  )
}

fn worker_websocket(
  deps: Deps,
  req: Request(Connection),
  secret: String,
) -> Response(ResponseData) {
  case secret == worker_ws_secret() {
    True -> open_worker_websocket(deps, req, can_broadcast: True)
    False -> json_response(401, "{\"error\":\"unauthorized\"}")
  }
}

fn open_worker_websocket(
  deps: Deps,
  req: Request(Connection),
  can_broadcast can_broadcast: Bool,
) -> Response(ResponseData) {
  let broker_name = deps.broadcaster
  mist.websocket(
    request: req,
    on_init: fn(_conn) {
      let broadcaster_subject = process.named_subject(broker_name)
      let tick_subject = process.new_subject()
      let selector = process.new_selector() |> process.select(tick_subject)
      process.send(broadcaster_subject, broadcaster.Subscribe(tick_subject))
      #(
        WorkerWsState(
          tick_subject: tick_subject,
          broadcaster_subject: broadcaster_subject,
          can_broadcast: can_broadcast,
        ),
        Some(selector),
      )
    },
    on_close: fn(state) {
      let WorkerWsState(
        tick_subject: tick_subject,
        broadcaster_subject: broker_subject,
        can_broadcast: _,
      ) = state
      process.send(broker_subject, broadcaster.Unsubscribe(tick_subject))
    },
    handler: worker_ws_handler,
  )
}

fn worker_ws_handler(
  state: WorkerWsState,
  message: mist.WebsocketMessage(broadcaster.StreamMessage),
  conn: mist.WebsocketConnection,
) -> mist.Next(WorkerWsState, broadcaster.StreamMessage) {
  case message {
    mist.Text("ping") -> {
      process.send(state.broadcaster_subject, broadcaster.RecordWsMessage)
      let assert Ok(_) = mist.send_text_frame(conn, "{\"type\":\"pong\"}")
      mist.continue(state)
    }
    mist.Text(payload) -> {
      process.send(state.broadcaster_subject, broadcaster.RecordWsMessage)
      let _ = case state.can_broadcast {
        True -> {
          process.send(
            state.broadcaster_subject,
            broadcaster.BroadcastJson(payload),
          )
          let assert Ok(_) =
            mist.send_text_frame(conn, "{\"type\":\"ack\",\"broadcast\":true}")
        }
        False -> {
          let _ = nats_publish_via_sidecar(payload)
          let assert Ok(_) =
            mist.send_text_frame(
              conn,
              "{\"type\":\"ack\",\"message\":\"send 'ping' for pong; ticks stream automatically\"}",
            )
        }
      }
      mist.continue(state)
    }
    mist.Binary(_) -> mist.continue(state)
    mist.Custom(broadcaster.StreamJson(payload)) -> {
      let assert Ok(_) = mist.send_text_frame(conn, payload)
      mist.continue(state)
    }
    mist.Closed -> mist.stop()
    mist.Shutdown -> mist.stop()
  }
}

fn broadcast(
  deps: Deps,
  req: Request(Connection),
) -> Response(ResponseData) {
  let expected_secret = broadcast_secret()
  case request.get_header(req, "x-dd-internal-auth") {
    Ok(secret) -> {
      case secret == expected_secret {
        True -> {
          case mist.read_body(req, 1_048_576) {
            Ok(req) -> {
              case bit_array.to_string(req.body) {
                Ok(payload) -> {
                  let broker_subject = process.named_subject(deps.broadcaster)
                  process.send(
                    broker_subject,
                    broadcaster.BroadcastJson(payload),
                  )
                  json_response(202, "{\"ok\":true}")
                }
                Error(_) -> json_response(400, "{\"error\":\"body-not-utf8\"}")
              }
            }
            Error(_) -> json_response(400, "{\"error\":\"invalid-body\"}")
          }
        }
        False -> json_response(401, "{\"error\":\"unauthorized\"}")
      }
    }
    _ -> json_response(401, "{\"error\":\"unauthorized\"}")
  }
}

fn broadcast_secret() -> String {
  let assert Ok(secret) = env_get("GLEAM_BROADCAST_SECRET")
  case secret {
    "" -> panic as "GLEAM_BROADCAST_SECRET must be configured"
    value -> value
  }
}

fn worker_ws_secret() -> String {
  case env_get("GLEAM_WORKER_WS_SECRET") {
    Ok(secret) ->
      case secret {
        "" -> broadcast_secret()
        value -> value
      }
    Error(_) -> broadcast_secret()
  }
}

fn metrics(deps: Deps) -> Response(ResponseData) {
  let broker_subject = process.named_subject(deps.broadcaster)
  let snapshot = process.call(broker_subject, 1000, broadcaster.GetSnapshot)
  let broadcaster.MetricsSnapshot(
    subscribers: subscribers,
    ticks: ticks,
    http_requests: http_requests,
    ws_messages: ws_messages,
    nats_messages: nats_messages,
  ) = snapshot

  response.new(200)
  |> response.set_header(
    "content-type",
    "text/plain; version=0.0.4; charset=utf-8",
  )
  |> response.set_body(
    Bytes(bytes_tree.from_string(
      "# HELP dd_gleamlang_ws_connections Active worker WebSocket connections.\n"
      <> "# TYPE dd_gleamlang_ws_connections gauge\n"
      <> "dd_gleamlang_ws_connections{service=\"dd-gleamlang-ws-server\"} "
      <> int.to_string(subscribers)
      <> "\n# HELP dd_gleamlang_ticks_total Broadcast tick count.\n"
      <> "# TYPE dd_gleamlang_ticks_total counter\n"
      <> "dd_gleamlang_ticks_total{service=\"dd-gleamlang-ws-server\"} "
      <> int.to_string(ticks)
      <> "\n# HELP dd_gleamlang_http_requests_total HTTP requests observed by the Gleam runtime.\n"
      <> "# TYPE dd_gleamlang_http_requests_total counter\n"
      <> "dd_gleamlang_http_requests_total{service=\"dd-gleamlang-ws-server\"} "
      <> int.to_string(http_requests)
      <> "\n# HELP dd_gleamlang_ws_messages_total WebSocket client messages observed by the Gleam runtime.\n"
      <> "# TYPE dd_gleamlang_ws_messages_total counter\n"
      <> "dd_gleamlang_ws_messages_total{service=\"dd-gleamlang-ws-server\"} "
      <> int.to_string(ws_messages)
      <> "\n# HELP dd_gleamlang_nats_messages_total NATS task events bridged into worker fanout.\n"
      <> "# TYPE dd_gleamlang_nats_messages_total counter\n"
      <> "dd_gleamlang_nats_messages_total{service=\"dd-gleamlang-ws-server\"} "
      <> int.to_string(nats_messages)
      <> "\n",
    )),
  )
}

fn home_page() -> Response(ResponseData) {
  response.new(200)
  |> response.set_header("content-type", "text/html; charset=utf-8")
  |> response.set_body(Bytes(bytes_tree.from_string(home_html)))
}

fn json_response(
  status: Int,
  body: String,
) -> Response(ResponseData) {
  response.new(status)
  |> response.set_header("content-type", "application/json")
  |> response.set_body(Bytes(bytes_tree.from_string(body)))
}

const home_html = "<!doctype html><html><head><meta charset=\"utf-8\"/><title>dd gleamlang-ws-server</title><style>body{font-family:system-ui;margin:24px}pre{max-height:50vh;overflow:auto;background:#111;color:#0f0;padding:12px;border-radius:8px}</style></head><body><h1>dd gleamlang-ws-server</h1><p>Worker WebSocket: <code id=\"ws-path\">/worker-ws/&lt;secret&gt;</code></p><p>Health: <code id=\"health-path\">/healthz</code> &nbsp; Metrics: <code>/metrics</code></p><pre id=\"log\">connecting...</pre><script>const log=document.getElementById('log');const prefix=location.pathname.startsWith('/gleam/')?'/gleam':'';const wsPath=prefix+'/worker-ws/PASTE-YOUR-SECRET-HERE';document.getElementById('ws-path').textContent=wsPath;document.getElementById('health-path').textContent=prefix+'/healthz';const proto=location.protocol==='https:'?'wss':'ws';const ws=new WebSocket(`${proto}://${location.host}${wsPath}`);ws.onopen=()=>{log.textContent='connected\\n';ws.send('ping')};ws.onmessage=(event)=>{log.textContent += event.data + '\\n';log.scrollTop=log.scrollHeight};ws.onclose=()=>{log.textContent += 'closed\\n'}</script></body></html>"
