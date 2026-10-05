# echoserver — TCP echo server + client

Line-based TCP server: echoes input, answers `/time`, `/stats`, `/quit`;
one thread per connection; counters shared behind `Arc<Mutex<_>>`. A tiny
client binary is included.

```sh
cargo run --bin server            # terminal 1
cargo run --bin client            # terminal 2
cargo test                        # includes real-socket integration tests
```

## Covers

`TcpListener`/`TcpStream`, `try_clone` to split read/write halves, thread
per connection, `Arc<Mutex<Stats>>`, multiple `[[bin]]` targets sharing a
`lib.rs`, integration tests binding port 0.

## Rewrite exercises

1. Rewrite from scratch; keep `respond` free of any socket code.
2. Add `/name alice`: greet by name for the rest of the connection
   (per-connection state vs shared state — feel the difference).
3. Turn it into a broadcast chat: every line goes to all connected clients
   (you'll need a shared list of senders).
4. Add a connection limit: refuse the 11th client politely.
5. Replace thread-per-connection with a fixed worker pool taking accepted
   sockets off a channel — reuse the hashr pattern.
