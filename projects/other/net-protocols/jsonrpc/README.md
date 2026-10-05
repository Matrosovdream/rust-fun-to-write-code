# jsonrpc — JSON-RPC 2.0 over TCP

A JSON-RPC 2.0 server that speaks newline-delimited JSON and follows the
[spec](https://www.jsonrpc.org/specification) exactly: requests,
notifications (no `id` means no reply), batches (an empty batch is
invalid, and a batch of only notifications gets no reply line at all), and
the five standard error codes. Methods live in a `Registry` of boxed
closures: `add`, `subtract`, `echo`, `now`, and `counter.incr`, whose
count is shared by every connection.

```sh
cargo run                                    # server on 127.0.0.1:7200
cargo run --bin jsonrpc-client -- add 1 2    # in another terminal
cargo test                                   # the spec's examples as a table + socket tests
```

## Try it

With the server running (`cargo run`). Replies come back with their keys
sorted, because serde_json's maps are ordered by key. JSON objects are
unordered, so this is still exactly what the spec asks for.

```sh
echo '{"jsonrpc":"2.0","method":"add","params":[1,2],"id":1}' | nc -w 2 127.0.0.1 7200
# {"id":1,"jsonrpc":"2.0","result":3}
echo '{"jsonrpc":"2.0","method":"subtract","params":{"minuend":42,"subtrahend":23},"id":2}' | nc -w 2 127.0.0.1 7200
# {"id":2,"jsonrpc":"2.0","result":19}
```

A notification followed by a request on the same connection: the
notification runs (the counter goes to 5), but only the request is
answered.

```sh
printf '%s\n' '{"jsonrpc":"2.0","method":"counter.incr","params":[5]}' \
              '{"jsonrpc":"2.0","method":"counter.incr","id":3}' | nc -w 2 127.0.0.1 7200
# {"id":3,"jsonrpc":"2.0","result":6}
```

Batches: one reply per request, none for the notification in the middle.

```sh
echo '[{"jsonrpc":"2.0","method":"echo","params":["hi"],"id":"a"},{"jsonrpc":"2.0","method":"echo","params":["shh"]},{"foo":"boo"},{"jsonrpc":"2.0","method":"nope","id":"b"}]' | nc -w 2 127.0.0.1 7200
# [{"id":"a","jsonrpc":"2.0","result":["hi"]},{"error":{"code":-32600,"message":"Invalid Request"},"id":null,"jsonrpc":"2.0"},{"error":{"code":-32601,"message":"Method not found"},"id":"b","jsonrpc":"2.0"}]
echo '[{"jsonrpc":"2.0","method":"echo"},{"jsonrpc":"2.0","method":"counter.incr"}]' | nc -w 2 127.0.0.1 7200
# (nothing: all notifications)
echo '[]' | nc -w 2 127.0.0.1 7200
# {"error":{"code":-32600,"message":"Invalid Request"},"id":null,"jsonrpc":"2.0"}
echo 'hello?' | nc -w 2 127.0.0.1 7200
# {"error":{"code":-32700,"message":"Parse error"},"id":null,"jsonrpc":"2.0"}
```

The client parses each parameter as JSON when it can, and sends it as a
string otherwise:

```sh
cargo run --bin jsonrpc-client -- add 1 2
# 3
cargo run --bin jsonrpc-client -- echo hi 2 '{"k":[true]}'
# ["hi",2,{"k":[true]}]
cargo run --bin jsonrpc-client -- counter.incr          # 8: the silent batch above bumped it too
# 8
cargo run --bin jsonrpc-client -- add 1 x
# error -32602: Invalid params ("expected two integers: [x, y] or {\"a\": x, \"b\": y}")
cargo run --bin jsonrpc-client -- add 9223372036854775807 1
# error -32603: Internal error ("integer overflow")
cargo run --bin jsonrpc-client -- nope
# error -32601: Method not found
```

Or talk to it interactively with `nc 127.0.0.1 7200`, one JSON object per
line.

## Covers

`Box<dyn Fn(Value) -> Result<Value, RpcError> + Send + Sync>` stored in a
`HashMap`, so closures that capture different state share one type;
generic `register<F: Fn… + 'static>`; why `Send + Sync` is needed when an
`Arc<Registry>` is shared across threads; a `move` closure owning an
`Arc<Mutex<i64>>`; dynamic JSON with `serde_json::Value`, `json!`, and
`as_i64`/`as_str`; `Option` and `?` for validation (`let id = id?;` is
how a notification stays silent); `filter_map` over a batch; checked
arithmetic instead of overflow panics; bounding a line with
`by_ref().take(…)` + `read_until` instead of `lines()`; and implementing a
spec to the letter, with its own examples as the test table.

## Rewrite exercises

1. Rewrite from scratch, starting from the spec-examples table in
   `src/lib.rs`, then make it pass one row at a time.
2. Add `divide`, which returns `-32602` with `"data": "division by zero"`,
   and `sum`, which takes any number of positional params.
3. Make the `Handler` trait-based: `trait Method { fn call(&self, params:
   Value) -> Result<Value, RpcError>; }` with a blanket impl for closures,
   and register a struct that carries its own state.
4. Run each batch element on its own thread and collect the replies (the
   spec allows any order). Check the batch test still passes, then keep the
   input order anyway.
5. Add server-to-client notifications: a `subscribe` method after which
   the server sends `{"jsonrpc":"2.0","method":"tick","params":[n]}` every
   second on that connection. The connection now needs a writer that both
   the request loop and a timer thread can use.
