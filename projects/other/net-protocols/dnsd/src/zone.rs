//! The zone: the records this server is authoritative for, loaded from a
//! text file with one record per line:
//!
//! ```text
//! # name           TTL   TYPE   value
//! hello.test       300   A      192.0.2.1
//! hello.test       300   AAAA   2001:db8::1
//! www.hello.test   300   CNAME  hello.test
//! hello.test       300   TXT    "hello from dnsd"
//! ```

use std::collections::HashMap;

use crate::wire::{MAX_NAME, NOERROR, NXDOMAIN, RData, Record, SERVFAIL};

/// How many CNAMEs `lookup` follows before giving up, so a loop in the zone
/// (a -> b -> a) can't spin forever.
const MAX_CHAIN: usize = 8;

/// Records grouped by owner name. Keys are lowercase because DNS names are
/// case-insensitive (RFC 1035 2.3.3).
#[derive(Debug, Default)]
pub struct Zone {
    names: HashMap<String, Vec<Record>>,
}

impl Zone {
    /// Parses a zone file. Errors name the line, e.g. "line 3: bad TTL".
    pub fn parse(text: &str) -> Result<Zone, String> {
        let mut zone = Zone::default();
        for (i, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let record = parse_line(line).map_err(|e| format!("line {}: {e}", i + 1))?;
            // `entry().or_default()` finds the name's Vec or inserts an
            // empty one, in a single lookup.
            zone.names
                .entry(record.name.clone())
                .or_default()
                .push(record);
        }
        Ok(zone)
    }

    pub fn record_count(&self) -> usize {
        self.names.values().map(Vec::len).sum()
    }

    /// Answers a question: returns the response code and the answer records.
    ///
    /// - Records of the asked type: NOERROR with those records.
    /// - A CNAME instead: add it to the answers and look up its target
    ///   (RFC 1034 4.3.2).
    /// - The name exists but has neither: NOERROR with no answers, which
    ///   resolvers call NODATA.
    /// - The name doesn't exist: NXDOMAIN. At the end of a CNAME chain the
    ///   code describes the last name (RFC 6604), and the CNAMEs stay.
    pub fn lookup(&self, name: &str, qtype: u16) -> (u8, Vec<Record>) {
        let mut answers = Vec::new();
        let mut name = name.to_ascii_lowercase();
        for _ in 0..MAX_CHAIN {
            let Some(records) = self.names.get(&name) else {
                return (NXDOMAIN, answers);
            };
            let before = answers.len();
            answers.extend(records.iter().filter(|r| r.data.rtype() == qtype).cloned());
            if answers.len() > before {
                return (NOERROR, answers);
            }
            let cname = records.iter().find_map(|r| match &r.data {
                RData::Cname(target) => Some((r, target)),
                _ => None,
            });
            let Some((record, target)) = cname else {
                return (NOERROR, answers);
            };
            answers.push(record.clone());
            name = target.clone();
        }
        (SERVFAIL, Vec::new())
    }
}

fn parse_line(line: &str) -> Result<Record, String> {
    let (name, rest) = next_word(line);
    let (ttl, rest) = next_word(rest);
    let (rtype, value) = next_word(rest);
    // The value is the rest of the line, because TXT values contain spaces.
    let value = value.trim();

    let name = parse_name(name)?;
    let ttl = ttl.parse().map_err(|_| format!("bad TTL {ttl:?}"))?;
    let data = match rtype.to_ascii_uppercase().as_str() {
        // `parse` works for anything that implements `FromStr`, including
        // `Ipv4Addr` and `Ipv6Addr`. The type comes from the enum variant.
        "A" => RData::A(
            value
                .parse()
                .map_err(|_| format!("bad IPv4 address {value:?}"))?,
        ),
        "AAAA" => RData::Aaaa(
            value
                .parse()
                .map_err(|_| format!("bad IPv6 address {value:?}"))?,
        ),
        "CNAME" => RData::Cname(parse_name(value)?),
        "TXT" => {
            let text = value
                .strip_prefix('"')
                .and_then(|v| v.strip_suffix('"'))
                .ok_or("TXT value must be in double quotes")?;
            RData::Txt(text.to_string())
        }
        other => return Err(format!("unsupported type {other:?}")),
    };
    Ok(Record { name, ttl, data })
}

/// Splits off the first whitespace-separated word.
fn next_word(s: &str) -> (&str, &str) {
    let s = s.trim_start();
    s.split_at(s.find(char::is_whitespace).unwrap_or(s.len()))
}

/// Lowercases a name and checks it against the rules the wire format
/// enforces (labels of 1 to 63 printable characters, 255 bytes in all), so
/// encoding it later can't go wrong. A trailing dot is optional.
fn parse_name(name: &str) -> Result<String, String> {
    let name = name.strip_suffix('.').unwrap_or(name).to_ascii_lowercase();
    let labels_ok = name
        .split('.')
        .all(|l| (1..=63).contains(&l.len()) && l.bytes().all(|b| b.is_ascii_graphic()));
    // On the wire: one length byte per label plus the final zero, which is
    // the dotted length + 2.
    if !labels_ok || name.len() + 2 > MAX_NAME {
        return Err(format!("bad name {name:?}"));
    }
    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::{TYPE_A, TYPE_AAAA, TYPE_CNAME, TYPE_TXT};

    fn types(records: &[Record]) -> Vec<u16> {
        records.iter().map(|r| r.data.rtype()).collect()
    }

    #[test]
    fn loads_the_shipped_zone_file() {
        let zone = Zone::parse(include_str!("../zone.txt")).unwrap();
        assert_eq!(zone.record_count(), 5);
        let (rcode, answers) = zone.lookup("hello.test", TYPE_TXT);
        assert_eq!(rcode, NOERROR);
        assert_eq!(answers[0].data, RData::Txt("hello from dnsd".into()));
    }

    #[test]
    fn lookup_rules() {
        let zone = Zone::parse(include_str!("../zone.txt")).unwrap();
        // Two A records, matched case-insensitively.
        assert_eq!(
            types(&zone.lookup("HeLLo.TeSt", TYPE_A).1),
            [TYPE_A, TYPE_A]
        );
        // The alias is followed: CNAME first, then the target's records.
        let (rcode, answers) = zone.lookup("www.hello.test", TYPE_AAAA);
        assert_eq!(
            (rcode, types(&answers)),
            (NOERROR, vec![TYPE_CNAME, TYPE_AAAA])
        );
        // Asking for the CNAME itself doesn't chase it.
        assert_eq!(
            types(&zone.lookup("www.hello.test", TYPE_CNAME).1),
            [TYPE_CNAME]
        );
        // NODATA: the name exists, but has no MX (type 15).
        assert_eq!(zone.lookup("hello.test", 15), (NOERROR, vec![]));
        assert_eq!(zone.lookup("nope.test", TYPE_A), (NXDOMAIN, vec![]));
    }

    #[test]
    fn dangling_and_looping_cnames() {
        let zone = Zone::parse(
            "a.test 60 CNAME b.test\n\
             b.test 60 CNAME a.test\n\
             gone.test 60 CNAME nowhere.test\n",
        )
        .unwrap();
        assert_eq!(zone.lookup("a.test", TYPE_A), (SERVFAIL, vec![]));
        let (rcode, answers) = zone.lookup("gone.test", TYPE_A);
        assert_eq!((rcode, types(&answers)), (NXDOMAIN, vec![TYPE_CNAME]));
    }

    #[test]
    fn rejects_bad_lines_with_line_numbers() {
        let long_label = format!("{}.test 60 A 1.2.3.4", "x".repeat(64));
        for (text, error) in [
            ("# ok\nx.test abc A 1.2.3.4", "line 2: bad TTL \"abc\""),
            ("x.test 60 A 1.2.3", "line 1: bad IPv4 address \"1.2.3\""),
            (
                "x.test 60 AAAA 1.2.3.4",
                "line 1: bad IPv6 address \"1.2.3.4\"",
            ),
            ("x.test 60 MX mail.test", "line 1: unsupported type \"MX\""),
            (
                "x.test 60 TXT hi",
                "line 1: TXT value must be in double quotes",
            ),
            ("x..test 60 A 1.2.3.4", "line 1: bad name \"x..test\""),
            ("x.test 60", "line 1: unsupported type \"\""),
            (&long_label, "line 1: bad name"),
        ] {
            let got = Zone::parse(text).unwrap_err();
            assert!(got.starts_with(error), "{text:?}: {got}");
        }
    }
}
