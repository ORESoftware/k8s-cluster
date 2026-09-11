//! The relay-cell vocabulary.
//!
//! After the per-hop handshake completes, every message between the client and
//! a relay is a `Cell` encoded with a compact, bounded wire format and sealed
//! under that hop's forward/backward key. A relay only ever sees the one layer
//! addressed to it; the inner bytes remain encrypted for hops further down the
//! circuit.

use anyhow::{bail, ensure, Context, Result};

use crate::wire::MAX_FRAME_LEN;

const TAG_RELAY: u8 = 0;
const TAG_EXTEND: u8 = 1;
const TAG_BEGIN: u8 = 2;
const TAG_DATA: u8 = 3;
const TAG_END: u8 = 4;

#[derive(Debug, Clone, PartialEq, Eq)]
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
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        match self {
            Cell::Relay { payload } => {
                out.push(TAG_RELAY);
                put_bytes(&mut out, payload)?;
            }
            Cell::Extend { addr, create } => {
                out.push(TAG_EXTEND);
                put_string(&mut out, addr)?;
                put_bytes(&mut out, create)?;
            }
            Cell::Begin { host, port } => {
                out.push(TAG_BEGIN);
                put_string(&mut out, host)?;
                out.extend_from_slice(&port.to_be_bytes());
            }
            Cell::Data { bytes } => {
                out.push(TAG_DATA);
                put_bytes(&mut out, bytes)?;
            }
            Cell::End => out.push(TAG_END),
        }
        ensure!(
            out.len() <= MAX_FRAME_LEN,
            "encoded cell exceeds frame limit"
        );
        return Ok(out);
    }

    pub fn decode(bytes: &[u8]) -> Result<Cell> {
        ensure!(!bytes.is_empty(), "cell is empty");
        ensure!(bytes.len() <= MAX_FRAME_LEN, "cell exceeds frame limit");
        let mut reader = Reader::new(bytes);
        let cell = match reader.u8()? {
            TAG_RELAY => Cell::Relay {
                payload: reader.bytes()?.to_vec(),
            },
            TAG_EXTEND => Cell::Extend {
                addr: reader.string()?,
                create: reader.bytes()?.to_vec(),
            },
            TAG_BEGIN => Cell::Begin {
                host: reader.string()?,
                port: reader.u16()?,
            },
            TAG_DATA => Cell::Data {
                bytes: reader.bytes()?.to_vec(),
            },
            TAG_END => Cell::End,
            tag => bail!("unknown cell tag {tag}"),
        };
        ensure!(reader.is_finished(), "cell has trailing bytes");
        return Ok(cell);
    }
}

fn put_string(out: &mut Vec<u8>, value: &str) -> Result<()> {
    let len = u16::try_from(value.len()).context("cell string is too long")?;
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(value.as_bytes());
    return Ok(());
}

fn put_bytes(out: &mut Vec<u8>, value: &[u8]) -> Result<()> {
    let len = u32::try_from(value.len()).context("cell byte field is too long")?;
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(value);
    return Ok(());
}

struct Reader<'a> {
    input: &'a [u8],
    offset: usize,
}

impl<'a> Reader<'a> {
    fn new(input: &'a [u8]) -> Self {
        return Self { input, offset: 0 };
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8]> {
        let end = self
            .offset
            .checked_add(len)
            .context("cell field length overflow")?;
        ensure!(end <= self.input.len(), "cell is truncated");
        let value = &self.input[self.offset..end];
        self.offset = end;
        return Ok(value);
    }

    fn u8(&mut self) -> Result<u8> {
        return Ok(self.take(1)?[0]);
    }

    fn u16(&mut self) -> Result<u16> {
        return Ok(u16::from_be_bytes(
            self.take(2)?.try_into().expect("length checked"),
        ));
    }

    fn u32(&mut self) -> Result<u32> {
        return Ok(u32::from_be_bytes(
            self.take(4)?.try_into().expect("length checked"),
        ));
    }

    fn bytes(&mut self) -> Result<&'a [u8]> {
        let len = usize::try_from(self.u32()?).context("cell field length is unsupported")?;
        return self.take(len);
    }

    fn string(&mut self) -> Result<String> {
        let len = usize::from(self.u16()?);
        return String::from_utf8(self.take(len)?.to_vec()).context("cell string is not UTF-8");
    }

    fn is_finished(&self) -> bool {
        return self.offset == self.input.len();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cell_codec_rejects_trailing_bytes() {
        let mut encoded = Cell::End.encode().unwrap();
        encoded.push(0);
        assert!(Cell::decode(&encoded).is_err());
    }

    #[test]
    fn cell_codec_roundtrips_every_variant() {
        let cells = [
            Cell::Relay {
                payload: vec![1, 2, 3],
            },
            Cell::Extend {
                addr: "relay.example:9001".into(),
                create: vec![4, 5, 6],
            },
            Cell::Begin {
                host: "example.com".into(),
                port: 443,
            },
            Cell::Data {
                bytes: vec![7, 8, 9],
            },
            Cell::End,
        ];
        for cell in cells {
            assert_eq!(Cell::decode(&cell.encode().unwrap()).unwrap(), cell);
        }
    }

    #[test]
    fn cell_codec_rejects_truncation_and_unknown_tags() {
        assert!(Cell::decode(&[TAG_DATA, 0, 0, 0, 4, 1]).is_err());
        assert!(Cell::decode(&[255]).is_err());
    }
}
