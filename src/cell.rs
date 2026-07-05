//! The relay-cell vocabulary.
//!
//! After the per-hop handshake completes, every message between the client and
//! a relay is a `Cell` serialized with bincode and sealed under that hop's
//! forward/backward key. A relay only ever sees the one layer addressed to it;
//! the inner bytes remain encrypted for hops further down the circuit.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Cell {
    /// Forward a already-framed blob verbatim to the next hop. `payload` is the
    /// exact bytes (length prefix included) that the next hop will read as its
    /// own frame. This is the onion-forwarding primitive.
    Relay { payload: Vec<u8> },

    /// Ask this relay to open a connection to another relay at `addr` and send
    /// `create` (the raw handshake blob) to it, extending the circuit one hop.
    Extend { addr: String, create: Vec<u8> },

    /// Interpreted only by the exit hop: open a TCP connection to `host:port`.
    Begin { host: String, port: u16 },

    /// Application payload to/from the exit's destination connection.
    Data { bytes: Vec<u8> },

    /// Tear down the stream.
    End,
}

impl Cell {
    pub fn encode(&self) -> Vec<u8> {
        // bincode with a fixed config is deterministic and compact; a Cell is
        // small, so the default (unbounded) config is fine behind our frame cap.
        return bincode::serialize(self).expect("Cell serialization is infallible");
    }

    pub fn decode(bytes: &[u8]) -> anyhow::Result<Cell> {
        return Ok(bincode::deserialize(bytes)?);
    }
}
