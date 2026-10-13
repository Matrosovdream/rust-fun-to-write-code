# projects/web — async server patterns

Four tokio-based HTTP/WebSocket servers at simple-to-middle complexity level,
demonstrating core async patterns: request handling, shared state, streaming,
and real-time communication.

```sh
cargo test --workspace           # all tests
cargo run -p todoapi            # todo list API, port 8080
cargo run -p chatws             # chat server, port 8888 (WebSocket)
cargo run -p fileserve          # file server, port 3333
cargo run -p metricsd           # metrics server, port 9999
```

## Project overview

### 1. **todoapi** — REST API with in-memory store
- Minimal HTTP/1.1 server handling GET/POST/PATCH/DELETE
- `Arc<Mutex>` for thread-safe shared state
- JSON serialization with serde
- Basic request parsing and routing

**Key learning:** task-per-connection, spawn loops, shared mutable state

### 2. **chatws** — WebSocket chat with rooms
- WebSocket upgrade and message framing (tokio-tungstenite)
- `broadcast::channel` for one-to-many message distribution
- `tokio::select!` for multiplexing socket and channel
- Multiple rooms, user joining/switching

**Key learning:** WebSocket protocol, broadcast channels, multiplexing

### 3. **fileserve** — HTTP file server with upload
- Directory listing as HTML
- File download and multipart upload
- Path traversal protection
- Async filesystem I/O (tokio::fs)
- Content-type detection

**Key learning:** filesystem operations, binary responses, path validation

### 4. **metricsd** — Real-time metrics with SSE
- Background sampling task (`tokio::spawn` + `tokio::time::interval`)
- Server-Sent Events streaming
- Metrics snapshot endpoint
- Custom metric recording
- broadcast channels for real-time updates

**Key learning:** background tasks, Server-Sent Events, periodic intervals

## Quick start

Pick a project and cd into it:

```sh
cd todoapi && cargo run
```

Then in another terminal, try the examples in that project's README.

## Architecture patterns

| Pattern | Where | What it teaches |
| --- | --- | --- |
| Loop spawn | todoapi, fileserve | handling each connection in a task |
| Arc<Mutex> | todoapi | safe shared mutable state |
| broadcast | chatws, metricsd | one-to-many messaging |
| tokio::select! | chatws | multiplexing async sources |
| tokio::fs | fileserve | non-blocking filesystem I/O |
| tokio::spawn + interval | metricsd | background periodic tasks |
| WebSocket | chatws | protocol upgrade, framing |
| SSE | metricsd | streaming text events |

## Testing

Each project has basic unit tests for core logic:

```sh
cargo test -p todoapi --lib     # store operations
cargo test -p fileserve --lib   # path validation
cargo test -p metricsd --lib    # collector operations
```

No integration tests yet (those would need live servers and client libraries).

## Next steps

- Add clap for CLI args (listen address, port)
- Error handling: define custom error types instead of Box<dyn Error>
- Logging: add tracing or log crate for observability
- Graceful shutdown: SIGTERM handling
- Connection pools or limits
- HTTPS/TLS support
- Database backend instead of in-memory storage
