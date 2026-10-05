//! The frame codec (RFC 6455 §5) and message reassembly (§5.4).
//!
//! Everything here works over `impl Read` / `impl Write`, so the tests feed
//! it byte slices and `Vec<u8>` instead of sockets.

use std::io::{self, Read, Write};

// Close status codes we send (RFC 6455 §7.4.1).
pub const CLOSE_NORMAL: u16 = 1000;
pub const CLOSE_PROTOCOL_ERROR: u16 = 1002;
pub const CLOSE_INVALID_DATA: u16 = 1007;
pub const CLOSE_TOO_BIG: u16 = 1009;
/// Never sent. Reported when a close frame had no code, as a browser's
/// `CloseEvent.code` does.
pub const CLOSE_NO_STATUS: u16 = 1005;

/// The 4-bit frame type. `#[repr(u8)]` with explicit values lets us write
/// `opcode as u8` to get the wire bits back.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Opcode {
    Continuation = 0x0,
    Text = 0x1,
    Binary = 0x2,
    Close = 0x8,
    Ping = 0x9,
    Pong = 0xA,
}

impl Opcode {
    fn from_bits(bits: u8) -> Option<Opcode> {
        Some(match bits {
            0x0 => Opcode::Continuation,
            0x1 => Opcode::Text,
            0x2 => Opcode::Binary,
            0x8 => Opcode::Close,
            0x9 => Opcode::Ping,
            0xA => Opcode::Pong,
            _ => return None, // 0x3-0x7 and 0xB-0xF are reserved
        })
    }

    /// Control opcodes all have the high bit set (0x8-0xF).
    pub fn is_control(self) -> bool {
        self as u8 & 0x8 != 0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub fin: bool,
    pub opcode: Opcode,
    pub payload: Vec<u8>,
}

#[derive(Debug)]
pub enum WsError {
    /// The socket failed or closed. Nobody is left to tell.
    Io(io::Error),
    /// The peer broke the rules. We close with this status code and reason.
    Protocol(u16, &'static str),
}

impl From<io::Error> for WsError {
    fn from(e: io::Error) -> Self {
        WsError::Io(e)
    }
}

/// Shorthand for the most common error: a protocol violation (close 1002).
fn violation(why: &'static str) -> WsError {
    WsError::Protocol(CLOSE_PROTOCOL_ERROR, why)
}

/// XOR with the 4-byte key, cycling through it. XOR is its own inverse, so
/// the same function masks and unmasks.
pub fn apply_mask(data: &mut [u8], key: [u8; 4]) {
    for (i, byte) in data.iter_mut().enumerate() {
        *byte ^= key[i % 4];
    }
}

impl Frame {
    /// `impl Into<Vec<u8>>` accepts a `Vec<u8>`, `&[u8]`, `&str`, or `b"..."`.
    pub fn new(fin: bool, opcode: Opcode, payload: impl Into<Vec<u8>>) -> Frame {
        Frame {
            fin,
            opcode,
            payload: payload.into(),
        }
    }

    pub fn text(text: &str) -> Frame {
        Frame::new(true, Opcode::Text, text)
    }

    /// A close frame's payload is a big-endian status code plus an optional
    /// UTF-8 reason (§5.5.1).
    pub fn close(code: u16, reason: &str) -> Frame {
        let mut payload = code.to_be_bytes().to_vec();
        payload.extend_from_slice(reason.as_bytes());
        Frame::new(true, Opcode::Close, payload)
    }

    /// Reads one frame. `expect_masked` is true on the server, because clients
    /// MUST mask, and false on the client, because servers MUST NOT (§5.1).
    /// A declared length over `max_payload` is refused before we allocate, so
    /// a peer can't make us reserve 2^63 bytes by sending a 10-byte header.
    pub fn read_from(
        r: &mut impl Read,
        expect_masked: bool,
        max_payload: usize,
    ) -> Result<Frame, WsError> {
        let mut head = [0u8; 2];
        r.read_exact(&mut head)?;
        let fin = head[0] & 0x80 != 0;
        // RSV1-3 are for extensions, and we negotiated none.
        if head[0] & 0x70 != 0 {
            return Err(violation("reserved bits set"));
        }
        let opcode = Opcode::from_bits(head[0] & 0x0F).ok_or(violation("unknown opcode"))?;
        let masked = head[1] & 0x80 != 0;
        if masked != expect_masked {
            let why = if expect_masked {
                "client frames must be masked"
            } else {
                "server frames must not be masked"
            };
            return Err(violation(why));
        }
        // 7-bit length, or 126/127 meaning "the real length follows in 16/64 bits".
        let len = match head[1] & 0x7F {
            126 => {
                let mut bytes = [0u8; 2];
                r.read_exact(&mut bytes)?;
                u16::from_be_bytes(bytes) as u64
            }
            127 => {
                let mut bytes = [0u8; 8];
                r.read_exact(&mut bytes)?;
                u64::from_be_bytes(bytes)
            }
            short => short as u64,
        };
        // Control frames must fit in one small frame (§5.5).
        if opcode.is_control() && (!fin || len > 125) {
            return Err(violation("bad control frame"));
        }
        if len > max_payload as u64 {
            return Err(WsError::Protocol(CLOSE_TOO_BIG, "message too big"));
        }
        let mut key = [0u8; 4];
        if masked {
            r.read_exact(&mut key)?;
        }
        let mut payload = vec![0; len as usize]; // safe: len <= max_payload
        r.read_exact(&mut payload)?;
        if masked {
            apply_mask(&mut payload, key);
        }
        Ok(Frame::new(fin, opcode, payload))
    }

    /// Writes the frame. Clients pass a fresh random key for every frame;
    /// servers pass `None`. Header and payload go out in one `write_all`, so
    /// a frame never reaches the wire as a header without its payload.
    pub fn write_to(&self, w: &mut impl Write, mask: Option<[u8; 4]>) -> io::Result<()> {
        let mut out = Vec::with_capacity(14 + self.payload.len());
        let fin_bit = if self.fin { 0x80 } else { 0 };
        out.push(fin_bit | self.opcode as u8);
        let mask_bit = if mask.is_some() { 0x80 } else { 0 };
        let len = self.payload.len();
        if len < 126 {
            out.push(mask_bit | len as u8);
        } else if len <= 0xFFFF {
            out.push(mask_bit | 126);
            out.extend_from_slice(&(len as u16).to_be_bytes());
        } else {
            out.push(mask_bit | 127);
            out.extend_from_slice(&(len as u64).to_be_bytes());
        }
        let payload_start = out.len() + if mask.is_some() { 4 } else { 0 };
        if let Some(key) = mask {
            out.extend_from_slice(&key);
        }
        out.extend_from_slice(&self.payload);
        if let Some(key) = mask {
            apply_mask(&mut out[payload_start..], key);
        }
        w.write_all(&out)
    }
}

/// A complete, validated message: what the application actually cares about.
#[derive(Debug, PartialEq)]
pub enum Message {
    Text(String),
    Binary(Vec<u8>),
    Ping(Vec<u8>),
    Pong(Vec<u8>),
    /// The status code (if any) and reason from a close frame.
    Close(Option<u16>, String),
}

/// Turns frames into messages. A text or binary message may be split into
/// fragments: a first frame with FIN=0, then Continuation frames, the last
/// one with FIN=1. Control frames may arrive between fragments (§5.4). The
/// only state is the message in progress, so this is a two-state machine:
/// idle (`None`) or collecting (`Some`).
pub struct Assembler {
    partial: Option<(Opcode, Vec<u8>)>,
    max_message: usize,
}

impl Assembler {
    pub fn new(max_message: usize) -> Self {
        Assembler {
            partial: None,
            max_message,
        }
    }

    /// Feeds one frame. Returns `Ok(None)` while a fragmented message is
    /// still incomplete.
    pub fn push(&mut self, frame: Frame) -> Result<Option<Message>, WsError> {
        match (frame.opcode, &self.partial) {
            (Opcode::Ping, _) => return Ok(Some(Message::Ping(frame.payload))),
            (Opcode::Pong, _) => return Ok(Some(Message::Pong(frame.payload))),
            (Opcode::Close, _) => return parse_close(&frame.payload).map(Some),
            (Opcode::Text | Opcode::Binary, Some(_)) => {
                return Err(violation("new message inside a fragmented one"));
            }
            (Opcode::Continuation, None) => {
                return Err(violation("continuation with nothing to continue"));
            }
            (Opcode::Text | Opcode::Binary, None) => {
                self.partial = Some((frame.opcode, Vec::new()))
            }
            (Opcode::Continuation, Some(_)) => {}
        }

        let (_, data) = self.partial.as_mut().expect("partial was set above");
        // Each frame is under the limit, but many fragments could add up.
        if data.len() + frame.payload.len() > self.max_message {
            return Err(WsError::Protocol(CLOSE_TOO_BIG, "message too big"));
        }
        data.extend_from_slice(&frame.payload);
        if !frame.fin {
            return Ok(None);
        }

        let (opcode, data) = self.partial.take().expect("partial was set above");
        if opcode == Opcode::Binary {
            return Ok(Some(Message::Binary(data)));
        }
        // Validate UTF-8 on the whole message: a multi-byte character may be
        // split across two fragments.
        match String::from_utf8(data) {
            Ok(text) => Ok(Some(Message::Text(text))),
            Err(_) => Err(WsError::Protocol(
                CLOSE_INVALID_DATA,
                "text is not valid UTF-8",
            )),
        }
    }
}

/// A close payload is empty, or a 2-byte code plus a UTF-8 reason (§5.5.1).
fn parse_close(payload: &[u8]) -> Result<Message, WsError> {
    match payload {
        [] => Ok(Message::Close(None, String::new())),
        [_] => Err(violation("one-byte close payload")),
        [high, low, reason @ ..] => {
            let code = u16::from_be_bytes([*high, *low]);
            // 1004-1006 and 1015 are reserved and must never be on the wire
            // (§7.4.1). 3000-4999 are for libraries and applications.
            if !matches!(code, 1000..=1003 | 1007..=1011 | 3000..=4999) {
                return Err(violation("invalid close code"));
            }
            let reason = String::from_utf8(reason.to_vec())
                .map_err(|_| WsError::Protocol(CLOSE_INVALID_DATA, "close reason is not UTF-8"))?;
            Ok(Message::Close(Some(code), reason))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAX: usize = 64 * 1024;
    /// The masking key used by every example in RFC 6455 §5.7.
    const RFC_KEY: [u8; 4] = [0x37, 0xfa, 0x21, 0x3d];
    /// Everything after the opcode byte in the RFC's masked "Hello" frames:
    /// MASK bit + length 5, the key, then the masked payload.
    const MASKED_HELLO: [u8; 10] = [0x85, 0x37, 0xfa, 0x21, 0x3d, 0x7f, 0x9f, 0x4d, 0x51, 0x58];

    fn read(bytes: &[u8], expect_masked: bool) -> Result<Frame, WsError> {
        Frame::read_from(&mut &bytes[..], expect_masked, MAX)
    }

    fn encode(frame: &Frame, mask: Option<[u8; 4]>) -> Vec<u8> {
        let mut out = Vec::new();
        frame.write_to(&mut out, mask).unwrap();
        out
    }

    fn close_code(result: Result<impl std::fmt::Debug, WsError>) -> u16 {
        match result {
            Err(WsError::Protocol(code, _)) => code,
            other => panic!("expected a protocol error, got {other:?}"),
        }
    }

    #[test]
    fn rfc_examples_single_frames() {
        // Unmasked text, as a server sends it.
        assert_eq!(read(b"\x81\x05Hello", false).unwrap(), Frame::text("Hello"));
        assert_eq!(encode(&Frame::text("Hello"), None), b"\x81\x05Hello");
        // Masked text, as a client sends it.
        let wire = [&[0x81], &MASKED_HELLO[..]].concat();
        assert_eq!(read(&wire, true).unwrap(), Frame::text("Hello"));
        assert_eq!(encode(&Frame::text("Hello"), Some(RFC_KEY)), wire);
        // Unmasked ping, and a masked pong.
        let ping = Frame::new(true, Opcode::Ping, "Hello");
        assert_eq!(read(b"\x89\x05Hello", false).unwrap(), ping);
        let pong = Frame::new(true, Opcode::Pong, "Hello");
        assert_eq!(
            encode(&pong, Some(RFC_KEY)),
            [&[0x8a], &MASKED_HELLO[..]].concat()
        );
    }

    #[test]
    fn rfc_example_fragmented_text_is_reassembled() {
        let mut wire = &b"\x01\x03Hel\x80\x02lo"[..];
        let mut assembler = Assembler::new(MAX);
        let first = Frame::read_from(&mut wire, false, MAX).unwrap();
        assert_eq!(assembler.push(first).unwrap(), None);
        let last = Frame::read_from(&mut wire, false, MAX).unwrap();
        assert_eq!(
            assembler.push(last).unwrap(),
            Some(Message::Text("Hello".into()))
        );
    }

    #[test]
    fn all_three_length_encodings_round_trip() {
        // (payload size, expected second byte, header length)
        for (size, len_byte, header_len) in [(125, 125, 2), (256, 126, 4), (65536, 127, 10)] {
            let frame = Frame::new(true, Opcode::Binary, vec![7; size]);
            let wire = encode(&frame, None);
            assert_eq!(wire[1], len_byte);
            assert_eq!(wire.len(), header_len + size);
            assert_eq!(read(&wire, false).unwrap(), frame);
            // Masked, as a client would send it: four more header bytes.
            let wire = encode(&frame, Some(RFC_KEY));
            assert_eq!(wire.len(), header_len + 4 + size);
            assert_eq!(read(&wire, true).unwrap(), frame);
        }
        // The RFC's 256-byte example header: 0x82 0x7E 0x0100.
        let frame = Frame::new(true, Opcode::Binary, vec![0; 256]);
        assert_eq!(encode(&frame, None)[..4], [0x82, 0x7E, 0x01, 0x00]);
    }

    #[test]
    fn partial_and_truncated_frames() {
        /// A reader that hands out one byte per call, like a slow network.
        struct Trickle<'a>(&'a [u8]);
        impl Read for Trickle<'_> {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                let Some((first, rest)) = self.0.split_first() else {
                    return Ok(0);
                };
                buf[0] = *first;
                self.0 = rest;
                Ok(1)
            }
        }
        let wire = encode(&Frame::text("partial frames are fine"), Some(RFC_KEY));
        let frame = Frame::read_from(&mut Trickle(&wire), true, MAX).unwrap();
        assert_eq!(frame, Frame::text("partial frames are fine"));
        // A frame cut short anywhere is an I/O error (UnexpectedEof).
        for cut in [0, 1, 2, 5, wire.len() - 1] {
            assert!(
                matches!(read(&wire[..cut], true), Err(WsError::Io(_))),
                "cut at {cut}"
            );
        }
    }

    #[test]
    fn protocol_violations_map_to_close_codes() {
        let masked = encode(&Frame::text("Hello"), Some(RFC_KEY));
        assert_eq!(close_code(read(b"\x81\x05Hello", true)), 1002); // unmasked from client
        assert_eq!(close_code(read(&masked, false)), 1002); // masked from server
        assert_eq!(close_code(read(&[0xC1, 0x00], false)), 1002); // RSV1 set
        assert_eq!(close_code(read(&[0x83, 0x00], false)), 1002); // opcode 0x3 is reserved
        assert_eq!(close_code(read(&[0x09, 0x00], false)), 1002); // fragmented ping
        assert_eq!(close_code(read(&[0x89, 0x7E, 0x00, 0x7E], false)), 1002); // 126-byte ping
        // Claims 2^62 bytes: refused from the header alone, nothing allocated.
        let huge = [0x82, 0x7F, 0x40, 0, 0, 0, 0, 0, 0, 0];
        assert_eq!(close_code(read(&huge, false)), 1009);
    }

    #[test]
    fn assembler_handles_interleaved_control_frames_and_split_utf8() {
        let euro = "€".as_bytes(); // 3 bytes, split across two fragments
        let mut assembler = Assembler::new(MAX);
        let start = Frame::new(false, Opcode::Text, &euro[..1]);
        assert_eq!(assembler.push(start).unwrap(), None);
        let ping = assembler
            .push(Frame::new(true, Opcode::Ping, "hi"))
            .unwrap();
        assert_eq!(ping, Some(Message::Ping(b"hi".to_vec())));
        let end = Frame::new(true, Opcode::Continuation, &euro[1..]);
        assert_eq!(
            assembler.push(end).unwrap(),
            Some(Message::Text("€".into()))
        );
    }

    #[test]
    fn assembler_rejects_bad_sequences_and_data() {
        let start = || Frame::new(false, Opcode::Text, "x");
        let cont = |fin| Frame::new(fin, Opcode::Continuation, "x");
        assert_eq!(close_code(Assembler::new(MAX).push(cont(true))), 1002);

        let mut assembler = Assembler::new(MAX);
        assembler.push(start()).unwrap();
        assert_eq!(close_code(assembler.push(start())), 1002);

        let bad_utf8 = Frame::new(true, Opcode::Text, [0xC3, 0x28]);
        assert_eq!(close_code(Assembler::new(MAX).push(bad_utf8)), 1007);

        let mut small = Assembler::new(2);
        small.push(start()).unwrap();
        small.push(cont(false)).unwrap();
        assert_eq!(close_code(small.push(cont(true))), 1009);
    }

    #[test]
    fn close_payloads_are_validated() {
        let mut assembler = Assembler::new(MAX);
        let mut close = |payload: &[u8]| assembler.push(Frame::new(true, Opcode::Close, payload));
        assert_eq!(
            close(&[]).unwrap(),
            Some(Message::Close(None, String::new()))
        );
        let bye = Frame::close(1000, "bye").payload;
        assert_eq!(
            close(&bye).unwrap(),
            Some(Message::Close(Some(1000), "bye".into()))
        );
        assert_eq!(close_code(close(&[0x03])), 1002);
        assert_eq!(close_code(close(&1005u16.to_be_bytes())), 1002);
        assert_eq!(close_code(close(&[0x03, 0xE8, 0xFF])), 1007);
    }
}
