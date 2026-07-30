//// Mist HTTP / WebSocket server.
////
//// Every non-OPTIONS operation is matched through `route_contract.routes()`.
//// That typed registry is also the source of the OpenAPI document and SDK
//// inputs, so runtime route registration and documentation cannot drift.

import dd_otel_client
import dd_runtime_config_client
import gleam/bit_array
import gleam/bytes_tree
import gleam/erlang/atom
import gleam/http.{Options}
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
import gleamlang_presence_server/api_docs
import gleamlang_presence_server/connection.{
  type ConnScope, ConvScope, UserScope,
}
import gleamlang_presence_server/conversations.{type Conversations}
import gleamlang_presence_server/fanout.{type Fanout}
import gleamlang_presence_server/groups.{
  type ConnGroup, type ConnMsg, type DeviceId, ByConv, ByUser, ByUserDevice,
  Kick, Outbound,
}
import gleamlang_presence_server/nats_transport.{type Nats}
import gleamlang_presence_server/registry.{type Registry}
import gleamlang_presence_server/route_contract
import gleamlang_presence_server/store.{type Store}
import mist.{type Connection, type ResponseData, Bytes}

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
  mist.new(dd_otel_client.trace(fn(req) { with_cors(route(deps, req)) }))
  |> mist.port(port)
  |> mist.bind("0.0.0.0")
  |> mist.supervised
}

/// Stamp CORS headers onto every response. The Kubernetes ingress and network
/// policy are the current boundary for operational routes; the public OpenAPI
/// contract therefore fail-closes them out instead of describing them as
/// Internet-safe APIs.
fn with_cors(resp: Response(ResponseData)) -> Response(ResponseData) {
  resp
  |> response.set_header("access-control-allow-origin", "*")
  |> response.set_header(
    "access-control-allow-methods",
    "GET, POST, DELETE, OPTIONS",
  )
  |> response.set_header(
    "access-control-allow-headers",
    "content-type, x-server-auth",
  )
  |> response.set_header("access-control-max-age", "600")
}

fn route(deps: Deps, req: Request(Connection)) -> Response(ResponseData) {
  case req.method {
    // CORS preflight is generic transport middleware rather than a business
    // operation, so it is intentionally not duplicated across OpenAPI paths.
    Options ->
      response.new(204)
      |> response.set_body(Bytes(bytes_tree.from_string("")))
    _ ->
      case route_contract.match_route(req.method, request.path_segments(req)) {
        Ok(matched) -> dispatch(deps, req, matched)
        Error(_) -> not_found()
      }
  }
}

fn dispatch(
  deps: Deps,
  req: Request(Connection),
  matched: route_contract.RouteMatch,
) -> Response(ResponseData) {
  case matched.route.id {
    route_contract.Help -> help()
    route_contract.PublicOpenApi -> api_docs.json()
    route_contract.PublicDocs -> api_docs.html()
    route_contract.Health -> healthz(deps)
    route_contract.Nodes -> nodes_text()
    route_contract.WebSocket -> handle_ws_upgrade(deps, req)

    route_contract.AddConversationMember -> {
      let conv_id = required_parameter(matched, "conv_id")
      let user_id = required_parameter(matched, "user_id")
      conversations.add_member(deps.conversations, conv_id, user_id)
      ok_text("joined " <> user_id <> " -> " <> conv_id)
    }

    route_contract.RemoveConversationMember -> {
      let conv_id = required_parameter(matched, "conv_id")
      let user_id = required_parameter(matched, "user_id")
      conversations.remove_member(deps.conversations, conv_id, user_id)
      ok_text("left " <> user_id <> " -> " <> conv_id)
    }

    route_contract.ListConversationMembers -> {
      let conv_id = required_parameter(matched, "conv_id")
      let users = conversations.members_of(deps.conversations, conv_id)
      ok_text(string.join(users, "\n"))
    }

    route_contract.BroadcastConversation -> {
      let conv_id = required_parameter(matched, "conv_id")
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

    route_contract.BroadcastUser -> {
      let user_id = required_parameter(matched, "user_id")
      case mist.read_body(req, 1024 * 64) {
        Ok(req_with_body) -> {
          let payload =
            req_with_body.body
            |> bit_array.to_string
            |> result.unwrap("")
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

    route_contract.LogoutDevice -> {
      let user_id = required_parameter(matched, "user_id")
      let device_id = required_parameter(matched, "device_id")
      // Device-targeted kick: closes every ws (user-scoped AND conv-
      // scoped) belonging to this device of this user, cluster-wide.
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
      ok_text("logout queued for user=" <> user_id <> " device=" <> device_id)
    }

    route_contract.RuntimeConfigSnapshot ->
      dd_runtime_config_client.handle_snapshot(req)
    route_contract.RuntimeConfigApply ->
      dd_runtime_config_client.handle_apply(req)
    route_contract.RuntimeConfigReset ->
      dd_runtime_config_client.handle_reset(req)
  }
}

fn required_parameter(
  matched: route_contract.RouteMatch,
  name: String,
) -> String {
  route_contract.parameter(matched.parameters, name)
  |> result.unwrap("")
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
        True -> upgrade(deps, req, ConvScope(user_id, cid, device_id))
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
      "presence-server\n", "---------------\n",
      "GET    /openapi.json                     fail-closed public OpenAPI 3.1\n",
      "GET    /api/docs.json                    OpenAPI alias\n",
      "GET    /api/docs                         Scalar API reference\n",
      "GET    /docs/api                         Scalar API reference alias\n",
      "GET    /healthz                          health JSON\n",
      "GET    /nodes                            BEAM cluster peers\n",
      "GET    /ws?user=alice                    open a user-scoped ws as 'alice'\n",
      "GET    /ws?user=alice&conv=c1            open a conv-scoped ws (must be a member)\n",
      "                                         both accept optional &device=<id>\n",
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
