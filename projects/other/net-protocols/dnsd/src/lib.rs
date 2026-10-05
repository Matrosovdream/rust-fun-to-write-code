//! dnsd — an authoritative DNS server for a handful of names (RFC 1035).
//!
//! DNS turns names into data: "what is the IPv4 address of hello.test?". A
//! client sends one UDP datagram holding a question and gets one datagram
//! back holding the answers. This server is *authoritative*: it answers
//! only from its own zone file and never asks other servers (no recursion).
//!
//! # Message layout (RFC 1035 section 4.1)
//!
//! Queries and responses have the same shape: a 12-byte header, then four
//! sections whose sizes the header counts. This server uses the first two;
//! dig puts an EDNS "OPT" record in Additional, which we skip.
//!
//! ```text
//!   | Header | Question | Answer | Authority | Additional |
//! ```
//!
//! ## Header (4.1.1)
//!
//! ```text
//!                                   1  1  1  1  1  1
//!     0  1  2  3  4  5  6  7  8  9  0  1  2  3  4  5
//!   +--+--+--+--+--+--+--+--+--+--+--+--+--+--+--+--+
//!   |                      ID                       |  copied into the reply
//!   +--+--+--+--+--+--+--+--+--+--+--+--+--+--+--+--+
//!   |QR|   OPCODE  |AA|TC|RD|RA|   Z    |   RCODE   |
//!   +--+--+--+--+--+--+--+--+--+--+--+--+--+--+--+--+
//!   |                    QDCOUNT                    |  questions
//!   |                    ANCOUNT                    |  answers
//!   |                    NSCOUNT                    |  authority records
//!   |                    ARCOUNT                    |  additional records
//!   +--+--+--+--+--+--+--+--+--+--+--+--+--+--+--+--+
//!
//!   QR      0 = query, 1 = response
//!   OPCODE  0 = standard query; anything else gets NOTIMP
//!   AA      authoritative answer (we always are)
//!   TC      truncated: the reply didn't fit in 512 bytes
//!   RD, RA  recursion desired (copied from the query) / available (never)
//!   RCODE   0 NOERROR, 1 FORMERR, 2 SERVFAIL, 3 NXDOMAIN, 4 NOTIMP
//! ```
//!
//! ## Names (3.1) and compression (4.1.4)
//!
//! A name is a list of labels, each a length byte (at most 63) and that
//! many bytes, ending with a zero byte, 255 bytes at most in all:
//!
//! ```text
//!   05 h e l l o 04 t e s t 00        hello.test
//! ```
//!
//! To save space, a name or the tail of one can be replaced by a 2-byte
//! pointer to an earlier copy in the message: the top two bits are `11`,
//! the other 14 are an offset from the start of the message. The question
//! name always starts at offset 12, so `C0 0C` means "the question name".
//!
//! ## Question (4.1.2) and resource record (4.1.3)
//!
//! ```text
//!   Question:  NAME | QTYPE (2) | QCLASS (2)
//!   Record:    NAME | TYPE (2) | CLASS (2) | TTL (4) | RDLENGTH (2) | RDATA
//!
//!   TYPE   A = 1      RDATA is a 4-byte IPv4 address
//!          CNAME = 5  RDATA is a name: "this is an alias, look up that one"
//!          TXT = 16   RDATA is strings, each a length byte + up to 255 bytes
//!          AAAA = 28  RDATA is a 16-byte IPv6 address (RFC 3596)
//!   CLASS  always IN = 1 (the Internet)
//! ```
//!
//! # Example exchange
//!
//! `dig @127.0.0.1 -p 10053 hello.test` sends 39 bytes:
//!
//! ```text
//!   d9 ba  01 20  00 01  00 00  00 00  00 01   ID, flags RD (+ the DNSSEC AD bit), 1 question, 1 additional
//!   05 68 65 6c 6c 6f 04 74 65 73 74 00       hello.test
//!   00 01  00 01                               QTYPE A, QCLASS IN
//!   00  00 29  10 00  00 00 00 00  00 00       EDNS OPT record (ignored)
//! ```
//!
//! and dnsd replies with 60 bytes:
//!
//! ```text
//!   d9 ba  85 00  00 01  00 02  00 00  00 00   same ID, flags QR AA RD, 1 question, 2 answers
//!   05 68 65 6c 6c 6f 04 74 65 73 74 00       the question, echoed back
//!   00 01  00 01
//!   c0 0c  00 01  00 01  00 00 01 2c  00 04  c0 00 02 01   hello.test A, TTL 300, 192.0.2.1
//!   c0 0c  00 01  00 01  00 00 01 2c  00 04  c0 00 02 02   hello.test A, TTL 300, 192.0.2.2
//! ```
//!
//! # Response codes
//!
//! NXDOMAIN if the name doesn't exist. NOERROR with zero answers ("NODATA")
//! if it exists but has no records of the asked type. FORMERR for messages
//! that don't parse. NOTIMP for opcodes other than QUERY and classes other
//! than IN. Asking for A on an alias returns the CNAME record followed by
//! the target's A records (RFC 1034 4.3.2).

pub mod wire;
pub mod zone;

pub use wire::{Header, Message, Question, RData, Record};
pub use zone::Zone;

use std::io;
use std::net::UdpSocket;

use wire::{CLASS_IN, Cursor, FORMERR, NOTIMP, OPCODE_QUERY, rcode_name, type_name};

/// Plain DNS over UDP carries at most 512 bytes (RFC 1035 4.2.1). Clients
/// can raise the limit with EDNS, which this server doesn't speak.
pub const MAX_UDP: usize = 512;

/// Builds the reply to one query datagram, or `None` if it must not be
/// answered at all. Pure: bytes in, message out.
pub fn respond(query: &[u8], zone: &Zone) -> Option<Message> {
    // With fewer than 4 bytes there's no ID to reply to.
    let header = Header::read(&mut Cursor::new(query)).ok()?;
    // Never answer a response. That's how two servers end up in a loop,
    // and how attackers bounce traffic off a server.
    if header.qr {
        return None;
    }

    // `..Header::default()` fills in every field we don't name: struct
    // update syntax.
    let mut reply = Message {
        header: Header {
            id: header.id,
            qr: true,
            opcode: header.opcode,
            rd: header.rd,
            ..Header::default()
        },
        ..Message::default()
    };
    if header.opcode != OPCODE_QUERY {
        reply.header.rcode = NOTIMP;
        return Some(reply);
    }

    // RFC 1035 allows several questions per message, but in practice there
    // is always exactly one (RFC 9619), so anything else is FORMERR.
    let question = match Message::parse(query) {
        Ok(mut msg) if msg.questions.len() == 1 => msg.questions.pop(),
        _ => None,
    };
    let Some(question) = question else {
        reply.header.rcode = FORMERR;
        return Some(reply);
    };

    if question.qclass == CLASS_IN {
        let (rcode, answers) = zone.lookup(&question.name, question.qtype);
        reply.header.aa = true;
        reply.header.rcode = rcode;
        reply.answers = answers;
    } else {
        reply.header.rcode = NOTIMP;
    }
    // The question is echoed back with its original letter case. Some
    // resolvers randomize the case and check that it comes back unchanged.
    reply.questions.push(question);
    Some(reply)
}

/// Encodes a reply for UDP. If it's over 512 bytes, the answers are dropped
/// and TC is set, telling the client to ask again over TCP (RFC 1035
/// 4.2.1). Header + question is at most 12 + 255 + 4 bytes, so that fits.
pub fn encode_udp(mut reply: Message) -> Vec<u8> {
    let bytes = reply.encode();
    if bytes.len() <= MAX_UDP {
        return bytes;
    }
    reply.answers.clear();
    reply.header.tc = true;
    reply.encode()
}

/// Answers queries on an already-bound socket, one at a time, logging one
/// line per query. Only returns if the socket itself fails.
pub fn serve(socket: UdpSocket, zone: &Zone) -> io::Result<()> {
    // Real queries are well under 100 bytes. A longer datagram is cut to
    // this size and then usually fails to parse.
    let mut buf = [0u8; 1024];
    loop {
        let (n, peer) = socket.recv_from(&mut buf)?;
        let Some(reply) = respond(&buf[..n], zone) else {
            eprintln!("[{peer}] ignored a {n}-byte datagram");
            continue;
        };
        let asked = match reply.questions.first() {
            Some(q) => format!("{} {}", q.name, type_name(q.qtype)),
            None => "(no question)".to_string(),
        };
        let rcode = rcode_name(reply.header.rcode);
        eprintln!(
            "[{peer}] {asked} -> {rcode}, {} answers",
            reply.answers.len()
        );
        // A failed send only affects this one client, so log it and carry on.
        if let Err(e) = socket.send_to(&encode_udp(reply), peer) {
            eprintln!("[{peer}] send failed: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wire::{NOERROR, NXDOMAIN, TYPE_A, TYPE_CNAME, TYPE_TXT};

    fn zone() -> Zone {
        Zone::parse(include_str!("../zone.txt")).unwrap()
    }

    fn query(name: &str, qtype: u16) -> Vec<u8> {
        Message {
            header: Header {
                id: 0x1234,
                rd: true,
                ..Header::default()
            },
            questions: vec![Question {
                name: name.into(),
                qtype,
                qclass: CLASS_IN,
            }],
            answers: vec![],
        }
        .encode()
    }

    #[test]
    fn answers_from_the_zone() {
        let reply = respond(&query("WWW.Hello.Test", TYPE_A), &zone()).unwrap();
        let h = reply.header;
        assert_eq!(
            (h.id, h.qr, h.aa, h.rd, h.ra),
            (0x1234, true, true, true, false)
        );
        assert_eq!(h.rcode, NOERROR);
        // The question keeps the client's letter case.
        assert_eq!(reply.questions[0].name, "WWW.Hello.Test");
        // The alias is chased: its CNAME, then both A records of hello.test.
        let types: Vec<u16> = reply.answers.iter().map(|r| r.data.rtype()).collect();
        assert_eq!(types, [TYPE_CNAME, TYPE_A, TYPE_A]);

        let nope = respond(&query("nope.test", TYPE_A), &zone()).unwrap();
        assert_eq!((nope.header.rcode, nope.answers.len()), (NXDOMAIN, 0));
    }

    #[test]
    fn garbage_gets_formerr_or_silence() {
        let zone = zone();
        let mut no_question = query("hello.test", TYPE_A);
        no_question[5] = 0; // QDCOUNT = 0
        let mut two_questions = query("hello.test", TYPE_A);
        two_questions[5] = 2;
        let truncated = &query("hello.test", TYPE_A)[..20];
        for bad in [
            &no_question[..],
            &two_questions,
            truncated,
            b"\x12\x34\x01\x00",
        ] {
            let reply = respond(bad, &zone).unwrap();
            assert_eq!(reply.header.rcode, FORMERR, "{bad:?}");
            assert!(reply.questions.is_empty());
        }

        // Too short for an ID, or a response rather than a query: no reply.
        assert_eq!(respond(b"\x12\x34\x01", &zone), None);
        let mut response = query("hello.test", TYPE_A);
        response[2] |= 0x80; // set QR
        assert_eq!(respond(&response, &zone), None);
    }

    #[test]
    fn other_opcodes_and_classes_get_notimp() {
        let mut status = query("hello.test", TYPE_A);
        status[2] |= 2 << 3; // OPCODE 2 (STATUS) sits in bits 14..11
        let reply = respond(&status, &zone()).unwrap();
        assert_eq!((reply.header.opcode, reply.header.rcode), (2, NOTIMP));

        let mut chaos = query("hello.test", TYPE_TXT);
        let len = chaos.len();
        chaos[len - 1] = 3; // QCLASS CH
        assert_eq!(respond(&chaos, &zone()).unwrap().header.rcode, NOTIMP);
    }

    #[test]
    fn oversized_replies_are_truncated() {
        let zone = Zone::parse(&format!("big.test 60 TXT \"{}\"", "x".repeat(600))).unwrap();
        let reply = respond(&query("big.test", TYPE_TXT), &zone).unwrap();
        let bytes = encode_udp(reply);
        assert!(bytes.len() <= MAX_UDP);
        let parsed = Message::parse(&bytes).unwrap();
        assert!(parsed.header.tc);
        assert!(parsed.answers.is_empty());
    }
}
