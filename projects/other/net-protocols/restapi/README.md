# restapi — tiny web framework + todo API

A minimal web framework on its own small HTTP/1.1 module: a `Router` with
`:param` patterns, handlers as trait objects, and middleware with a `next`
continuation (an access logger and a per-IP token-bucket rate limiter). On
top sits a JSON todo API (list, create, show, patch, delete) backed by an
in-memory `Arc<Mutex<Store>>`.

```sh
cargo run                       # 127.0.0.1:8181
cargo run -- 127.0.0.1:9999     # another address
cargo test                      # routing table, handlers, fake-clock bucket, sockets
```

## Try it

With the server running in another terminal, paste these one at a time.
(Pasted all at once they come in faster than the rate limit allows, and
the last few get a 429.)

```sh
curl -s 127.0.0.1:8181/todos
# []

curl -s -X POST -d '{"title":"write a router"}' 127.0.0.1:8181/todos
# {"id":1,"title":"write a router","done":false}

curl -i -X POST -d '{"title":"add middleware"}' 127.0.0.1:8181/todos
# HTTP/1.1 201 Created
# Content-Type: application/json
# Location: /todos/2
# Content-Length: 47
#
# {"id":2,"title":"add middleware","done":false}

curl -X PATCH -d '{"done":true}' 127.0.0.1:8181/todos/1
# {"id":1,"title":"write a router","done":true}

curl -s '127.0.0.1:8181/todos?done=true'
# [{"id":1,"title":"write a router","done":true}]

curl -i -X DELETE 127.0.0.1:8181/todos/1
# HTTP/1.1 204 No Content
```

Errors are JSON as well:

```sh
curl -s 127.0.0.1:8181/todos/1
# {"error":"not found"}
curl -s -X POST -d 'not json' 127.0.0.1:8181/todos
# {"error":"invalid JSON: expected ident at line 1 column 2"}
curl -s -X POST -d '{}' 127.0.0.1:8181/todos
# {"error":"invalid JSON: missing field `title` at line 1 column 2"}
curl -i -X PUT 127.0.0.1:8181/todos/2
# HTTP/1.1 405 Method Not Allowed
# Allow: GET, PATCH, DELETE
# ...
# {"error":"method not allowed"}
```

The rate limiter allows a burst of 10 requests per IP, then refills 5 per
second. Thirty requests in a row trip it:

```sh
for i in $(seq 30); do curl -s -o /dev/null -w '%{http_code} ' 127.0.0.1:8181/todos; done; echo
# 200 200 200 200 200 200 200 200 200 200 429 429 429 429 429 ... 429
curl -i 127.0.0.1:8181/todos        # right after the loop
# HTTP/1.1 429 Too Many Requests
# Content-Type: application/json
# Retry-After: 1
# ...
# {"error":"too many requests"}
```

The server logs every request, the 429s included, because the logger is
the outermost middleware:

```text
listening on http://127.0.0.1:8181
127.0.0.1:60533 POST /todos 201 47B 0.1ms
127.0.0.1:60539 DELETE /todos/1 204 0B 0.0ms
127.0.0.1:60864 GET /todos 429 30B 0.0ms
```

## Covers

Designing a small API: `Request`/`Response` types, `Response::json` taking
`&impl Serialize`, and status codes that carry meaning (201 + `Location`,
204 with no body and no `Content-Length`, 405 + `Allow`, 429 +
`Retry-After`). Trait objects for extensibility: a `Handler` trait with a
blanket impl for closures, stored as `Box<dyn Handler>`, and a `Middleware`
trait whose `Next` continuation runs the rest of the chain or
short-circuits it. Closures that capture an `Arc` clone (`with(&store,
list)`), `Arc<Mutex<_>>` shared state, and keeping a lock's scope small.
`serde` derive with `deny_unknown_fields` and `DeserializeOwned`. A token
bucket that takes time as a parameter and a limiter with an injected clock,
so tests control time. A thread pool built from `thread::scope`, where
every worker calls `accept()` on a borrowed listener. I/O stays at the
edges: everything but `serve` is tested without sockets.

## Rewrite exercises

1. Rewrite from scratch. Start with `match_path` and the routing table test,
   then the token bucket and its fake-clock test, then the handlers, and
   the socket glue last.
2. Add `PUT /todos/:id` (full replace: every field required) and
   `GET /todos?q=word` with real percent-decoding of the query string
   (`?q=write%20a`).
3. Persist the store to `todos.json` after every change (write to a temp
   file, then rename it into place) and load it at startup.
4. The per-IP bucket map grows forever. Evict full buckets (they hold no
   information) every N requests, and prove it with the fake clock.
5. Make the router generic over app state: `Router<S>` with handlers
   `Fn(&Request, &S) -> Response`, so `.get("/todos", with(&store, list))`
   becomes `.get("/todos", list)`. See what this changes in `Handler`,
   `Middleware`, and `Next`.
