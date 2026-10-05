//! The DNS message format (RFC 1035 section 4): a bounds-checked byte
//! cursor for reading, and parse/encode for the parts of a message this
//! server uses. No sockets here.

use std::net::{Ipv4Addr, Ipv6Addr};

// Record types (RFC 1035 3.2.2; AAAA is RFC 3596).
pub const TYPE_A: u16 = 1;
pub const TYPE_CNAME: u16 = 5;
pub const TYPE_TXT: u16 = 16;
pub const TYPE_AAAA: u16 = 28;
pub const CLASS_IN: u16 = 1;

pub const OPCODE_QUERY: u8 = 0;

// Response codes (RFC 1035 4.1.1).
pub const NOERROR: u8 = 0;
pub const FORMERR: u8 = 1;
pub const SERVFAIL: u8 = 2;
pub const NXDOMAIN: u8 = 3;
pub const NOTIMP: u8 = 4;

/// The longest a name may be on the wire, length bytes included
/// (RFC 1035 2.3.4).
pub const MAX_NAME: usize = 255;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WireError {
    /// The message ends in the middle of something.
    Truncated,
    /// A label we can't accept: a length byte starting with the reserved
    /// bits 01 or 10, or bytes that aren't printable ASCII or contain a dot.
    BadLabel,
    /// A name longer than 255 bytes.
    NameTooLong,
    /// A compression pointer that doesn't point backwards: a possible loop.
    BadPointer,
    /// RDATA of the wrong size for its type, e.g. an A record that isn't
    /// 4 bytes.
    BadRdata,
}

/// The ID and flags from the header. The four section counts aren't stored
/// here: `Message::parse` reads them and `Message::encode` writes the
/// lengths of its vectors, so they can never disagree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Header {
    pub id: u16,
    /// false = query, true = response.
    pub qr: bool,
    pub opcode: u8,
    /// Authoritative answer.
    pub aa: bool,
    /// Truncated: the full reply didn't fit in one datagram.
    pub tc: bool,
    /// Recursion desired (set by the client).
    pub rd: bool,
    /// Recursion available (set by the server).
    pub ra: bool,
    pub rcode: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Question {
    pub name: String,
    pub qtype: u16,
    pub qclass: u16,
}

/// A resource record. The class is always IN, so it isn't stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub name: String,
    pub ttl: u32,
    pub data: RData,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RData {
    A(Ipv4Addr),
    Aaaa(Ipv6Addr),
    Cname(String),
    Txt(String),
    /// Any type we don't decode, kept as raw bytes.
    Other(u16, Vec<u8>),
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Message {
    pub header: Header,
    pub questions: Vec<Question>,
    pub answers: Vec<Record>,
}

/// Reads big-endian numbers and names from a message. It holds the *whole*
/// message, not just the unread part, because compression pointers are
/// offsets from the start of the message.
#[derive(Clone)]
pub struct Cursor<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Cursor { buf, pos: 0 }
    }

    /// The next `n` bytes. `get` returns `None` instead of panicking when the
    /// range runs past the end, so a short message is an error, not a crash.
    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8], WireError> {
        let slice = self
            .buf
            .get(self.pos..self.pos + n)
            .ok_or(WireError::Truncated)?;
        self.pos += n;
        Ok(slice)
    }

    pub fn u16(&mut self) -> Result<u16, WireError> {
        let b = self.bytes(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }

    pub fn u32(&mut self) -> Result<u32, WireError> {
        let b = self.bytes(4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    /// Reads a name: labels, each a length byte and that many bytes, ending
    /// with a zero byte (RFC 1035 3.1). A length byte whose top two bits are
    /// `11` is a compression pointer instead (4.1.4): the other 14 bits are
    /// the offset of the rest of the name, earlier in the message.
    pub fn name(&mut self) -> Result<String, WireError> {
        let mut name = String::new();
        let mut wire_len = 1; // the final zero byte
        let mut pos = self.pos;
        // After a pointer, the cursor continues after the *pointer*, not
        // after wherever the name's tail was found.
        let mut resume = None;
        // RFC 1035 says a pointer refers to a *prior* occurrence of a name.
        // Requiring each jump to land below the previous one means positions
        // only ever go down, so a pointer loop is impossible.
        let mut floor = self.pos;
        loop {
            let len = *self.buf.get(pos).ok_or(WireError::Truncated)?;
            match len >> 6 {
                0b00 if len == 0 => {
                    pos += 1;
                    break;
                }
                // Top bits 00: a label of 1..=63 bytes.
                0b00 => {
                    let len = usize::from(len);
                    let label = self
                        .buf
                        .get(pos + 1..pos + 1 + len)
                        .ok_or(WireError::Truncated)?;
                    wire_len += 1 + len;
                    if wire_len > MAX_NAME {
                        return Err(WireError::NameTooLong);
                    }
                    if !label.iter().all(|&b| b.is_ascii_graphic() && b != b'.') {
                        return Err(WireError::BadLabel);
                    }
                    if !name.is_empty() {
                        name.push('.');
                    }
                    name.extend(label.iter().map(|&b| char::from(b)));
                    pos += 1 + len;
                }
                0b11 => {
                    let low = *self.buf.get(pos + 1).ok_or(WireError::Truncated)?;
                    let target = (usize::from(len & 0x3F) << 8) | usize::from(low);
                    if target >= floor {
                        return Err(WireError::BadPointer);
                    }
                    resume.get_or_insert(pos + 2);
                    floor = target;
                    pos = target;
                }
                // 01 and 10 are reserved.
                _ => return Err(WireError::BadLabel),
            }
        }
        self.pos = resume.unwrap_or(pos);
        Ok(name)
    }
}

impl Header {
    /// Reads the ID and the flags word, pulling the fields out with masks:
    ///
    /// ```text
    ///   bit  15  14..11  10  9   8   7   6..4  3..0
    ///        QR  OPCODE  AA  TC  RD  RA  Z     RCODE
    /// ```
    pub fn read(c: &mut Cursor) -> Result<Header, WireError> {
        let id = c.u16()?;
        let flags = c.u16()?;
        Ok(Header {
            id,
            qr: flags & 0x8000 != 0,
            opcode: ((flags >> 11) & 0xF) as u8,
            aa: flags & 0x0400 != 0,
            tc: flags & 0x0200 != 0,
            rd: flags & 0x0100 != 0,
            ra: flags & 0x0080 != 0,
            rcode: (flags & 0xF) as u8,
        })
    }

    /// The reverse of `read`: shift each field into place and OR them
    /// together. The Z bits are always sent as zero.
    fn flags(&self) -> u16 {
        (u16::from(self.qr) << 15)
            | (u16::from(self.opcode & 0xF) << 11)
            | (u16::from(self.aa) << 10)
            | (u16::from(self.tc) << 9)
            | (u16::from(self.rd) << 8)
            | (u16::from(self.ra) << 7)
            | u16::from(self.rcode & 0xF)
    }
}

impl Question {
    fn read(c: &mut Cursor) -> Result<Question, WireError> {
        let name = c.name()?;
        let qtype = c.u16()?;
        let qclass = c.u16()?;
        Ok(Question {
            name,
            qtype,
            qclass,
        })
    }
}

impl RData {
    pub fn rtype(&self) -> u16 {
        match self {
            RData::A(_) => TYPE_A,
            RData::Aaaa(_) => TYPE_AAAA,
            RData::Cname(_) => TYPE_CNAME,
            RData::Txt(_) => TYPE_TXT,
            RData::Other(rtype, _) => *rtype,
        }
    }
}

impl Record {
    fn read(c: &mut Cursor) -> Result<Record, WireError> {
        let name = c.name()?;
        let rtype = c.u16()?;
        let _class = c.u16()?;
        let ttl = c.u32()?;
        let len = usize::from(c.u16()?);
        // A CNAME target may itself be compressed, so it needs a cursor over
        // the whole message (a copy of this one), not just the RDATA bytes.
        let mut at_rdata = c.clone();
        let rdata = c.bytes(len)?;
        let data = match rtype {
            TYPE_A => RData::A(fixed::<4>(rdata)?.into()),
            TYPE_AAAA => RData::Aaaa(fixed::<16>(rdata)?.into()),
            TYPE_CNAME => RData::Cname(at_rdata.name()?),
            TYPE_TXT => RData::Txt(read_txt(rdata)?),
            other => RData::Other(other, rdata.to_vec()),
        };
        Ok(Record { name, ttl, data })
    }
}

/// Turns a slice into an array of exactly `N` bytes, or `BadRdata`. `N` is a
/// const generic: one function for both the 4-byte and the 16-byte case.
fn fixed<const N: usize>(bytes: &[u8]) -> Result<[u8; N], WireError> {
    bytes.try_into().map_err(|_| WireError::BadRdata)
}

/// TXT RDATA is one or more strings, each a length byte followed by that
/// many bytes (RFC 1035 3.3.14). We join them into one.
fn read_txt(mut rdata: &[u8]) -> Result<String, WireError> {
    let mut text = Vec::new();
    while let Some((&len, rest)) = rdata.split_first() {
        let (chunk, rest) = rest
            .split_at_checked(usize::from(len))
            .ok_or(WireError::Truncated)?;
        text.extend(chunk);
        rdata = rest;
    }
    Ok(String::from_utf8_lossy(&text).into_owned())
}

impl Message {
    /// Parses the header, questions and answers. The authority and
    /// additional sections are never read. That's where dig puts its EDNS
    /// "OPT" record (RFC 6891); ignoring it means we answer without EDNS,
    /// which every client accepts.
    pub fn parse(buf: &[u8]) -> Result<Message, WireError> {
        let mut c = Cursor::new(buf);
        let header = Header::read(&mut c)?;
        let qdcount = c.u16()?;
        let ancount = c.u16()?;
        let _nscount = c.u16()?;
        let _arcount = c.u16()?;
        // `collect` can gather an iterator of `Result`s into a
        // `Result<Vec<_>, _>`, stopping at the first error.
        let questions = (0..qdcount)
            .map(|_| Question::read(&mut c))
            .collect::<Result<_, _>>()?;
        let answers = (0..ancount)
            .map(|_| Record::read(&mut c))
            .collect::<Result<_, _>>()?;
        Ok(Message {
            header,
            questions,
            answers,
        })
    }

    /// Encodes the message. Names in the answers that equal the question
    /// name are written as the pointer `C0 0C`. Names must already be valid
    /// (the wire parser and the zone loader both check them).
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(512);
        out.extend(self.header.id.to_be_bytes());
        out.extend(self.header.flags().to_be_bytes());
        // QDCOUNT, ANCOUNT, NSCOUNT, ARCOUNT.
        for count in [self.questions.len(), self.answers.len(), 0, 0] {
            out.extend((count as u16).to_be_bytes());
        }

        for q in &self.questions {
            put_name(&mut out, &q.name, None);
            out.extend(q.qtype.to_be_bytes());
            out.extend(q.qclass.to_be_bytes());
        }

        let qname = self.questions.first().map(|q| q.name.as_str());
        for r in &self.answers {
            put_name(&mut out, &r.name, qname);
            out.extend(r.data.rtype().to_be_bytes());
            out.extend(CLASS_IN.to_be_bytes());
            out.extend(r.ttl.to_be_bytes());
            // RDLENGTH comes first, but a compressed CNAME target's length
            // isn't known until it's written. Reserve two bytes, fill them in
            // afterwards.
            let len_at = out.len();
            out.extend([0, 0]);
            match &r.data {
                RData::A(ip) => out.extend(ip.octets()),
                RData::Aaaa(ip) => out.extend(ip.octets()),
                RData::Cname(target) => put_name(&mut out, target, qname),
                RData::Txt(text) => {
                    if text.is_empty() {
                        out.push(0);
                    }
                    for chunk in text.as_bytes().chunks(255) {
                        out.push(chunk.len() as u8);
                        out.extend(chunk);
                    }
                }
                RData::Other(_, bytes) => out.extend(bytes),
            }
            let len = (out.len() - len_at - 2) as u16;
            out[len_at..len_at + 2].copy_from_slice(&len.to_be_bytes());
        }
        out
    }
}

/// Writes `name` as labels, or as a pointer to the question name (which
/// always starts at offset 12, right after the header) if it's the same.
fn put_name(out: &mut Vec<u8>, name: &str, question: Option<&str>) {
    if question.is_some_and(|q| q.eq_ignore_ascii_case(name)) {
        out.extend([0xC0, 12]);
        return;
    }
    for label in name.split('.').filter(|l| !l.is_empty()) {
        out.push(label.len() as u8);
        out.extend(label.as_bytes());
    }
    out.push(0);
}

pub fn type_name(rtype: u16) -> String {
    match rtype {
        TYPE_A => "A".into(),
        TYPE_AAAA => "AAAA".into(),
        TYPE_CNAME => "CNAME".into(),
        TYPE_TXT => "TXT".into(),
        // The generic notation for unknown types (RFC 3597).
        other => format!("TYPE{other}"),
    }
}

pub fn rcode_name(rcode: u8) -> &'static str {
    match rcode {
        NOERROR => "NOERROR",
        FORMERR => "FORMERR",
        SERVFAIL => "SERVFAIL",
        NXDOMAIN => "NXDOMAIN",
        NOTIMP => "NOTIMP",
        _ => "RCODE?",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real query from `dig @127.0.0.1 -p 10054 hello.test`, captured with
    /// `nc -u -l 10054 > query.bin`. ARCOUNT is 1: dig's EDNS OPT record.
    const DIG_QUERY: [u8; 39] = [
        0xd9, 0xba, 0x01, 0x20, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, // header
        0x05, b'h', b'e', b'l', b'l', b'o', 0x04, b't', b'e', b's', b't', 0x00, // name
        0x00, 0x01, 0x00, 0x01, // QTYPE A, QCLASS IN
        0x00, 0x00, 0x29, 0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // OPT
    ];

    fn header(id: u16, flags: [u8; 2], qd: u8, an: u8) -> Vec<u8> {
        let [hi, lo] = id.to_be_bytes();
        vec![hi, lo, flags[0], flags[1], 0, qd, 0, an, 0, 0, 0, 0]
    }

    #[test]
    fn parses_a_real_dig_query() {
        let msg = Message::parse(&DIG_QUERY).unwrap();
        assert_eq!(msg.header.id, 0xd9ba);
        assert!(!msg.header.qr && msg.header.rd);
        assert_eq!(msg.header.opcode, OPCODE_QUERY);
        assert_eq!(
            msg.questions,
            [Question {
                name: "hello.test".into(),
                qtype: TYPE_A,
                qclass: CLASS_IN
            }]
        );
        assert!(msg.answers.is_empty());
    }

    #[test]
    fn every_truncation_of_the_query_is_an_error() {
        // Header (12) + question (16) = 28 bytes; the OPT record after that
        // is never read.
        for n in 0..28 {
            assert!(Message::parse(&DIG_QUERY[..n]).is_err(), "{n} bytes");
        }
    }

    #[test]
    fn flags_round_trip_through_the_bit_masks() {
        let h = Header {
            id: 7,
            qr: true,
            opcode: 2,
            aa: true,
            tc: false,
            rd: true,
            ra: false,
            rcode: NXDOMAIN,
        };
        // QR=1 OPCODE=0010 AA=1 TC=0 | RD=1 RA=0 Z=000 RCODE=0011
        assert_eq!(h.flags(), 0b1001_0101_0000_0011);
        let msg = Message {
            header: h,
            ..Message::default()
        };
        assert_eq!(Message::parse(&msg.encode()).unwrap().header, h);
    }

    #[test]
    fn follows_compression_pointers() {
        // Question hello.test (offset 12), then a CNAME record whose owner is
        // `C0 0C` and whose target is "www" + a pointer back to offset 12.
        let mut msg = header(1, [0x84, 0], 1, 1);
        msg.extend(b"\x05hello\x04test\x00\x00\x05\x00\x01");
        msg.extend(b"\xc0\x0c\x00\x05\x00\x01\x00\x00\x00\x3c\x00\x06\x03www\xc0\x0c");
        let parsed = Message::parse(&msg).unwrap();
        assert_eq!(parsed.answers[0].name, "hello.test");
        assert_eq!(
            parsed.answers[0].data,
            RData::Cname("www.hello.test".into())
        );
    }

    #[test]
    fn rejects_pointer_loops() {
        let mut to_itself = header(1, [0, 0], 1, 0);
        to_itself.extend(b"\xc0\x0c\x00\x01\x00\x01");
        assert_eq!(Message::parse(&to_itself), Err(WireError::BadPointer));

        // A label, then a pointer back to the start of the same name.
        let mut cycle = header(1, [0, 0], 1, 0);
        cycle.extend(b"\x01a\xc0\x0c\x00\x01\x00\x01");
        assert_eq!(Message::parse(&cycle), Err(WireError::BadPointer));
    }

    #[test]
    fn rejects_bad_labels_and_long_names() {
        for bad in [&b"\x40"[..], b"\x80", b"\x03a.b\x00", b"\x03a b\x00"] {
            assert_eq!(Cursor::new(bad).name(), Err(WireError::BadLabel));
        }
        // Five 63-byte labels: 5 * 64 + 1 = 321 bytes, over the limit.
        let mut long = Vec::new();
        for _ in 0..5 {
            long.push(63);
            long.extend([b'x'; 63]);
        }
        long.push(0);
        assert_eq!(Cursor::new(&long).name(), Err(WireError::NameTooLong));
    }

    #[test]
    fn a_response_round_trips_and_points_at_the_question() {
        let record = |name: &str, data| Record {
            name: name.into(),
            ttl: 300,
            data,
        };
        let msg = Message {
            header: Header {
                id: 42,
                qr: true,
                aa: true,
                ..Header::default()
            },
            questions: vec![Question {
                name: "WWW.hello.test".into(),
                qtype: TYPE_A,
                qclass: CLASS_IN,
            }],
            answers: vec![
                record("WWW.hello.test", RData::Cname("hello.test".into())),
                record("hello.test", RData::A(Ipv4Addr::new(192, 0, 2, 1))),
                record("hello.test", RData::Aaaa("2001:db8::1".parse().unwrap())),
                record("hello.test", RData::Txt("x".repeat(300))),
            ],
        };
        let bytes = msg.encode();
        // 12 header bytes + a 16-byte name + QTYPE and QCLASS: the first
        // answer starts at offset 32.
        assert_eq!(bytes[32..34], [0xC0, 0x0C]);
        assert_eq!(Message::parse(&bytes).unwrap(), msg);
    }
}
