# dnsd — authoritative DNS server over UDP

An RFC 1035 subset that answers A, AAAA, CNAME, and TXT questions from a
small zone file. It parses the binary message format by hand, decodes
compression pointers (and refuses pointer loops), compresses its own
answers with `C0 0C`, chases CNAMEs inside the zone, and returns the right
response codes: NXDOMAIN, NODATA, NOTIMP, and FORMERR. It's checked by a
real client, `dig`.

```sh
cargo run -- 127.0.0.1:10053 zone.txt     # the defaults: plain `cargo run` does the same
cargo test                                # codec + zone unit tests, real UDP round trips
```

## Try it

Start the server in the `dnsd` folder, then ask it questions with `dig`
from a second terminal:

```sh
dig @127.0.0.1 -p 10053 hello.test
# ;; ->>HEADER<<- opcode: QUERY, status: NOERROR, id: 15018
# ;; flags: qr aa rd; QUERY: 1, ANSWER: 2, AUTHORITY: 0, ADDITIONAL: 0
# ;; WARNING: recursion requested but not available
# ...
# ;; ANSWER SECTION:
# hello.test.		300	IN	A	192.0.2.1
# hello.test.		300	IN	A	192.0.2.2
# ...
# ;; MSG SIZE  rcvd: 60
```

`aa` means the answer is authoritative. The warning is expected: dig sets
RD ("please recurse") by default, and dnsd only answers from its own zone,
so it replies with RA=0. Add `+norec` and the warning goes away. There's no
"OPT PSEUDOSECTION" either, because dnsd answers without EDNS.

```sh
dig @127.0.0.1 -p 10053 hello.test +short
# 192.0.2.1
# 192.0.2.2
dig @127.0.0.1 -p 10053 AAAA hello.test +short
# 2001:db8::1
dig @127.0.0.1 -p 10053 TXT hello.test +short
# "hello from dnsd"
dig @127.0.0.1 -p 10053 HeLLo.TeSt +short       # names are case-insensitive
# 192.0.2.1
# 192.0.2.2
dig @127.0.0.1 -p 10053 www.hello.test +short   # an alias, chased inside the zone
# hello.test.
# 192.0.2.1
# 192.0.2.2
dig @127.0.0.1 -p 10053 www.hello.test +norec
# ;; flags: qr aa; QUERY: 1, ANSWER: 3, AUTHORITY: 0, ADDITIONAL: 0
# ;; ANSWER SECTION:
# www.hello.test.		300	IN	CNAME	hello.test.
# hello.test.		300	IN	A	192.0.2.1
# hello.test.		300	IN	A	192.0.2.2
```

The error cases. NODATA isn't a separate code: it's NOERROR with zero
answers, meaning the name exists but has no record of that type.

```sh
dig @127.0.0.1 -p 10053 nope.test
# ;; ->>HEADER<<- opcode: QUERY, status: NXDOMAIN, id: 28145
dig @127.0.0.1 -p 10053 MX hello.test           # NODATA: the name exists, but has no MX
# ;; ->>HEADER<<- opcode: QUERY, status: NOERROR, id: 25806
# ;; flags: qr aa rd; QUERY: 1, ANSWER: 0, AUTHORITY: 0, ADDITIONAL: 0
dig @127.0.0.1 -p 10053 hello.test +opcode=status +noedns
# ;; ->>HEADER<<- opcode: STATUS, status: NOTIMP, id: 56640
```

(Without `+noedns`, dig adds a hint suggesting EDNS caused the NOTIMP.)

`dig` only sends well-formed messages, so craft a bad one by hand. This
query has ID `abcd` and one question whose name is a compression pointer
to itself (`C0 0C` at offset 12). The reply is just a header: the same ID,
flags `81 01` (QR, RD, RCODE 1 = FORMERR):

```sh
printf '\xab\xcd\x01\x00\x00\x01\x00\x00\x00\x00\x00\x00\xc0\x0c' | nc -u -w1 127.0.0.1 10053 | xxd
# 00000000: abcd 8101 0000 0000 0000 0000            ............
```

The server logs one line per query. `TYPE15` is MX in the generic notation
for types it doesn't know (RFC 3597):

```
[127.0.0.1:55913] www.hello.test A -> NOERROR, 3 answers
[127.0.0.1:64537] nope.test A -> NXDOMAIN, 0 answers
[127.0.0.1:63859] hello.test TYPE15 -> NOERROR, 0 answers
[127.0.0.1:54097] (no question) -> FORMERR, 0 answers
```

Edit `zone.txt` and restart the server to serve your own names.

## Covers

Reading a real RFC wire format: a `Cursor<'a>` that borrows the whole
message (compression pointers are offsets from its start), bounds-checked
reads with `slice::get` and `split_at_checked`, big-endian integers, and
masks and shifts for the flag word. Compression pointers, with a "must
jump backwards" rule that makes loops impossible, plus the 63-byte label
and 255-byte name limits. `Header`/`Question`/`Record`/`Message` structs
that mirror the wire and an `RData` enum. Back-patching RDLENGTH after
writing the data, struct update syntax (`..Header::default()`), and
collecting an iterator of `Result`s into `Result<Vec<_>, _>`. A zone file
parser with line-numbered errors, `str::parse` into `Ipv4Addr`/`Ipv6Addr`,
and `HashMap::entry`. DNS semantics: case-insensitive names, CNAME chasing
with a hop limit, NXDOMAIN versus NODATA, the 512-byte UDP limit and the
TC bit, and never answering a response. Unit tests on bytes captured from
`dig`, plus integration tests on port 0.

## Rewrite exercises

1. Rewrite from scratch. Start with `Cursor::name` and its tests: a plain
   name, a pointer, a pointer to itself, a name over 255 bytes.
2. Add MX records (`hello.test 300 MX 10 mail.hello.test`). The RDATA is a
   16-bit preference followed by a name, which you can compress too.
   Check it with `dig MX hello.test`.
3. Add an SOA record and put it in the authority section of NXDOMAIN and
   NODATA replies, so resolvers can cache the negative answer (RFC 2308).
   dig will then show `AUTHORITY: 1`.
4. Serve DNS over TCP on the same port (RFC 1035 4.2.2: each message has a
   2-byte length prefix), so that `dig +tcp` works and clients that get a
   TC reply can fetch the full answer.
5. Forward names outside the zone to an upstream resolver such as
   `1.1.1.1:53`: relay the query under a new ID, map the reply back to the
   client's ID and address, set RA, and handle upstream timeouts without
   blocking other clients.
