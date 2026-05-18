//// Glue between mist's websocket handler API and the rest of the
//// presence service.
////
//// Each websocket runs in its own Erlang process. On init we:
////
////   1. Create a `Subject(ConnMsg)` that mist's selector delivers custom
////      messages to.
////   2. Register that subject under `ByUser(user_id)` in the local ETS
////      registry and join the same group in cluster-wide `pg`.
////   3. Load the user's current conversations (via the conversations
////      actor, which caches the read) and register/join each `ByConv`.
////   4. Monitor the registry actor's PID so we can re-register if the
////      supervisor restarts it (part (a) of the re-registration design).
////
//// Re-registration flow:
////   - Registry crashes → supervisor restarts it.
////   - This connection's monitor fires → we receive a `Down` message.
////   - We send `ReRegister` to ourselves; the handler re-creates ETS rows,
////     re-joins pg, and re-arms the monitor against the new registry pid.

import gleam/erlang/process.{type Selector, type Subject}
import gleam/list
import gleam/option.{type Option, Some}
import gleam/set.{type Set}
import mist.{
  type WebsocketConnection, type WebsocketMessage, Binary, Closed, Custom,
  Shutdown, Text,
}
import gleamlang_presence_server/conversations.{type Conversations}
import gleamlang_presence_server/groups.{
  type ConnGroup, type ConnMsg, type ConvId, type UserId, ByConv, ByUser,
  JoinConv, LeaveConv, Outbound, ReRegister,
}
import gleamlang_presence_server/pg_groups
import gleamlang_presence_server/registry.{type Registry}

pub type ConnState {
  ConnState(
    user_id: UserId,
    self_subject: Subject(ConnMsg),
    current_convs: Set(ConvId),
    registry: Registry(ConnMsg, ConnGroup),
    conversations: Conversations,
  )
}

/// Returns the `on_init` callback that mist expects. Closes over the deps
/// so that the per-connection setup runs *inside* the mist-spawned ws
/// process — the subject, ETS rows, pg memberships, and the registry
/// monitor all belong to that process.
pub fn make_on_init(
  user_id: UserId,
  registry: Registry(ConnMsg, ConnGroup),
  conversations: Conversations,
) -> fn(WebsocketConnection) -> #(ConnState, Option(Selector(ConnMsg))) {
  fn(_ws_conn) {
    let self = process.new_subject()

    let convs = conversations.convs_of(conversations, user_id)
    register_everything(registry, user_id, self, convs)

    let state =
      ConnState(
        user_id: user_id,
        self_subject: self,
        current_convs: set.from_list(convs),
        registry: registry,
        conversations: conversations,
      )

    let selector = build_selector(self, registry)

    #(state, Some(selector))
  }
}

fn register_everything(
  registry: Registry(ConnMsg, ConnGroup),
  user_id: UserId,
  self: Subject(ConnMsg),
  convs: List(ConvId),
) -> Nil {
  registry.register(registry, ByUser(user_id), self)
  pg_groups.join(group: ByUser(user_id))

  list.each(convs, fn(conv_id) {
    registry.register(registry, ByConv(conv_id), self)
    pg_groups.join(group: ByConv(conv_id))
  })
  Nil
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
      let _ =
        mist.send_text_frame(
          ws_conn,
          "echo[" <> state.user_id <> "]: " <> text,
        )
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
    Outbound(conv_id, payload) ->
      case set.contains(state.current_convs, conv_id) {
        False -> mist.continue(state)
        True -> {
          let frame =
            "conv["
            <> conv_id
            <> "] -> "
            <> state.user_id
            <> ": "
            <> payload
          let _ = mist.send_text_frame(ws_conn, frame)
          mist.continue(state)
        }
      }

    JoinConv(conv_id) -> {
      registry.register(state.registry, ByConv(conv_id), state.self_subject)
      pg_groups.join(group: ByConv(conv_id))
      let new_state =
        ConnState(
          ..state,
          current_convs: set.insert(state.current_convs, conv_id),
        )
      let _ =
        mist.send_text_frame(ws_conn, "system: joined " <> conv_id)
      mist.continue(new_state)
    }

    LeaveConv(conv_id) -> {
      registry.unregister(state.registry, ByConv(conv_id), state.self_subject)
      pg_groups.leave(group: ByConv(conv_id))
      let new_state =
        ConnState(
          ..state,
          current_convs: set.delete(state.current_convs, conv_id),
        )
      let _ =
        mist.send_text_frame(ws_conn, "system: left " <> conv_id)
      mist.continue(new_state)
    }

    ReRegister -> {
      // Re-hydrate from Postgres (the conversations actor caches it) and
      // re-arm everything against the new registry pid.
      conversations.hydrate(state.conversations, state.user_id)
      let convs = conversations.convs_of(state.conversations, state.user_id)
      register_everything(
        state.registry,
        state.user_id,
        state.self_subject,
        convs,
      )

      let new_selector = build_selector(state.self_subject, state.registry)
      let new_state =
        ConnState(..state, current_convs: set.from_list(convs))
      let _ =
        mist.send_text_frame(
          ws_conn,
          "system: re-registered with new registry instance",
        )
      mist.continue(new_state)
      |> mist.with_selector(new_selector)
    }
  }
}

/// Called when the ws process is shutting down. The local ETS registry's
/// monitor and `pg`'s monitor both fire automatically — there's nothing
/// for us to do here, but we keep the hook for symmetry / future logging.
pub fn on_close(_state: ConnState) -> Nil {
  Nil
}
