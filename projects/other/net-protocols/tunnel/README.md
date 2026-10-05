# tunnel — ngrok-lite reverse tunnel

Expose a service on a private machine through a public one. `tunnel-agent`
runs next to the private service and dials *out* to `tunnel-relay`
(`HELLO <token>`), then keeps that control connection alive with
`PING`/`PONG`. When someone connects to the relay's public port, the relay
sends `CONNECT <id>`. The agent opens a fresh data connection (`DATA <id>`),
connects to the local target, and both sides splice with
`copy_bidirectional`. If the relay disappears, the agent reconnects with
exponential backoff. The capstone: it combines everything from projects 1–16.
The full design (diagram, step-by-step sequence, protocol table) is in the
`src/lib.rs` doc comment.

```sh
cargo run --bin tunnel-relay        # control 127.0.0.1:7500, public 127.0.0.1:8090
cargo run --bin tunnel-agent -- --relay 127.0.0.1:7500 --target 127.0.0.1:18080
cargo run --bin tunnel-relay -- --control 127.0.0.1:7500 --public 127.0.0.1:8090 --token s3cret
cargo test                          # codec unit tests + in-process end-to-end tests
```

## Try it

```sh
# terminal 1: the private service (httpd on 127.0.0.1:8180 works as a target too)
mkdir -p /tmp/www && echo "hello from behind the tunnel" > /tmp/www/hello.txt
cd /tmp/www && python3 -m http.server 18080 --bind 127.0.0.1

# terminal 2: the public side
cargo run --bin tunnel-relay
# tunnel-relay listening on 127.0.0.1:7500 (control) and 127.0.0.1:8090 (public)

# terminal 3: the private side
cargo run --bin tunnel-agent -- --relay 127.0.0.1:7500 --target 127.0.0.1:18080
# tunnel-agent: relay 127.0.0.1:7500, target 127.0.0.1:18080
# connected to relay 127.0.0.1:7500, forwarding to 127.0.0.1:18080

# terminal 4: a public client
curl -v 127.0.0.1:8090/
# > GET / HTTP/1.1
# > Host: 127.0.0.1:8090
# < HTTP/1.0 200 OK
# < Server: SimpleHTTP/0.6 Python/3.9.6
# <!DOCTYPE HTML PUBLIC ... (python's directory listing)
curl 127.0.0.1:8090/hello.txt
# hello from behind the tunnel
```

Each request shows up on both sides:

```text
relay:  [127.0.0.1:60479] agent connected
        [127.0.0.1:60480] public connection #0: asking the agent
        [127.0.0.1:60480] #0 done: 77 bytes in, 494 bytes out
agent:  #0 done: 77 bytes to 127.0.0.1:18080, 494 bytes back
```

**Kill the relay** (Ctrl-C in terminal 2), wait a few seconds, and start it
again. The agent backs off, then reconnects on its own:

```text
relay 127.0.0.1:7500: relay closed the connection; retrying in 250ms
relay 127.0.0.1:7500: Connection refused (os error 61); retrying in 500ms
relay 127.0.0.1:7500: Connection refused (os error 61); retrying in 1s
relay 127.0.0.1:7500: Connection refused (os error 61); retrying in 2s
relay 127.0.0.1:7500: Connection refused (os error 61); retrying in 4s
connected to relay 127.0.0.1:7500, forwarding to 127.0.0.1:18080
```

A curl sent while the agent is still backing off doesn't fail. The relay
logs `public connection #0: no agent yet, waiting`, and the request goes
through as soon as the agent is back (we saw it take 2.3 s).

**Stop the agent** and curl again. After 5 s:
`curl: (56) Recv failure: Connection reset by peer`, and the relay logs
`#1 timed out waiting for the agent, closing`. It's a reset rather than a
clean close because the relay never read curl's request.

**Wrong token:** `cargo run --bin tunnel-agent -- --target 127.0.0.1:18080
--token nope` prints `tunnel-agent: relay refused us: bad token` and exits 1.

**Speak the protocol yourself:** `nc 127.0.0.1 7500`, type `HELLO dev` (you
get `OK`) and `PING` (you get `PONG`). Then stop typing: after 15 s the
relay logs `agent dropped: no PING for too long` and hangs up. Typing
`GET / HTTP/1.1` or `DATA 12345` as the first line just gets you
disconnected.

## Covers

The control/data connection split (as in FTP and frp), a newline-delimited
control protocol with `Display`/`FromStr` as a round-trip pair, handing a
`TcpStream` from one task to another through a `oneshot`, a
`HashMap<u64, oneshot::Sender<TcpStream>>` of pending connections behind a
`std::sync::Mutex` (and why it must never be held across `.await`),
`tokio::io::copy_bidirectional` with half-close, `select!` over a channel, a
socket, and a timer, cancel-safe line reading, `sleep` + `reset` for
heartbeat deadlines, `interval` for pings, `tokio::time::timeout` everywhere
a peer could stall, exponential backoff with saturating arithmetic, reading
a first line without over-reading, `JoinSet` so aborting the relay takes
every connection with it, and in-process end-to-end tests.

## Rewrite exercises

1. Rewrite from scratch, starting with the codec (`Msg`, its round-trip
   test, `MsgReader`), then the relay tested with a hand-driven fake agent,
   then the agent.
2. Add jitter to the backoff (±20%) so a hundred agents restarting together
   don't hit the relay in lockstep. Keep `backoff` pure by passing the random
   number in.
3. Multiple tunnels: `HELLO <token> <name>`, and the relay routes by `Host:`
   header (`curl -H 'Host: blog.localhost' 127.0.0.1:8090`) to the agent
   that registered that name. You'll have to peek at the request without
   consuming it before you can splice.
4. Limit the pending map (at most 100 waiting clients, refuse the rest at
   once) and add per-tunnel stats (`bytes in/out`, active connections) shown
   by the relay on a third, admin port, as loadbal does.
5. Multiplex all streams over the one control connection instead of opening
   data connections: frames of `[stream id: u32][len: u16][bytes]`, plus a
   window for flow control so one slow stream can't block the others. Then
   compare the code size with the data-connection design.
