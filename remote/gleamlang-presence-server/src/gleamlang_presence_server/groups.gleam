//// Domain types shared between the local ETS registry, the cross-node `pg`
//// shadow registry, the fanout relay, the conversations actor, and the
//// per-connection websocket processes.
////
//// `ConnGroup` is the key type used to index connections on both axes:
////
////   - `ByUser(user_id)` — every device the user has online (on this node
////     and across the cluster).
////   - `ByConv(conv_id)` — every device of every member of a conversation.
////
//// A single connection subject is registered under one `ByUser` group and
//// one `ByConv` group per conversation the user is currently a member of.
//// Broadcasting to a conv is a single `dispatch_group(ByConv(_), …)` call;
//// the registry handles the fan-out across users + devices for us.

pub type UserId =
  String

pub type ConvId =
  String

pub type ConnGroup {
  ByUser(user_id: UserId)
  ByConv(conv_id: ConvId)
}

/// Messages sent to a per-connection actor from outside its mist process.
///
/// - `Outbound(conv_id, payload)`: the only frame that gets relayed to the
///   client. The connection actor filters against its local "currently a
///   member of" set before sending, so a `LeaveConv` racing with a
///   broadcast can't leak.
/// - `JoinConv` / `LeaveConv`: control messages from the conversations
///   actor when membership changes mid-session.
/// - `ReRegister`: sent by the connection itself (via `process.send_after`)
///   after it observes the registry going down, so the new registry
///   instance learns about it.
pub type ConnMsg {
  Outbound(conv_id: ConvId, payload: String)
  JoinConv(conv_id: ConvId)
  LeaveConv(conv_id: ConvId)
  ReRegister
}
