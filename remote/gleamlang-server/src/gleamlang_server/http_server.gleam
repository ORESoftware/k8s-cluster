import gleam/bit_array
import gleam/bytes_tree
import gleam/erlang/process
import gleam/http/request
import gleam/http/response
import gleam/int
import gleam/option.{Some}
import gleamlang_server/broadcaster
import mist

const host = "0.0.0.0"

const port = 8081

@external(erlang, "gleamlang_server_env", "getenv")
fn env_get(name: String) -> Result(String, Nil)

type WsState {
  WsState(
    tick_subject: process.Subject(broadcaster.StreamMessage),
    broadcaster_subject: process.Subject(broadcaster.Message),
  )
}

pub fn supervised(broker_name: process.Name(broadcaster.Message)) {
  mist.new(fn(req) { route(req, broker_name) })
  |> mist.bind(host)
  |> mist.port(port)
  |> mist.supervised
}

fn route(
  req: request.Request(mist.Connection),
  broker_name: process.Name(broadcaster.Message),
) -> response.Response(mist.ResponseData) {
  let broker_subject = process.named_subject(broker_name)
  process.send(broker_subject, broadcaster.RecordHttpRequest)

  case request.path_segments(req) {
    [] -> redirect("/home")
    ["home"] -> home_page()
    ["healthz"] -> healthz()
    ["metrics"] -> metrics(broker_name)
    ["broadcast"] -> broadcast(req, broker_name)
    ["ws"] -> websocket(req, broker_name)
    _ -> not_found()
  }
}

fn websocket(
  req: request.Request(mist.Connection),
  broker_name: process.Name(broadcaster.Message),
) -> response.Response(mist.ResponseData) {
  mist.websocket(
    request: req,
    on_init: fn(_conn) {
      let broadcaster_subject = process.named_subject(broker_name)
      let tick_subject = process.new_subject()
      let selector = process.new_selector() |> process.select(tick_subject)

      process.send(broadcaster_subject, broadcaster.Subscribe(tick_subject))

      #(
        WsState(
          tick_subject: tick_subject,
          broadcaster_subject: broadcaster_subject,
        ),
        Some(selector),
      )
    },
    on_close: fn(state) {
      let WsState(
        tick_subject: tick_subject,
        broadcaster_subject: broker_subject,
      ) = state
      process.send(broker_subject, broadcaster.Unsubscribe(tick_subject))
    },
    handler: ws_handler,
  )
}

fn ws_handler(
  state: WsState,
  message: mist.WebsocketMessage(broadcaster.StreamMessage),
  conn: mist.WebsocketConnection,
) -> mist.Next(WsState, broadcaster.StreamMessage) {
  case message {
    mist.Text("ping") -> {
      process.send(state.broadcaster_subject, broadcaster.RecordWsMessage)
      let assert Ok(_) = mist.send_text_frame(conn, "{\"type\":\"pong\"}")
      mist.continue(state)
    }

    mist.Text(_) -> {
      process.send(state.broadcaster_subject, broadcaster.RecordWsMessage)
      let assert Ok(_) =
        mist.send_text_frame(
          conn,
          "{\"type\":\"ack\",\"message\":\"send 'ping' for pong; ticks stream automatically\"}",
        )
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

fn redirect(path: String) -> response.Response(mist.ResponseData) {
  response.new(302)
  |> response.set_header("location", path)
  |> response.set_body(mist.Bytes(bytes_tree.from_string("")))
}

fn home_page() -> response.Response(mist.ResponseData) {
  response.new(200)
  |> response.set_header("content-type", "text/html; charset=utf-8")
  |> response.set_body(mist.Bytes(bytes_tree.from_string(home_html)))
}

fn healthz() -> response.Response(mist.ResponseData) {
  response.new(200)
  |> response.set_header("content-type", "application/json")
  |> response.set_body(
    mist.Bytes(bytes_tree.from_string(
      "{\"ok\":true,\"service\":\"gleamlang-server\"}",
    )),
  )
}

fn broadcast(
  req: request.Request(mist.Connection),
  broker_name: process.Name(broadcaster.Message),
) -> response.Response(mist.ResponseData) {
  let expected_secret = broadcast_secret()
  case request.get_header(req, "x-dd-internal-auth") {
    Ok(secret) -> {
      case secret == expected_secret {
        True -> {
          case mist.read_body(req, 1_048_576) {
            Ok(req) -> {
              case bit_array.to_string(req.body) {
                Ok(payload) -> {
                  let broker_subject = process.named_subject(broker_name)
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

fn metrics(
  broker_name: process.Name(broadcaster.Message),
) -> response.Response(mist.ResponseData) {
  let broker_subject = process.named_subject(broker_name)
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
    mist.Bytes(bytes_tree.from_string(
      "# HELP dd_gleamlang_ws_connections Active WebSocket connections.\n"
      <> "# TYPE dd_gleamlang_ws_connections gauge\n"
      <> "dd_gleamlang_ws_connections{service=\"dd-gleamlang-server\"} "
      <> int.to_string(subscribers)
      <> "\n# HELP dd_gleamlang_ticks_total Broadcast tick count.\n"
      <> "# TYPE dd_gleamlang_ticks_total counter\n"
      <> "dd_gleamlang_ticks_total{service=\"dd-gleamlang-server\"} "
      <> int.to_string(ticks)
      <> "\n# HELP dd_gleamlang_http_requests_total HTTP requests observed by the Gleam runtime.\n"
      <> "# TYPE dd_gleamlang_http_requests_total counter\n"
      <> "dd_gleamlang_http_requests_total{service=\"dd-gleamlang-server\"} "
      <> int.to_string(http_requests)
      <> "\n# HELP dd_gleamlang_ws_messages_total WebSocket client messages observed by the Gleam runtime.\n"
      <> "# TYPE dd_gleamlang_ws_messages_total counter\n"
      <> "dd_gleamlang_ws_messages_total{service=\"dd-gleamlang-server\"} "
      <> int.to_string(ws_messages)
      <> "\n# HELP dd_gleamlang_nats_messages_total NATS task events bridged into websocket fanout.\n"
      <> "# TYPE dd_gleamlang_nats_messages_total counter\n"
      <> "dd_gleamlang_nats_messages_total{service=\"dd-gleamlang-server\"} "
      <> int.to_string(nats_messages)
      <> "\n",
    )),
  )
}

fn not_found() -> response.Response(mist.ResponseData) {
  json_response(404, "{\"error\":\"not-found\"}")
}

fn json_response(
  status: Int,
  body: String,
) -> response.Response(mist.ResponseData) {
  response.new(status)
  |> response.set_header("content-type", "application/json")
  |> response.set_body(mist.Bytes(bytes_tree.from_string(body)))
}

const home_html = "<!doctype html><html><head><meta charset=\"utf-8\"/><title>dd gleamlang-server</title><style>body{font-family:system-ui;margin:24px}pre{max-height:50vh;overflow:auto;background:#111;color:#0f0;padding:12px;border-radius:8px}</style></head><body><h1>dd gleamlang-server</h1><p>WebSocket stream endpoint: <code id=\"ws-path\">/ws</code></p><p>Health endpoint: <code id=\"health-path\">/healthz</code></p><pre id=\"log\">connecting...</pre><script>const log=document.getElementById('log');const prefix=location.pathname.startsWith('/gleam/')?'/gleam':'';const wsPath=prefix+'/ws';document.getElementById('ws-path').textContent=wsPath;document.getElementById('health-path').textContent=prefix+'/healthz';const proto=location.protocol==='https:'?'wss':'ws';const ws=new WebSocket(`${proto}://${location.host}${wsPath}`);ws.onopen=()=>{log.textContent='connected\\n';ws.send('ping')};ws.onmessage=(event)=>{log.textContent += event.data + '\\n';log.scrollTop=log.scrollHeight};ws.onclose=()=>{log.textContent += 'closed\\n'}</script></body></html>"
