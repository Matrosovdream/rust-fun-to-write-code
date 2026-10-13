# metricsd — real-time metrics server

A metrics collection service with Server-Sent Events (SSE) streaming. Demonstrates
background tasks sampling data, broadcast channels for real-time updates, and
server-sent event framing.

```sh
cargo run                       # 127.0.0.1:9999
cargo test                      # metrics operations
```

## Endpoints

```sh
# List current metrics (snapshot)
curl 127.0.0.1:9999/metrics
# [{"name":"request_count","value":0},...]

# Stream metrics in real-time (SSE)
curl 127.0.0.1:9999/stream
# data: {"name":"cpu_usage","value":52.5,"timestamp":1697...}
# data: {"name":"memory_mb","value":615.0,"timestamp":1697...}
# ...

# Record a custom metric
curl -X POST -d '{"name":"custom","value":42.5}' 127.0.0.1:9999/record
# {"ok": true}
```

## Learning goals

- **Background sampling** — `tokio::spawn` for periodic tasks
- **broadcast channels** — multi-subscriber event stream
- **Server-Sent Events** — text/event-stream framing
- **tokio::time::interval** — fixed-interval tasks
- **JSON in HTTP** — parsing and formatting request/response bodies
