# ssefeed — Server-Sent Events broadcast server

A live feed over plain HTTP. `GET /events` is a response that never ends: it
uses chunked encoding and sends one chunk per event. `POST /publish`
broadcasts to every subscriber. The last 100 events sit in a ring buffer, so a
client that reconnects with `Last-Event-ID` gets the ones it missed. A small
browser page built on `EventSource` is included.

```sh
cargo run                          # listens on 127.0.0.1:8182
cargo run -- 127.0.0.1:18182       # or pick another address
cargo test                         # unit tests + real-socket integration tests
```

## Try it

Terminal 1 subscribes. `-N` turns off curl's output buffering:

```sh
curl -N 127.0.0.1:8182/events
```

Terminal 2 publishes:

```sh
curl -d 'hello' 127.0.0.1:8182/publish
# published event 1
curl --data-binary $'line one\nline two' 127.0.0.1:8182/publish
# published event 2
curl -d 'disk full' '127.0.0.1:8182/publish?event=alert'
# published event 3
```

Terminal 1 prints each event as soon as it's published, then a heartbeat
comment after 15 quiet seconds:

```
id: 1
event: message
data: hello

id: 2
event: message
data: line one
data: line two

id: 3
event: alert
data: disk full

: ping
```

Reconnect as a client that last saw event 1, and the server replays the rest:

```sh
curl -N -H 'Last-Event-ID: 1' 127.0.0.1:8182/events
# id: 2 ... data: line two
# id: 3 ... data: disk full
```

`--raw` shows the chunk framing that curl usually decodes for you (0x24 = 36
bytes):

```sh
curl -si --raw -N -H 'Last-Event-ID: 2' 127.0.0.1:8182/events
# HTTP/1.1 200 OK
# Content-Type: text/event-stream
# Cache-Control: no-cache
# Transfer-Encoding: chunked
#
# 24
# id: 3
# event: alert
# data: disk full
```

Bad requests get an error status instead of crashing anything:

```sh
curl -si -X POST 127.0.0.1:8182/publish | head -1                # no body
# HTTP/1.1 411 Length Required
curl -s -d x '127.0.0.1:8182/publish?event=a+b'
# use ?event=NAME with NAME made of [A-Za-z0-9_-]
```

Open <http://127.0.0.1:8182> in two browser tabs and publish from either one,
or from curl. Messages sent with `?event=alert` show up with an orange bar,
because the page listens for that type with `addEventListener`. Stop and
restart the server, and the tabs reconnect by themselves.

## Covers

SSE wire format (`id:`/`event:`/`data:` fields, comments as heartbeats,
multi-line data). A response with no end, framed with chunked transfer
encoding. Implementing the `Write` trait for a `ChunkedWriter` adapter, and why
`BufWriter` plus `flush()` decides whether `curl -N` sees anything. Fan-out
with one `mpsc::Sender` per subscriber in an `Arc<Mutex<Hub>>`, and a
`VecDeque` ring buffer for replay. `recv_timeout` doubles as the heartbeat
timer. Dead subscribers are found when a write fails: their thread exits, the
`Receiver` drops, and `retain` prunes the `Sender` on the next publish. Read
timeouts for the request and write timeouts for the stream, `let … else` for
validation, and a heartbeat interval passed as a parameter so tests run in
milliseconds.

## Rewrite exercises

1. Rewrite from scratch. Start with `format_event` and `ChunkedWriter` and
   their tests, then the `Hub`, then the sockets.
2. Add `GET /stats` that returns the subscriber count and the last event id.
   Watch the count drop a heartbeat after you Ctrl-C a `curl -N`.
3. Send `retry: 2000` as the first chunk of every stream, and accept
   `GET /events?since=N` as an alternative to the `Last-Event-ID` header
   (for clients that can't set headers).
4. Replace `Arc<Mutex<Hub>>` with a hub thread that owns the state, as in
   websock. `publish` and `subscribe` become messages that carry a reply
   channel. Which version is easier to read?
5. Fix the slow-consumer problem: switch to `mpsc::sync_channel(64)` and use
   `try_send`, so a subscriber whose queue is full gets dropped instead of
   growing memory without bound. Then make history survive restarts by
   appending events to a file and reloading it at startup.
