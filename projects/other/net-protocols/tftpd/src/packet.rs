//! The five TFTP packets and their encoding (RFC 1350 section 5).
//!
//! Pure code over byte slices: no sockets here, so every case is easy to
//! unit test.

/// A DATA packet carries at most this many bytes. A shorter one ends the
/// transfer.
pub const BLOCK_SIZE: usize = 512;

const RRQ: u16 = 1;
const WRQ: u16 = 2;
const DATA: u16 = 3;
const ACK: u16 = 4;
const ERROR: u16 = 5;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Packet {
    /// Read request: the client wants to download `filename`.
    Rrq {
        filename: String,
        mode: String,
    },
    /// Write request: the client wants to upload `filename`.
    Wrq {
        filename: String,
        mode: String,
    },
    Data {
        block: u16,
        data: Vec<u8>,
    },
    Ack {
        block: u16,
    },
    Error {
        code: u16,
        message: String,
    },
}

/// The error codes this server sends (RFC 1350 section 5). The RFC also
/// defines 3 "disk full" and 7 "no such user".
///
/// Giving each variant an explicit value lets `code as u16` turn it into
/// the number that goes on the wire.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorCode {
    NotDefined = 0,
    FileNotFound = 1,
    AccessViolation = 2,
    IllegalOperation = 4,
    UnknownTid = 5,
    FileExists = 6,
}

impl Packet {
    pub fn error(code: ErrorCode, message: &str) -> Packet {
        Packet::Error {
            code: code as u16,
            message: message.to_string(),
        }
    }

    /// Decodes one datagram. Every length is checked before it's used, so
    /// truncated or hostile input gives an `Err`, never a panic.
    pub fn parse(buf: &[u8]) -> Result<Packet, &'static str> {
        // `split_first_chunk::<2>` returns the first two bytes as a `&[u8; 2]`
        // plus the rest, or `None` if the slice is too short. No indexing,
        // so there's nothing that can go out of bounds.
        let (opcode, rest) = buf.split_first_chunk::<2>().ok_or("no opcode")?;
        match u16::from_be_bytes(*opcode) {
            op @ (RRQ | WRQ) => {
                let (filename, rest) = take_string(rest)?;
                // Anything after the mode is RFC 2347 options (macOS tftp
                // sends "tsize" and "rollover"). A server that doesn't
                // support options ignores them, and the client falls back
                // to plain RFC 1350.
                let (mode, _options) = take_string(rest)?;
                Ok(if op == RRQ {
                    Packet::Rrq { filename, mode }
                } else {
                    Packet::Wrq { filename, mode }
                })
            }
            DATA => {
                let (block, data) = rest.split_first_chunk::<2>().ok_or("DATA too short")?;
                if data.len() > BLOCK_SIZE {
                    return Err("DATA longer than 512 bytes");
                }
                Ok(Packet::Data {
                    block: u16::from_be_bytes(*block),
                    data: data.to_vec(),
                })
            }
            ACK => {
                // `try_into` turns a slice into a fixed-size array only if
                // the length is exactly 2.
                let block: [u8; 2] = rest.try_into().map_err(|_| "ACK must be 4 bytes")?;
                Ok(Packet::Ack {
                    block: u16::from_be_bytes(block),
                })
            }
            ERROR => {
                let (code, rest) = rest.split_first_chunk::<2>().ok_or("ERROR too short")?;
                let (message, _) = take_string(rest)?;
                Ok(Packet::Error {
                    code: u16::from_be_bytes(*code),
                    message,
                })
            }
            _ => Err("unknown opcode"),
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        match self {
            Packet::Rrq { filename, mode } => put_request(&mut out, RRQ, filename, mode),
            Packet::Wrq { filename, mode } => put_request(&mut out, WRQ, filename, mode),
            Packet::Data { block, data } => {
                out.extend(DATA.to_be_bytes());
                out.extend(block.to_be_bytes());
                out.extend(data);
            }
            Packet::Ack { block } => {
                out.extend(ACK.to_be_bytes());
                out.extend(block.to_be_bytes());
            }
            Packet::Error { code, message } => {
                out.extend(ERROR.to_be_bytes());
                out.extend(code.to_be_bytes());
                out.extend(message.as_bytes());
                out.push(0);
            }
        }
        out
    }
}

fn put_request(out: &mut Vec<u8>, opcode: u16, filename: &str, mode: &str) {
    out.extend(opcode.to_be_bytes());
    for s in [filename, mode] {
        out.extend(s.as_bytes());
        out.push(0);
    }
}

/// Splits a zero-terminated string off the front of `buf`.
fn take_string(buf: &[u8]) -> Result<(String, &[u8]), &'static str> {
    let end = buf
        .iter()
        .position(|&b| b == 0)
        .ok_or("string is not zero-terminated")?;
    // `end` is the index of a byte we found, so both slices are in bounds.
    let text = std::str::from_utf8(&buf[..end]).map_err(|_| "string is not UTF-8")?;
    Ok((text.to_string(), &buf[end + 1..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What macOS `tftp` really sends for `get hello.txt`, captured with
    /// `nc -u -l`: note the options after the mode.
    const MACOS_RRQ: &[u8] = b"\x00\x01hello.txt\x00octet\x00tsize\x000\x00rollover\x000\x00";

    #[test]
    fn parses_a_real_rrq_and_ignores_options() {
        assert_eq!(
            Packet::parse(MACOS_RRQ),
            Ok(Packet::Rrq {
                filename: "hello.txt".into(),
                mode: "octet".into()
            })
        );
    }

    #[test]
    fn every_packet_round_trips() {
        let packets = [
            Packet::Rrq {
                filename: "a.txt".into(),
                mode: "octet".into(),
            },
            Packet::Wrq {
                filename: "dir/b.bin".into(),
                mode: "netascii".into(),
            },
            Packet::Data {
                block: 65535,
                data: vec![0xAB; BLOCK_SIZE],
            },
            Packet::Data {
                block: 2,
                data: vec![],
            },
            Packet::Ack { block: 0 },
            Packet::error(ErrorCode::FileExists, "File already exists"),
        ];
        for packet in packets {
            assert_eq!(Packet::parse(&packet.encode()), Ok(packet));
        }
    }

    #[test]
    fn encodes_exact_bytes() {
        assert_eq!(Packet::Ack { block: 258 }.encode(), [0, 4, 1, 2]);
        assert_eq!(
            Packet::error(ErrorCode::FileNotFound, "nope").encode(),
            b"\x00\x05\x00\x01nope\x00"
        );
    }

    #[test]
    fn rejects_truncated_packets() {
        for bad in [
            &b""[..],
            b"\x00",
            b"\x00\x01hello.txt",          // filename never terminated
            b"\x00\x01hello.txt\x00octet", // mode never terminated
            b"\x00\x03\x00",               // DATA with half a block number
            b"\x00\x04\x00",               // ACK with half a block number
            b"\x00\x05\x00",               // ERROR with half a code
            b"\x00\x05\x00\x01oops",       // ERROR message never terminated
        ] {
            assert!(Packet::parse(bad).is_err(), "accepted {bad:?}");
        }
    }

    #[test]
    fn rejects_malformed_packets() {
        let mut too_big = vec![0, 3, 0, 1];
        too_big.extend([0u8; BLOCK_SIZE + 1]);
        assert_eq!(Packet::parse(&too_big), Err("DATA longer than 512 bytes"));
        assert_eq!(Packet::parse(b"\x00\x09"), Err("unknown opcode"));
        assert_eq!(
            Packet::parse(b"\x00\x04\x00\x01\x00"),
            Err("ACK must be 4 bytes")
        );
        assert_eq!(
            Packet::parse(b"\x00\x01\xff\xfe\x00octet\x00"),
            Err("string is not UTF-8")
        );
    }
}
