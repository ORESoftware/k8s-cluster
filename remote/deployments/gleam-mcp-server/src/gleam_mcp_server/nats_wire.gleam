//// Pure NATS wire helpers used by the transport and by tests.
////
//// Erlang `dd_nats` re-validates subjects on the socket path so a
//// buggy caller cannot inject a new NATS verb via CR/LF/space in a
//// subject. This module is the Gleam-side gate used before we even
//// post to the gen_server.

import gleam/list
import gleam/option.{type Option, None, Some}
import gleam/string

pub const max_subject_bytes = 255

/// A NATS subject (or wildcard subscription) is safe to put on the
/// TCP line if it is non-empty, bounded, and contains no whitespace
/// or control bytes. `*` and `>` remain legal.
pub fn subject_is_safe(subject: String) -> Bool {
  let size = string.byte_size(subject)
  size > 0
  && size <= max_subject_bytes
  && !string.contains(subject, " ")
  && !string.contains(subject, "\r")
  && !string.contains(subject, "\n")
  && !string.contains(subject, "\t")
  && !string.contains(subject, "\u{0000}")
}

/// Case-insensitive header lookup. NATS header names are
/// case-insensitive on the wire.
pub fn header_get(
  headers: List(#(String, String)),
  key: String,
) -> Option(String) {
  let want = string.lowercase(key)
  list.find(headers, fn(kv) { string.lowercase(kv.0) == want })
  |> fn(found) {
    case found {
      Ok(#(_, v)) -> Some(v)
      Error(_) -> None
    }
  }
}

/// Drop NATS copies that the BEAM `pg` mesh already delivered: our
/// own node, or any currently connected distribution peer. Unknown
/// source nodes are kept so NATS still works across a partition.
pub fn source_node_already_delivered(
  source_node: Option(String),
  self_node: String,
  peer_nodes: List(String),
) -> Bool {
  case source_node {
    Some(node) -> node == self_node || list.contains(peer_nodes, node)
    None -> False
  }
}
