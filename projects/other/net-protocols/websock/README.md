# websock — WebSocket chat server + terminal client

RFC 6455 from raw TCP. An HTTP `Upgrade` handshake answered with
`101 Switching Protocols`, then a binary frame protocol: FIN/opcode bits,
7/16/64-bit lengths, client masking, fragmentation, ping/pong, and the close
handshake. Text messages are broadcast to everyone in the room. Includes a
browser chat page and a terminal client.

```sh
cargo run                          # server on 127.0.0.1:8183
cargo run --bin websock-client     # terminal client (default 127.0.0.1:8183)
cargo test                         # unit tests + real-socket integration tests
```

## Try it

Start the server with `cargo run` in terminal 1, then join from terminal 2:

```sh
cargo run --bin websock-client
# connected to ws://127.0.0.1:8183/ws (type a line to send it, Ctrl-D to quit)
# * user1 joined (1 online)
```

From terminal 3, pipe one message in. The client sends it, then closes
cleanly when stdin ends:

```sh
echo 'hello from a pipe' | cargo run --bin websock-client
# connected to ws://127.0.0.1:8183/ws (type a line to send it, Ctrl-D to quit)
# * user2 joined (2 online)
# * connection closed, code 1000
```

Terminal 2 prints it (names are connection numbers, so yours may differ):

```
* user2 joined (2 online)
user2: hello from a pipe
* user2 left
```

Check the handshake by hand with curl and the RFC's sample key. The accept
value must be `s3pPLMBiTxaQ9kYGzzhZRbK+xOo=`. `-m 2` makes curl give up
after two seconds, because it doesn't speak WebSocket:

```sh
curl -i -N -m 2 --http1.1 -H 'Connection: Upgrade' -H 'Upgrade: websocket' \
  -H 'Sec-WebSocket-Version: 13' -H 'Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==' \
  http://127.0.0.1:8183/ws
# HTTP/1.1 101 Switching Protocols
# Upgrade: websocket
# Connection: Upgrade
# Sec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=
#
# �* user3 joined (2 online)     <- a raw text frame: 0x81, 0x19, then 25 bytes
# curl: (28) Operation timed out after 2002 milliseconds with 27 bytes received
```

A plain GET (no upgrade headers) is refused:

```sh
curl -i 127.0.0.1:8183/ws
# HTTP/1.1 400 Bad Request
# ...
# expected Upgrade: websocket and Connection: Upgrade
```

Now open <http://127.0.0.1:8183> in two browser tabs and chat between them
and the terminal client. Restart the server and the tabs reconnect on their
own.

The server logs one line per request and one when a chat connection ends.
curl (user3) just hung up, without the close handshake:

```
127.0.0.1:62331 GET /ws HTTP/1.1
127.0.0.1:62332 GET /ws HTTP/1.1
127.0.0.1:62332 user2 left: closed by client, code 1000
127.0.0.1:62333 GET /ws HTTP/1.1
127.0.0.1:62333 user3 left: hung up without a close frame
```

## Covers

The HTTP Upgrade handshake and `base64(sha1(key + GUID))`. Why the
`BufReader` that read the head must keep reading frames: it may already hold
some. Bit-level framing with masks and shifts, a `#[repr(u8)]` opcode enum,
big-endian `to_be_bytes`/`from_be_bytes`, XOR masking. Refusing a huge length
before allocating. A frame codec over `impl Read`/`impl Write`, tested on byte
slices and on a reader that returns one byte at a time. Message reassembly as
a two-state machine, slice patterns (`[high, low, reason @ ..]`) for close
payloads, and close codes 1002/1007/1009. Threads: per client, a reader plus a
writer draining an `mpsc` outbox, so frames never interleave, and a hub
thread that owns the client list (actor style, no Mutex). The client instead
shares a `Mutex<Connection>` between two threads. Pings from the writer
replace a read timeout as the liveness check. Random masking keys from
`RandomState`, with no crate.

## Rewrite exercises

1. Rewrite from scratch. Start with `frame.rs` and get the RFC §5.7 example
   tests passing before you open a socket.
2. Nicknames: a message `/nick alice` renames you in the hub (hub state, not
   connection state) and broadcasts `* user3 is now alice`.
3. Check `Origin`: browsers send it on the upgrade request. Refuse anything
   that isn't `http://127.0.0.1:8183` or `http://localhost:8183` with 403,
   then read up on cross-site WebSocket hijacking to see why it matters.
4. Fail fast on bad UTF-8: validate each fragment as it arrives instead of
   the whole message at the end. `Utf8Error::error_len()` tells "invalid"
   apart from "incomplete, more bytes coming".
5. Backpressure: swap the unbounded outbox for `mpsc::sync_channel(32)`. When
   a slow client's outbox is full, the hub should close it with code 1008
   instead of blocking the whole room. Then make the writer split messages
   over 4 KiB into continuation frames.
