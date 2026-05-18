//// Glue between mist's websocket handler API and the rest of the
//// presence service.
////
//// Connection scope
//// ----------------
////
//// A single device opens N+1 websockets to this server: one **user-
//// scoped** ws (`/ws?user=<userId>`) plus one **conv-scoped** ws per
//// active conversation (`/ws?user=<userId>&conv=<convId>`). Both
//// variants optionally accept `&device=<deviceId>` so device-targeted
//// sends can address every ws of one device of one user.
////
//// Each websocket runs in its own Erlang process. On init we:
////
////   1. Create a `Subject(ConnMsg)` that mist's selector delivers custom
////      messages to.
////   2. Register that subject under a single `ConnGroup` axis (or two,
////      counting the optional `ByUserDevice` shadow registration):
////      `ByUser` for user-scope, `ByConv` + `ByUserConv` for conv-scope.
////   3. Monitor the registry actor's pid so we can re-register if the
////      supervisor restarts it.
////
//// Re-registration flow:
////   - Registry crashes → supervisor restarts it.
////   - This connection's monitor fires → we receive a `ReRegister`.
////   - We re-create ETS rows, re-join the same `pg` groups, and re-arm
////     the monitor against the new registry pid.

import gleam/erlang/process.{type Selector, type Subject}
import gleam/option.{type Option, None, Some}
import mist.{
  type WebsocketConnection, type WebsocketMessage, Binary, Closed, Custom,
  Shutdown, Text,
}
import gleamlang_presence_server/conversations.{type Conversations}
import gleamlang_presence_server/groups.{
  type ConnGroup, type ConnMsg, type ConvId, type DeviceId, type UserId,
  ByConv, ByUser, ByUserConv, ByUserDevice, Kick, MembershipChanged, Outbound,
  ReRegister,
}
import gleamlang_presence_server/pg_groups
import gleamlang_presence_server/registry.{type Registry}
import gleamlang_presence_server/wire

/// What kind of websocket this connection is. A single device opens one
/// `UserScope` ws plus one `ConvScope` ws per active conversation.
pub type ConnScope {
  UserScope(user_id: UserId, device_id: Option(DeviceId))
  ConvScope(user_id: UserId, conv_id: ConvId, device_id: Option(DeviceId))
}

pub type ConnState {
  ConnState(
    scope: ConnScope,
    self_subject: Subject(ConnMsg),
    registry: Registry(ConnMsg, ConnGroup),
    conversations: Conversations,
  )
}

/// Returns the `on_init` callback that mist expects. Closes over the deps
/// so that the per-connection setup runs *inside* the mist-spawned ws
/// process — the subject, ETS rows, pg memberships, and the registry
/// monitor all belong to that process.
pub fn make_on_init(
  scope: ConnScope,
  registry: Registry(ConnMsg, ConnGroup),
  conversations: Conversations,
) -> fn(WebsocketConnection) -> #(ConnState, Option(Selector(ConnMsg))) {
  fn(_ws_conn) {
    let self = process.new_subject()

    register_for_scope(registry, scope, self)

    let state =
      ConnState(
        scope: scope,
        self_subject: self,
        registry: registry,
        conversations: conversations,
      )

    let selector = build_selector(self, registry)

    #(state, Some(selector))
  }
}

/// Register the subject in the local ETS registry and join the same
/// keys in `pg` so cross-node fan-out reaches us too.
///
/// Public so tests can drive the registration directly. The runtime
/// uses it via `make_on_init` (called inside the mist ws process).
pub fn register_for_scope(
  registry: Registry(ConnMsg, ConnGroup),
  scope: ConnScope,
  self: Subject(ConnMsg),
) -> Nil {
  case scope {
    UserScope(user_id, device_id) -> {
      registry.register(registry, ByUser(user_id), self)
      pg_groups.join(group: ByUser(user_id))
      register_device_shadow(registry, self, user_id, device_id)
    }
    ConvScope(user_id, conv_id, device_id) -> {
      registry.register(registry, ByConv(conv_id), self)
      pg_groups.join(group: ByConv(conv_id))
      registry.register(registry, ByUserConv(user_id, conv_id), self)
      pg_groups.join(group: ByUserConv(user_id, conv_id))
      register_device_shadow(registry, self, user_id, device_id)
    }
  }
}

fn register_device_shadow(
  registry: Registry(ConnMsg, ConnGroup),
  self: Subject(ConnMsg),
  user_id: UserId,
  device_id: Option(DeviceId),
) -> Nil {
  case device_id {
    Some(d) -> {
      registry.register(registry, ByUserDevice(user_id, d), self)
      pg_groups.join(group: ByUserDevice(user_id, d))
    }
    None -> Nil
  }
}

fn build_selector(
  self: Subject(ConnMsg),
  reg: Registry(ConnMsg, ConnGroup),
) -> Selector(ConnMsg) {
  let base =
    process.new_selector()
    |> process.select(self)

  // Arm a monitor on the registry. If the registry actor exits, fire a
  // ReRegister to ourselves so the new instance learns about this conn.
  case registry.whereis(reg) {
    Ok(pid) -> {
      let _monitor = process.monitor(pid)
      base
      |> process.select_monitors(fn(_down) { ReRegister })
    }
    Error(_) -> base
  }
}

/// The main websocket message handler. Mist invokes this in the WS
/// process for every inbound frame and every selected custom message.
pub fn handle(
  state: ConnState,
  message: WebsocketMessage(ConnMsg),
  ws_conn: WebsocketConnection,
) -> mist.Next(ConnState, ConnMsg) {
  case message {
    Text(text) -> {
      let label = scope_label(state.scope)
      let _ =
        mist.send_text_frame(ws_conn, "echo[" <> label <> "]: " <> text)
      mist.continue(state)
    }
    Binary(_) -> mist.continue(state)
    Closed | Shutdown -> mist.stop()
    Custom(msg) -> handle_custom(state, msg, ws_conn)
  }
}

fn handle_custom(
  state: ConnState,
  msg: ConnMsg,
  ws_conn: WebsocketConnection,
) -> mist.Next(ConnState, ConnMsg) {
  case msg {
    Outbound(payload) -> {
      let _ = mist.send_text_frame(ws_conn, payload)
      mist.continue(state)
    }

    MembershipChanged(conv_id, change) ->
      case state.scope {
        UserScope(_, _) -> {
          let _ =
            mist.send_text_frame(
              ws_conn,
              wire.encode_membership_changed(conv_id, change),
            )
          mist.continue(state)
        }
        // Conv-scoped ws shouldn't be subscribed to ByUser, but if a
        // peer-gossip race delivers one anyway, drop quietly.
        ConvScope(_, _, _) -> mist.continue(state)
      }

    Kick(reason) -> {
      let _ = mist.send_text_frame(ws_conn, wire.encode_kick(reason))
      mist.stop()
    }

    ReRegister -> {
      // Re-hydrate the conversations cache only for user-scoped ws;
      // conv-scoped ws doesn't depend on the user's full conv list.
      case state.scope {
        UserScope(user_id, _) ->
          conversations.hydrate(state.conversations, user_id)
        ConvScope(_, _, _) -> Nil
      }
      register_for_scope(state.registry, state.scope, state.self_subject)

      let new_selector = build_selector(state.self_subject, state.registry)
      let _ = mist.send_text_frame(ws_conn, wire.encode_re_registered())
      mist.continue(state)
      |> mist.with_selector(new_selector)
    }
  }
}

fn scope_label(scope: ConnScope) -> String {
  case scope {
    UserScope(user_id, _) -> "user=" <> user_id
    ConvScope(user_id, conv_id, _) ->
      "user=" <> user_id <> ",conv=" <> conv_id
  }
}

/// Called when the ws process is shutting down. The local ETS registry's
/// monitor and `pg`'s monitor both fire automatically — there's nothing
/// for us to do here, but we keep the hook for symmetry / future logging.
pub fn on_close(_state: ConnState) -> Nil {
  Nil
}
