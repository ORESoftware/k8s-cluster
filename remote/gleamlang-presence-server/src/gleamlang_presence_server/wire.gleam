//// Wire-frame encoders for server-originated system messages.
////
//// Out-of-band frames sent from the server to a websocket client are
//// JSON objects with a `"type"` discriminator. This keeps the client
//// parser uniform (one `JSON.parse` per system frame, switch on
//// `msg.type`) and gives us room to add more fields later without
//// breaking older clients.
////
//// Conv-broadcast bodies (the payload of `Outbound(_)`) are passed
//// through as-is and are NOT wrapped — the application is in control of
//// that wire shape. The encoders below cover only the messages the
//// server itself generates: membership changes and kicks.

import gleam/json
import gleamlang_presence_server/groups.{
  type ConvId, type MembershipChange, type UserId, AddedToConv, RemovedFromConv,
}

/// JSON-encode a `MembershipChanged(conv_id, change)` event for the wire.
///
///   AddedToConv(members):
///     {"type":"membership-changed","change":"added",
///      "conv":"<conv_id>","members":["<u1>","<u2>",...]}
///
///   RemovedFromConv:
///     {"type":"membership-changed","change":"removed","conv":"<conv_id>"}
///
/// The `members` array on `AddedToConv` is the conv's full current
/// member list (including the just-added user). The client can use it
/// directly instead of a follow-up `GET /conv/<id>/members` round-trip.
pub fn encode_membership_changed(conv_id: ConvId, change: MembershipChange) -> String {
  case change {
    AddedToConv(members) ->
      json.object([
        #("type", json.string("membership-changed")),
        #("change", json.string("added")),
        #("conv", json.string(conv_id)),
        #("members", json.array(members, json.string)),
      ])
      |> json.to_string
    RemovedFromConv ->
      json.object([
        #("type", json.string("membership-changed")),
        #("change", json.string("removed")),
        #("conv", json.string(conv_id)),
      ])
      |> json.to_string
  }
}

/// JSON-encode a `Kick(reason)` event for the wire.
///
///   {"type":"kick","reason":"<reason>"}
///
/// Sent immediately before the server closes the ws.
pub fn encode_kick(reason: String) -> String {
  json.object([
    #("type", json.string("kick")),
    #("reason", json.string(reason)),
  ])
  |> json.to_string
}

/// Convenience: format the "system: re-registered ..." notification the
/// connection emits after re-registering against a new registry pid.
/// Kept here so the wire shape is in one place; not strictly needed for
/// client parsing today, but pinned in case we want to JSON-ify it
/// later.
pub fn encode_re_registered() -> String {
  json.object([
    #("type", json.string("re-registered")),
  ])
  |> json.to_string
}

/// Internal helper used by tests and (transitively) by the encoder
/// above. Re-exports the canonical UserId/ConvId types so callers in
/// the test tree don't have to import groups just for the type alias.
pub type Members =
  List(UserId)
