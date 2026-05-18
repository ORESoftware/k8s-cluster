//// mist HTTP / websocket server.
////
//// Routes:
////   GET    /                                  Help text.
////   GET    /healthz                           JSON health check.
////   GET    /nodes                             Plain text list of connected BEAM peers.
////   GET    /ws?user=<id>                      Upgrade to a user-scoped ws.
////   GET    /ws?user=<id>&conv=<id>            Upgrade to a conv-scoped ws (must be a member).
////                                             Both variants accept optional `&device=<id>`.
////   POST   /conv/<conv_id>/members/<user_id>  Add user to conv (PG + cache + cluster broadcast).
////   DELETE /conv/<conv_id>/members/<user_id>  Remove user from conv.
////   GET    /conv/<conv_id>/members            List members (PG-backed).
////   POST   /conv/<conv_id>/broadcast          Body is broadcast to every conv-scoped
////                                             ws of every current member, on every node, in
////                                             O(peer nodes) cross-node sends.

import gleam/bit_array
import gleam/bytes_tree
import gleam/erlang/atom
import gleam/http.{Delete, Get, Post}
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
import gleamlang_presence_server/connection.{type ConnScope, ConvScope, UserScope}
import gleamlang_presence_server/conversations.{type Conversations}
import gleamlang_presence_server/fanout.{type Fanout}
import gleamlang_presence_server/groups.{
  type ConnGroup, type ConnMsg, type DeviceId, ByConv, Outbound,
}
import gleamlang_presence_server/nats_transport.{type Nats}
import gleamlang_presence_server/registry.{type Registry}
import gleamlang_presence_server/store.{type Store}

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
  mist.new(fn(req) { route(deps, req) })
  |> mist.port(port)
  |> mist.bind("0.0.0.0")
  |> mist.supervised
}

fn route(deps: Deps, req: Request(Connection)) -> Response(ResponseData) {
  let path = request.path_segments(req)
  case req.method, path {
    Get, [] -> help()
    Get, ["healthz"] -> healthz(deps)
    Get, ["nodes"] -> nodes_text()
    Get, ["ws"] -> handle_ws_upgrade(deps, req)

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

    _, _ -> not_found()
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
      "presence-server\n", "---------------\n",
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
    ])
    |> string_tree.to_string
  response.new(200)
  |> response.set_header("content-type", "text/plain; charset=utf-8")
  |> response.set_body(Bytes(bytes_tree.from_string(body)))
}
