//// Domain types shared between the local ETS registry, the cross-node `pg`
//// shadow registry, the fanout relay, the conversations actor, and the
//// per-connection websocket processes.
////
//// Connection topology
//// -------------------
////
//// A single device opens MULTIPLE websocket connections to this server:
////
////   - exactly one **user-scoped** ws  (`/ws?user=<userId>`), which
////     receives membership-change notifications and any other per-user
////     out-of-band traffic; and
////   - one **conv-scoped** ws per active conversation
////     (`/ws?user=<userId>&conv=<convId>`), which receives that
////     conversation's broadcast frames.
////
//// `ConnGroup` is the key type used to index those connections from the
//// dispatch side. Four axes are addressable:
////
////   - `ByUser(user_id)` — every user-scoped ws of the user, on every
////     node. Used by `MembershipChanged` events so each device's user-ws
////     learns when it should open/close conv-scoped websockets.
////   - `ByUserDevice(user_id, device_id)` — every ws (user- or conv-
////     scoped) belonging to a single device of a user. Used for targeted
////     "log out this device" semantics.
////   - `ByConv(conv_id)` — every conv-scoped ws of the conversation, on
////     every node. The conv-broadcast endpoint sends `Outbound(_)` here.
////   - `ByUserConv(user_id, conv_id)` — the conv-scoped ws(es) belonging
////     to one user/conv pair. Used to deliver a `Kick(_)` when the user
////     is removed from the conv so the server-side closes that user's
////     conv-ws cluster-wide (defense-in-depth on top of the client-side
////     close triggered by `MembershipChanged(RemovedFromConv)`).
////
//// A user-scoped ws is registered under `ByUser` (and optionally
//// `ByUserDevice` if the client supplied a `device` query param). A conv-
//// scoped ws is registered under `ByConv` and `ByUserConv` (and
//// optionally `ByUserDevice`). Crucially, a single ws is **never**
//// subscribed to more than one conv group — that's the responsibility of
//// the client, by opening additional websockets.

pub type UserId =
  String

pub type ConvId =
  String

pub type DeviceId =
  String

pub type ConnGroup {
  ByUser(user_id: UserId)
  ByUserDevice(user_id: UserId, device_id: DeviceId)
  ByConv(conv_id: ConvId)
  ByUserConv(user_id: UserId, conv_id: ConvId)
}

/// Membership-change direction, used by `MembershipChanged` events that
/// fan out to the user's user-scoped websockets.
pub type MembershipChange {
  AddedToConv
  RemovedFromConv
}

/// Messages sent to a per-connection actor from outside its mist process.
///
/// - `Outbound(payload)`: the payload is forwarded to the websocket
///   client verbatim. The dispatcher decides which group(s) to send to
///   (`ByConv` for a conv broadcast, `ByUserDevice` for a per-device
///   notification, etc.); the receiving ws doesn't need to know what
///   group routed the message to it.
/// - `MembershipChanged(conv_id, change)`: emitted by the conversations
///   actor when the user is added to or removed from a conversation,
///   delivered to `ByUser(user_id)`. The client uses it to open/close
///   its own conv-scoped websockets. Conv-scoped websockets that
///   incidentally receive this (shouldn't happen given the registration
///   rules, but is possible during peer-gossip races) ignore it.
/// - `Kick(reason)`: tells the connection to send a short system frame
///   to the client and stop. Used when a user is removed from a conv
///   (the matching conv-ws gets kicked) and reserved for future "log out
///   this device" semantics.
/// - `ReRegister`: sent by the connection itself after it observes the
///   registry actor going down. The handler re-creates ETS rows and
///   re-joins the relevant `pg` groups for its scope.
pub type ConnMsg {
  Outbound(payload: String)
  MembershipChanged(conv_id: ConvId, change: MembershipChange)
  Kick(reason: String)
  ReRegister
}
