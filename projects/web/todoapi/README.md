# todoapi — simple REST API

A minimal HTTP/1.1 server for a todo list with in-memory storage. Demonstrates
basic request parsing, JSON serialization, and shared mutable state with `Arc<Mutex>`.

```sh
cargo run                       # 127.0.0.1:8080
cargo test                      # store operations
```

## Endpoints

```sh
curl 127.0.0.1:8080/todos
# []

curl -X POST -d '{"title":"buy milk"}' 127.0.0.1:8080/todos
# {"id":1,"title":"buy milk","done":false}

curl 127.0.0.1:8080/todos/1
# {"id":1,"title":"buy milk","done":false}

curl -X PATCH -d '{"done":true}' 127.0.0.1:8080/todos/1
# {"id":1,"title":"buy milk","done":true}

curl -X DELETE 127.0.0.1:8080/todos/1
# {}

curl '127.0.0.1:8080/todos?all'
# []
```

## Learning goals

- **Tokio spawn** — handle each connection in a new task
- **Arc<Mutex>** — safe shared mutable state across tasks
- **HTTP parsing** — minimal request line parsing
- **JSON** — serde for serialization
- **Error handling** — Result types and early returns
