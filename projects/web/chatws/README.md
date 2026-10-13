# chatws — WebSocket chat with rooms

A WebSocket server that broadcasts messages within rooms. Demonstrates async I/O,
broadcast channels, and multiplexing with `tokio::select!`.

```sh
cargo run                       # ws://127.0.0.1:8888
```

## Try it

Install `websocat` or `wscat` to test:

```sh
# Terminal 1 (server)
cargo run

# Terminal 2 (user A, general room)
wscat -c ws://127.0.0.1:8888/room/general
# Type: {"text":"hello room"}

# Terminal 3 (user B, general room)
wscat -c ws://127.0.0.1:8888/room/general
# Type: {"text":"hi there!"}

# Switching rooms in terminal 2
# Type: {"room":"random"}
# Type: {"text":"hello random!"}

# Terminal 4 (user C, random room)
wscat -c ws://127.0.0.1:8888/room/random
```

## Learning goals

- **WebSocket** — tokio-tungstenite for upgrades and framing
- **broadcast channels** — one-to-many message distribution
- **tokio::select!** — multiplexing between socket and channel
- **Arc cloning** — sharing server state across tasks
- **JSON** — parsing message frames
