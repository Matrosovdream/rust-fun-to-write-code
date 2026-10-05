# Plan — 17 network protocol projects

Build your own servers and protocols, starting from raw sockets: chat, SMTP, a
Redis clone, DNS, TFTP, HTTP/1.1, WebSocket, proxies, a load balancer, and an
ngrok-style tunnel. Every project is a real server that runs on localhost, so
you can test it with tools already on a Mac (`nc`, `curl`, `dig`, `tftp`, a
browser). No Docker.

Projects live in `projects/other/net-protocols/<name>` and are ordered as a
progression. They assume you have done the easy group, especially
`echoserver` (TCP basics) and `hashr` (threads and channels).

## Conventions

- **Layout.** Each project keeps the same shape:
  - `src/lib.rs` holds the protocol: parsers, encoders, state machines, and a
    `serve(listener, …)` function. As much as possible is pure code over
    `&[u8]` / `&str`, so it's testable without sockets.
  - `src/main.rs` is the server binary (named after the package). It's thin:
    parse args, bind, call `serve`.
  - `src/bin/<name>-client.rs` holds a client, for projects that need one.
  - `tests/net.rs` holds integration tests over real sockets.
- **Running.** Run commands from inside the project folder: `cargo run`
  starts the server and `cargo run --bin <name>-client` starts the client.
  The first argument is always the listen address.
- **Localhost only.** Servers bind `127.0.0.1` by default. That avoids the
  macOS firewall prompt and keeps the toy servers off your LAN.
- **Fixed ports.** Each project has its own default port (see the table
  below), so all of them can run at the same time. The defaults stay away
  from the usual dev ports (8080, 9000, 6379/6380), which Docker containers
  often already hold.
- **Bad input never crashes the server.** Malformed or hostile input gets
  an error reply or a closed connection, never a panic. `expect` is fine
  for things that really can't fail, such as a poisoned mutex.
- **Dependencies.** std only, unless the crate is the point of the project
  or hand-writing it would just be noise: `serde_json` (jsonrpc, restapi),
  `sha1` + `base64` (websock), `tokio` (asyncchat, tunnel).
- **Three layers of tests:**
  - unit tests on byte slices, including partial, malformed, and oversized
    input;
  - integration tests on `127.0.0.1:0`, where the OS picks a free port;
  - a "Try it" section in each README with exact commands for real tools.

## Port map

| Project | Default address | Transport | Extra |
| --- | --- | --- | --- |
| linechat | 127.0.0.1:7000 | TCP | |
| smtpd | 127.0.0.1:2525 | TCP | |
| miniredis | 127.0.0.1:6399 | TCP | (real Redis uses 6379) |
| binkv | 127.0.0.1:7100 | TCP | |
| jsonrpc | 127.0.0.1:7200 | TCP | |
| udpping | 127.0.0.1:7300 | UDP | |
| tftpd | 127.0.0.1:6969 | UDP | each transfer gets its own ephemeral port |
| dnsd | 127.0.0.1:10053 | UDP | (5353 is taken by macOS mDNS) |
| httpd | 127.0.0.1:8180 | TCP | |
| fetchr | — | client | |
| restapi | 127.0.0.1:8181 | TCP | |
| ssefeed | 127.0.0.1:8182 | TCP | |
| websock | 127.0.0.1:8183 | TCP | |
| httpproxy | 127.0.0.1:8888 | TCP | |
| loadbal | 127.0.0.1:9400 | TCP | admin 9490, backends 9401–9403 |
| asyncchat | 127.0.0.1:7001 | TCP | |
| tunnel | 127.0.0.1:7500 | TCP | control on 7500, public on 8090 |

## Coverage map

| Area | Where it's covered |
| --- | --- |
| Threads, channels, the actor pattern | linechat, ssefeed, websock |
| `Arc<Mutex<_>>` shared state | miniredis, restapi, jsonrpc |
| Atomics | loadbal |
| Line-based text protocols | linechat, smtpd, jsonrpc |
| Protocol as a state machine | smtpd, tftpd, websock |
| Streaming parsers and partial frames | miniredis, binkv, websock |
| Binary wire formats and byte order | binkv, udpping, tftpd, dnsd, websock |
| UDP: datagrams, timeouts, retransmission | udpping, tftpd, dnsd |
| HTTP/1.1 message format | httpd, fetchr, restapi, ssefeed, httpproxy |
| Thread pools | httpd, restapi |
| Traits and boxed closures for extensibility | jsonrpc, restapi |
| Long-lived connections and fan-out | linechat, ssefeed, websock, asyncchat |
| Splicing two streams (proxying) | httpproxy, loadbal, tunnel |
| `serde` / JSON | jsonrpc, restapi |
| async / tokio | asyncchat, tunnel |
| Implementing `Read` / `Write` adapters | fetchr, ssefeed |
| Testing over real sockets | every project |

---

## Group A — TCP: text and binary protocols

### 1. `linechat` — multi-room TCP chat (`:7000`)

A line-based chat server. Plain lines go to everyone in your current room as
`[room] nick: text`. Commands:

- `/nick NAME`
- `/join ROOM` (everyone starts in `lobby`)
- `/msg NICK text` for a private message
- `/who` and `/rooms`
- `/quit`

Server notices start with `* `.

*   **Patterns:** a single hub thread owns all state (clients, nicks, rooms).
    Each connection has a reader thread that sends `Event`s to the hub over
    `mpsc`, and a writer thread that drains that client's own `Receiver<String>`
    into the socket. There's no `Mutex` anywhere.
*   **Stdlib:** `TcpListener`, `std::sync::mpsc`, `std::thread`.
*   **Crates:** none.
*   **Try it:** `cargo run`, then run `nc 127.0.0.1 7000` in three terminals.
*   **Testing:** test the hub by feeding it events and reading the per-client
    receivers (no sockets). An integration test has two clients see each
    other's messages.
*   **Teaches:** message passing instead of shared memory, who owns what, and
    cleaning up on disconnect (reader hits EOF → `Leave` event).

### 2. `smtpd` — SMTP mail sink (`:2525`)

An RFC 5321 subset:

- `220` greeting, then `EHLO`/`HELO`, `MAIL FROM:<…>`, `RCPT TO:<…>`
  (repeatable), and `DATA`.
- After `DATA` comes `354`, then the message body up to a lone `.` line.
- Then `250 OK: queued as <id>`.
- Also `RSET`, `NOOP`, and `QUIT` → `221`.

Commands sent in the wrong order get `503`, unknown commands get `500`, and
messages that are too large get `552`. Each message is saved as an `.eml` file
in a mail directory with `X-Envelope-From/To` headers prepended.

*   **Patterns:** `enum Session { Ready, Mail { from }, Rcpt { from, to },
    Data { … } }` with a pure `fn step(self, line) -> (Session, Reply)`. State
    is consumed by value and a new one is returned.
*   **Stdlib:** `BufRead`, `std::fs`, CRLF handling.
*   **Crates:** none (`tempfile` for tests).
*   **Try it:** `curl smtp://127.0.0.1:2525 --mail-from alice@example.com
    --mail-rcpt bob@example.com -T examples/hello.eml`, then `ls maildir/`.
    Or play both sides by hand with `nc -c 127.0.0.1 2525`.
*   **Testing:** a transition table (every command × every state),
    dot-unstuffing, and a full session over a socket that checks the saved
    file.
*   **Teaches:** a protocol as an explicit state machine, types that make
    illegal transitions unrepresentable, and line endings on the wire.

### 3. `miniredis` — Redis-compatible server (`:6399`)

A RESP2 server. It handles every RESP2 type: simple strings, errors, integers,
bulk strings, null bulk, and arrays. Commands:

- `PING [msg]`, `ECHO`
- `SET key value [EX s | PX ms]`, `GET`, `DEL k…`, `EXISTS k…`
- `INCR`, `EXPIRE`, `TTL`
- `KEYS pattern` (glob with `*` and `?`)
- `DBSIZE`, `FLUSHALL`

Inline commands (`PING\r\n`) work too, so you can drive it with plain `nc`.
Pipelining is supported: every complete frame in the buffer is answered in
order.

*   **Patterns:** `fn parse(buf: &[u8]) -> Result<Option<(Frame, usize)>,
    Error>`, where `None` means "need more bytes". `Frame::encode`, a
    `Command` enum built from a `Frame`, and a `Db` of `HashMap<Vec<u8>,
    Entry>` with an optional `Instant` deadline (lazy expiry).
*   **Stdlib:** `Vec<u8>` byte handling, `Instant`, `Arc<Mutex<_>>`.
*   **Crates:** none.
*   **Try it:** `printf 'PING\r\n' | nc 127.0.0.1 6399`, the bundled
    `miniredis-cli`, or `redis-cli -p 6399` if you have Redis installed.
*   **Testing:** the parser on complete, partial, and garbage input; every
    command; expiry with a short `PX`; pipelining over a real socket.
*   **Teaches:** streaming parsers over partial reads, bytes instead of
    `String`, and time-based expiry.

### 4. `binkv` — key-value store over a binary protocol (`:7100`)

A custom binary protocol, designed and documented in `lib.rs`:

- On connect, the client sends the magic bytes `BKV1`.
- Every message is a frame: `[u32 BE length][u8 opcode][payload]`.
- Strings inside the payload are `[u16 BE len][bytes]`.
- Requests are PING, GET, PUT, DEL, and LIST.
- Responses carry a status byte: OK, NOT_FOUND, or ERROR.

Frames over 1 MiB are rejected before allocating anything.

*   **Patterns:** `Request`/`Response` enums with `encode`/`decode`, a
    `read_frame(&mut impl Read) -> io::Result<Option<Vec<u8>>>` (`None` = clean
    EOF), and a small cursor type for decoding.
*   **Stdlib:** `u32::to_be_bytes`/`from_be_bytes`, `Read::read_exact`.
*   **Crates:** none.
*   **Try it:** `cargo run --bin binkv-client -- put name rust`, `… get
    name`. Look at the raw bytes with `printf 'BKV1\0\0\0\1\5' | nc
    127.0.0.1 7100 | xxd`.
*   **Testing:** encode/decode round trips, truncated frames, an oversized
    length prefix, and a bad magic.
*   **Teaches:** byte order, length-prefix framing, protocol versioning, and
    never trusting a length that came off the wire.

### 5. `jsonrpc` — JSON-RPC 2.0 over TCP (`:7200`)

Newline-delimited JSON implementing the full JSON-RPC 2.0 spec:

- requests and notifications (a request with no `id` gets no response);
- batches (an empty batch is invalid, and an all-notification batch gets no
  reply);
- the standard error codes `-32700`, `-32600`, `-32601`, `-32602`, and
  `-32603`.

Methods: `add` and `subtract` (positional or named params), `echo`, `now`, and
`counter.incr` (shared state).

*   **Patterns:** a `Registry` of `HashMap<String, Box<dyn Fn(Value) ->
    Result<Value, RpcError> + Send + Sync>>`, `registry.register("add", |p|
    …)`, and `handle_line(&str) -> Option<String>`.
*   **Stdlib:** `BufRead::lines`, `Arc`.
*   **Crates:** `serde_json`.
*   **Try it:** `echo '{"jsonrpc":"2.0","method":"add","params":[1,2],"id":1}'
    | nc 127.0.0.1 7200`, or `cargo run --bin jsonrpc-client -- add 1 2`.
*   **Testing:** every example from the spec's "Examples" section as a table.
*   **Teaches:** boxed closures in a registry, `Send + Sync` bounds, dynamic
    JSON, and implementing a spec exactly.

---

## Group B — UDP

### 6. `udpping` — UDP ping (`:7300/udp`)

The server echoes every datagram back. Two flags make the network worse on
purpose: `--drop 0.3` drops 30% of packets, and `--delay 50` adds latency.
The client sends `[u32 seq][u64 send-time nanos]` probes and prints a
`seq=3 time=0.21 ms` line or `timeout` for each one. At the end it prints a
summary like real `ping`: sent, received, loss %, and min/avg/max RTT.
Late or duplicate replies are recognised by their sequence number and
ignored.

*   **Patterns:** a probe encoder/decoder, matching replies to requests by
    sequence number, a `Stats` struct, and a hand-written xorshift PRNG for
    the drop decisions.
*   **Stdlib:** `UdpSocket::{send_to, recv_from}`, `set_read_timeout`,
    `Instant`.
*   **Crates:** none.
*   **Try it:** `cargo run -- 127.0.0.1:7300 --drop 0.3`, then `cargo run
    --bin udpping-client -- 127.0.0.1:7300 -c 20`. `nc -u 127.0.0.1 7300`
    works too, as a plain echo.
*   **Testing:** the probe codec, the stats math, a real UDP round trip, and
    `--drop 1.0` producing all timeouts.
*   **Teaches:** datagrams keep their boundaries but nothing guarantees
    delivery; timeouts; random numbers without a crate.

### 7. `tftpd` — TFTP server (`:6969/udp`)

An RFC 1350 server supporting RRQ, WRQ, DATA, ACK, and ERROR. It serves files
from a root directory. Each transfer runs on its own thread and its own
ephemeral socket (the TFTP "TID"). It moves 512-byte blocks in lock-step, and
the final block is shorter than 512 bytes. If a file is an exact multiple of
512, the last block is empty. Timeouts are retransmitted (1 s, 5 tries).

Errors returned:

- file not found
- access violation (`..` or an absolute path)
- file already exists
- illegal operation
- unknown TID

Uploads are written to a temporary file and then renamed into place.

*   **Patterns:** a `Packet` enum with `parse`/`encode`, a per-transfer state
    machine, and block numbers using `u16::wrapping_add`.
*   **Stdlib:** `UdpSocket`, `std::fs`, `Path` sanitising.
*   **Crates:** none (`tempfile` for tests).
*   **Try it:** `cargo run -- 127.0.0.1:6969 files`, then use the macOS
    built-in `tftp 127.0.0.1 6969` → `binary` → `get hello.txt` / `put
    notes.txt`.
*   **Testing:** the packet codec; downloads of 0-, 511-, 512-, and
    1025-byte files; an upload; a retransmit when the test client
    deliberately drops an ACK.
*   **Teaches:** building reliability (ack + retransmit) on top of UDP,
    wraparound arithmetic, and keeping file serving inside a sandbox.

### 8. `dnsd` — authoritative DNS server (`:10053/udp`)

An RFC 1035 subset:

- The header has an id, flags (QR, OPCODE, AA, TC, RD, RA, RCODE), and
  record counts.
- One question per message (name labels, QTYPE, QCLASS).
- Answers can be A, AAAA, CNAME, or TXT, loaded from a simple zone file
  (`name TTL TYPE value`).

Behaviour:

- **Response codes:** NXDOMAIN for unknown names, NODATA when the name
  exists but has no records of the requested type, NOTIMP for other
  opcodes, FORMERR for garbage.
- **Matching:** names match case-insensitively.
- **CNAME chasing:** CNAMEs are followed inside the zone.
- **Compression:** the parser decodes compression pointers (with a guard
  against pointer loops). Answers point back at the question name
  (`0xC00C`).

*   **Patterns:** a `Message` struct mirroring the wire format, a byte cursor
    with `read_u16`/`read_name`, and bit masks for the flag fields.
*   **Stdlib:** `UdpSocket`, `Ipv4Addr`/`Ipv6Addr`, bit operations.
*   **Crates:** none.
*   **Try it:** `cargo run -- 127.0.0.1:10053 zone.txt`, then `dig
    @127.0.0.1 -p 10053 hello.test`, `… www.hello.test` (CNAME), `… TXT
    hello.test`, and `… nope.test` (NXDOMAIN).
*   **Testing:** parsing real query bytes captured from `dig`, decoding a
    compression pointer (including a loop), building a response and
    re-parsing it, and a full round trip over UDP.
*   **Teaches:** reading a real RFC wire format, bit fields, cursor parsing,
    and being checked by a real client (`dig`).

---

## Group C — HTTP/1.1 from scratch

### 9. `httpd` — static file server (`:8180`)

**Request parsing**

- Parses the request line and headers. Header names are case-insensitive.
- Reads `Content-Length` bodies.
- Rejects malformed requests with 400 and an oversized header block with
  431.

**Serving files**

- Handles `GET` and `HEAD`. Any other method gets 405 with an `Allow` header.
- Serves files from a root directory with the right MIME type.
- A directory serves its `index.html`, or an HTML directory listing.
- Percent-decodes paths, and returns 403 for any path that escapes the
  root.

**Connections**

- Keep-alive is on by default for HTTP/1.1. `Connection: close` is honoured
  and HTTP/1.0 connections close.
- Idle connections time out.
- Logs one access line per request.
- Runs on a fixed thread pool.

*   **Patterns:** `Request`/`Response` types, `parse_request(&mut impl
    BufRead)`, and a `ThreadPool` (workers taking jobs off an `mpsc` channel,
    joined in `Drop`).
*   **Stdlib:** `BufReader`, `Read::take`, `std::fs`, `Path` normalisation.
*   **Crates:** none.
*   **Try it:** `cargo run -- 127.0.0.1:8180 public`, then `curl -v
    127.0.0.1:8180/`, `curl -I …/style.css`, and `curl -v --path-as-is
    127.0.0.1:8180/../Cargo.toml` (→ 403). Open it in a browser. `curl -v
    URL URL` shows "Re-using existing connection".
*   **Testing:** parser tests (well-formed, bare LF, huge headers), path
    resolution tests, and two requests over one kept-alive socket.
*   **Teaches:** the anatomy of an HTTP message, line-oriented headers
    followed by an exact-length body, a thread pool with clean shutdown, and
    path safety.

### 10. `fetchr` — curl-lite HTTP client

`fetchr [-I] [-v] [-L] [-X METHOD] [-d DATA] [-H 'K: V'] URL`, for `http://`
URLs only. TLS is out of scope, so `https://` gets a clear error. What it
does:

- Parses the URL into host, port, and path+query, then builds the request
  (`Host`, `User-Agent`, `Accept`, `Connection: close`).
- Reads the status line and headers, then the body. The body is framed by
  `Content-Length`, by chunked encoding, or by reading until the server
  closes.
- `-v` prints `> ` / `< ` lines to stderr like curl.
- `-L` follows 301/302/303/307/308 redirects, up to 10. A 303 switches the
  method to GET.

*   **Patterns:** a `Url` struct with `FromStr`, a `ChunkedReader<R: BufRead>`
    that implements `Read`, and a redirect loop.
*   **Stdlib:** `TcpStream`, `Read`/`BufRead`, `Read::take`.
*   **Crates:** none.
*   **Try it:** `cargo run -- -v http://127.0.0.1:8180/` (with httpd running),
    or `cargo run -- -I http://example.com/`.
*   **Testing:** a URL parser table, the chunked decoder (with extensions and
    trailers), and an in-test canned server serving a fixed-length body, a
    chunked body, a redirect chain, and a redirect loop.
*   **Teaches:** the client side of HTTP, writing your own `Read` adapter, and
    composing readers.

### 11. `restapi` — tiny web framework + todo API (`:8181`)

A minimal framework built on its own small HTTP module (written in this crate,
not depending on httpd):

- **`Request`:** method, path, path params, query, headers, and body.
- **`Response`:** includes a `Response::json` helper.
- **`Router`:** `Router::new().get("/todos", list).get("/todos/:id", show)…`,
  with 404 for unknown paths and 405 for a known path with the wrong method.
- **Middleware:** a logger (method, path, status, duration) and a per-IP
  token-bucket rate limiter that returns 429 with `Retry-After`.

On top of it sits a todo CRUD API (list, create, show, patch, delete) with an
in-memory store.

*   **Patterns:** a `Handler` trait implemented for closures, a `Middleware`
    trait with a `next` continuation, `Arc<Mutex<Store>>`, and a token bucket
    with an injected clock so it's testable.
*   **Stdlib:** `TcpListener`, a thread pool, `HashMap`.
*   **Crates:** `serde` (derive), `serde_json`.
*   **Try it:** `curl -s 127.0.0.1:8181/todos`, `curl -s -X POST -d
    '{"title":"write a router"}' 127.0.0.1:8181/todos`, `curl -X PATCH -d
    '{"done":true}' …/todos/1`, `curl -X DELETE …/todos/1`. A curl loop of 30
    requests trips the rate limiter.
*   **Testing:** a route-matching table, handlers called directly with
    constructed requests (no sockets), the token bucket with a fake clock,
    and one end-to-end socket test.
*   **Teaches:** designing a small API, trait objects for handlers and
    middleware, and keeping I/O at the edges so the core is testable.

### 12. `ssefeed` — Server-Sent Events (`:8182`)

Endpoints:

- **`GET /`:** an HTML page using `EventSource`, with a form for sending
  messages.
- **`GET /events`:** a long-lived `text/event-stream` response using
  `Transfer-Encoding: chunked`, with one chunk per event (`id:`, `event:`,
  `data:` lines). It sends a `: ping` heartbeat every 15 s.
- **`POST /publish`:** broadcasts the request body to every subscriber.

The server keeps the last 100 events in a ring buffer. A client that
reconnects with `Last-Event-ID` gets the events it missed. Subscribers that
have gone away are detected when a write fails, and pruned.

*   **Patterns:** a hub holding `Vec<mpsc::Sender<Event>>`, a writer per
    subscriber, `VecDeque` as a ring buffer, and an event formatter (multiline
    data becomes several `data:` lines).
*   **Stdlib:** `mpsc`, `VecDeque`, `recv_timeout` for heartbeats.
*   **Crates:** none.
*   **Try it:** `curl -N 127.0.0.1:8182/events` in one terminal, `curl -d
    'hello' 127.0.0.1:8182/publish` in another. Two browser tabs on
    `127.0.0.1:8182`. `curl -N -H 'Last-Event-ID: 3' …/events` replays
    missed events.
*   **Testing:** event formatting, chunk encoding, the replay logic, and an
    end-to-end test that subscribes, publishes, and reads the chunk.
*   **Teaches:** long-lived responses, chunked encoding from the server side,
    and fan-out with pruning of dead subscribers.

### 13. `websock` — WebSocket chat (`:8183`)

RFC 6455:

- **`GET /`:** serves the chat page.
- **`GET /ws`:** validates the upgrade request (`Upgrade`, `Connection`,
  `Sec-WebSocket-Version: 13`, the key) and answers `101` with
  `Sec-WebSocket-Accept = base64(sha1(key + GUID))`.

Framing:

- **Header:** FIN, RSV (must be 0), the opcode, the MASK bit, and a payload
  length that is 7, 16, or 64 bits.
- **Masking:** clients must mask their frames; an unmasked client frame
  closes the connection with 1002.
- **Fragmentation:** fragmented messages are reassembled.
- **Control frames:** at most 125 bytes and never fragmented. A ping gets a
  pong, and a close is echoed back.
- **Text:** must be valid UTF-8, otherwise the connection closes with 1007.

Text messages are broadcast to all clients. A terminal client is included.

*   **Patterns:** `Frame { fin, opcode, payload }` with `read_from`/`write_to`,
    an `Opcode` enum, a hub thread for broadcast, and XOR unmasking.
*   **Stdlib:** bit operations, `TcpStream::try_clone`, `mpsc`.
*   **Crates:** `sha1`, `base64`.
*   **Try it:** open `http://127.0.0.1:8183` in two browser tabs, then join
    from a terminal with `cargo run --bin websock-client`. Check the
    handshake with `curl` and the RFC's sample key.
*   **Testing:** the RFC sample key (`dGhlIHNhbXBsZSBub25jZQ==` →
    `s3pPLMBiTxaQ9kYGzzhZRbK+xOo=`), the RFC 5.7 example frames, all three
    length encodings, fragmentation, protocol violations, and two clients
    chatting over sockets.
*   **Teaches:** upgrading a connection from HTTP to another protocol,
    bit-level framing, and close handshakes.

---

## Group D — proxies and infrastructure

### 14. `httpproxy` — forward HTTP proxy (`:8888`)

**Plain HTTP**

- Takes absolute-form requests (`GET http://host/path HTTP/1.1`) and
  rewrites them to origin-form.
- Strips hop-by-hop headers and adds `Via`.
- Streams the upstream response back. There's one request per client
  connection.

**HTTPS**

- `CONNECT host:443` opens a tunnel: the proxy replies `200 Connection
  Established`, then copies bytes in both directions without understanding
  them.

**Errors and logging**

- Errors: 400 (bad request), 403 (`--block host`), 502 (upstream
  unreachable), and 504 (connect timeout).
- Logs one access line per request.

*   **Patterns:** request-target parsing, a header-rewrite function, and
    `splice(a, b)` using two threads, `io::copy`, and
    `shutdown(Shutdown::Write)` for half-close.
*   **Stdlib:** `TcpStream::connect_timeout`, `io::copy`, `Shutdown`.
*   **Crates:** none.
*   **Try it:** `curl -v -x http://127.0.0.1:8888 http://example.com/` and
    `curl -v -x http://127.0.0.1:8888 https://example.com/` (through a
    CONNECT tunnel).
*   **Testing:** target parsing, header rewriting, proxying to an in-test
    upstream, and a CONNECT tunnel to an in-test echo server.
*   **Teaches:** splicing two TCP streams, half-close, and why an HTTPS proxy
    can't see the content.

### 15. `loadbal` — L4 TCP load balancer (`:9400`)

`loadbal 127.0.0.1:9400 --backends 127.0.0.1:9401,127.0.0.1:9402,127.0.0.1:9403
--strategy round-robin|least-conn`.

- On accept, it picks a healthy backend and connects to it with a timeout.
  If the connect fails, it marks that backend down and tries the next one.
  Then it splices the two streams.
- A health-check thread probes every backend every 2 s and logs up/down
  changes.
- Each backend tracks active and total connections in atomics.
- An admin port (`:9490`) serves a plain-text stats table over HTTP.
- A `loadbal-backend` binary is included for testing: a tiny HTTP server
  that answers `hello from 127.0.0.1:9401`.

*   **Patterns:** strategy selection as a pure function over backend
    snapshots, `AtomicBool`/`AtomicUsize` with explicit `Ordering`, a
    background health thread, and failover.
*   **Stdlib:** `std::sync::atomic`, `connect_timeout`, threads.
*   **Crates:** none.
*   **Try it:** start three backends (`cargo run --bin loadbal-backend --
    127.0.0.1:9401`, and the same for `…:9402` and `…:9403`), then `cargo run
    -- 127.0.0.1:9400 --backends …`. A loop of `curl -s 127.0.0.1:9400`
    rotates across them. Kill a backend and watch it drop out. `curl
    127.0.0.1:9490` shows stats.
*   **Testing:** strategy unit tests, health-state transitions, round-robin
    distribution across in-test backends, and failover when one is down.
*   **Teaches:** atomics and memory ordering in practice, health checks, and
    proxying at layer 4 versus layer 7 (compare with httpproxy).

---

## Group E — async (tokio)

### 16. `asyncchat` — linechat on tokio (`:7001`)

The same protocol as linechat (`/nick`, `/join`, `/msg`, `/who`, `/rooms`,
`/quit`), rebuilt on tokio. There's one task per connection, and its loop
uses `tokio::select!` to wait for either a line from the socket or a message
for this client. Shared state lives in a hub task that the connection tasks
reach over channels.

Ctrl-C shuts down gracefully: it stops accepting, tells every client
`* server shutting down`, waits for the connection tasks to finish, and
exits. The README includes a table comparing it with linechat.

*   **Patterns:** an `async fn` per connection, a hub task, `mpsc` +
    `oneshot` channels, `select!`, and a shutdown signal via `watch`.
*   **Stdlib:** —
*   **Crates:** `tokio`.
*   **Try it:** `cargo run`, then `nc 127.0.0.1 7001` in several terminals.
    Ctrl-C the server and watch the clients get told.
*   **Teaches:** async/await, tasks vs threads, `select!`, cancellation, and
    graceful shutdown.

### 17. `tunnel` — ngrok-lite reverse tunnel (capstone)

Two binaries:

- **`tunnel-relay`** is the public side. It listens on a control port
  (`:7500`) and a public port (`:8090`).
- **`tunnel-agent`** is the private side. It dials *out* to the relay
  (`HELLO <token>`).

When someone connects to the public port:

1. The relay assigns the connection an id and sends `CONNECT <id>` over the
   control channel.
2. The agent opens a fresh data connection to the relay, says `DATA <id>`,
   and connects to its local target.
3. Both sides splice with `copy_bidirectional`.

Public connections that are still waiting for their data connection sit in a
`HashMap<u64, oneshot::Sender<TcpStream>>` and time out after 5 s. A
heartbeat (`PING`/`PONG`) runs on the control connection. If the relay
disappears, the agent reconnects with exponential backoff.

*   **Patterns:** control connection vs data connections (the same design as
    FTP and frp), handing a socket between tasks with `oneshot`, timeouts,
    and backoff.
*   **Crates:** `tokio`.
*   **Try it:** start the relay with `cargo run --bin tunnel-relay`. Start a
    private service (`python3 -m http.server 18080 --bind 127.0.0.1`, or
    httpd on 8180). Then `cargo run --bin tunnel-agent -- --relay
    127.0.0.1:7500 --target 127.0.0.1:18080`, and `curl -v
    127.0.0.1:8090/` reaches that service through the tunnel.
*   **Testing:** the control-protocol codec, an end-to-end test inside one
    process (echo target + relay + agent tasks), concurrent connections
    through the tunnel, and a public connection with no agent timing out.
*   **Teaches:** nothing new on its own: it combines everything from 1–16 in
    one project.
