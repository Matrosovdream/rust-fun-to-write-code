//! jsonrpc — JSON-RPC 2.0 over TCP, one JSON text per line.
//!
//! [JSON-RPC 2.0](https://www.jsonrpc.org/specification) is a tiny RPC
//! protocol: the client sends a *request object* and the server answers
//! with a *response object*. The spec says nothing about transport. Here,
//! every message is one line of JSON ending in `\n`; JSON never needs a raw
//! newline inside a value, so that's unambiguous.
//!
//! ```text
//! --> {"jsonrpc":"2.0","method":"subtract","params":[42,23],"id":1}
//! <-- {"id":1,"jsonrpc":"2.0","result":19}
//! --> {"jsonrpc":"2.0","method":"nope","id":2}
//! <-- {"error":{"code":-32601,"message":"Method not found"},"id":2,"jsonrpc":"2.0"}
//! ```
//!
//! (serde_json writes object keys in sorted order. JSON objects are
//! unordered, so that's still exactly the reply the spec asks for.)
//!
//! The rules implemented here, straight from the spec:
//!
//! - `jsonrpc` must be exactly `"2.0"` and `method` must be a string.
//!   `params` may be left out; if present it must be an array (positional)
//!   or an object (named).
//! - `id` is a string, a number, or null. A request with *no* `id` member
//!   is a **notification**: the server runs it but never replies, not even
//!   with an error.
//! - A JSON array is a **batch**: each element is handled on its own and
//!   the replies come back together as one array. An empty array is an
//!   Invalid Request. A batch of nothing but notifications gets no reply at
//!   all (not even `[]`).
//! - When the id can't be determined (a parse error, a request that isn't
//!   an object, a bad `id`), the error reply carries `"id": null`.
//!
//! | code   | message          | when                                        |
//! |--------|------------------|---------------------------------------------|
//! | -32700 | Parse error      | the line isn't valid JSON                   |
//! | -32600 | Invalid Request  | valid JSON, but not a valid request object  |
//! | -32601 | Method not found | nothing registered under that name          |
//! | -32602 | Invalid params   | the method didn't like its params           |
//! | -32603 | Internal error   | the method failed on our side (here: i64 overflow) |
//!
//! Methods: `add` and `subtract` (`[a, b]`, `{"a","b"}`, or
//! `{"minuend","subtrahend"}`), `echo` (returns its params), `now` (unix
//! seconds), and `counter.incr` (a counter shared by every connection).

use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

pub const PARSE_ERROR: i64 = -32700;
pub const INVALID_REQUEST: i64 = -32600;
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INVALID_PARAMS: i64 = -32602;
pub const INTERNAL_ERROR: i64 = -32603;

/// Longest line we'll buffer, newline included.
pub const MAX_LINE: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    /// Optional extra detail; the spec lets the server put anything here.
    pub data: Option<Value>,
}

impl RpcError {
    fn new(code: i64, message: &str) -> Self {
        RpcError {
            code,
            message: message.to_string(),
            data: None,
        }
    }

    pub fn invalid_params(detail: &str) -> Self {
        RpcError {
            data: Some(detail.into()),
            ..Self::new(INVALID_PARAMS, "Invalid params")
        }
    }

    pub fn internal(detail: &str) -> Self {
        RpcError {
            data: Some(detail.into()),
            ..Self::new(INTERNAL_ERROR, "Internal error")
        }
    }

    fn to_json(&self) -> Value {
        let mut error = json!({ "code": self.code, "message": self.message });
        if let Some(data) = &self.data {
            error["data"] = data.clone();
        }
        error
    }
}

fn error_reply(id: Value, error: &RpcError) -> Value {
    json!({ "jsonrpc": "2.0", "error": error.to_json(), "id": id })
}

/// What every method looks like. `Box<dyn Fn>` lets closures that capture
/// different things (nothing, a counter, …) live in one map. `Send + Sync`
/// because one `Registry` is shared by all connection threads, which may
/// call the same handler at the same time.
pub type Handler = Box<dyn Fn(Value) -> Result<Value, RpcError> + Send + Sync>;

#[derive(Default)]
pub struct Registry {
    methods: HashMap<String, Handler>,
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    /// `'static` because the closure is stored in the map and must not
    /// borrow anything that could be dropped before the registry is.
    pub fn register<F>(&mut self, name: &str, handler: F)
    where
        F: Fn(Value) -> Result<Value, RpcError> + Send + Sync + 'static,
    {
        self.methods.insert(name.to_string(), Box::new(handler));
    }

    /// One line in, at most one line out. `None` means "send nothing back".
    pub fn handle_line(&self, line: &str) -> Option<String> {
        let reply = match serde_json::from_str::<Value>(line) {
            Err(_) => Some(error_reply(
                Value::Null,
                &RpcError::new(PARSE_ERROR, "Parse error"),
            )),
            Ok(Value::Array(batch)) if batch.is_empty() => Some(invalid_request(Value::Null)),
            Ok(Value::Array(batch)) => {
                // `filter_map` drops the `None`s, i.e. the notifications.
                let replies: Vec<Value> = batch
                    .into_iter()
                    .filter_map(|r| self.handle_one(r))
                    .collect();
                (!replies.is_empty()).then_some(Value::Array(replies))
            }
            Ok(single) => self.handle_one(single),
        };
        // `Value`'s Display is compact JSON on one line, and can't fail.
        reply.map(|v| v.to_string())
    }

    fn handle_one(&self, request: Value) -> Option<Value> {
        // Look at the id first, so even an error reply can echo it.
        let id = match request.get("id") {
            None => None,
            Some(id @ (Value::Null | Value::String(_) | Value::Number(_))) => Some(id.clone()),
            Some(_) => return Some(invalid_request(Value::Null)),
        };
        let Some((method, params)) = parse_request(&request) else {
            // Invalid requests are answered even without an id: we can't
            // trust that something malformed meant to be a notification.
            return Some(invalid_request(id.unwrap_or(Value::Null)));
        };
        let outcome = match self.methods.get(method) {
            Some(handler) => handler(params),
            None => Err(RpcError::new(METHOD_NOT_FOUND, "Method not found")),
        };
        // A notification: the work is done (even if it failed), but the
        // spec says we stay silent. `?` on an Option returns None early.
        let id = id?;
        Some(match outcome {
            Ok(result) => json!({ "jsonrpc": "2.0", "result": result, "id": id }),
            Err(error) => error_reply(id, &error),
        })
    }
}

fn invalid_request(id: Value) -> Value {
    error_reply(id, &RpcError::new(INVALID_REQUEST, "Invalid Request"))
}

/// The request-object rules (spec section 4). Returns the method name and
/// the params; left-out params become `Null`, so a handler always gets an
/// array, an object, or null.
fn parse_request(request: &Value) -> Option<(&str, Value)> {
    let object = request.as_object()?;
    if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return None;
    }
    let method = object.get("method")?.as_str()?;
    let params = match object.get("params") {
        None => Value::Null,
        Some(p @ (Value::Array(_) | Value::Object(_))) => p.clone(),
        Some(_) => return None,
    };
    Some((method, params))
}

/// The two param styles the spec allows: `[x, y]` or `{"<n0>": x, "<n1>": y}`.
fn two_ints(params: &Value, names: [&str; 2]) -> Result<(i64, i64), RpcError> {
    let (a, b) = match params {
        Value::Array(items) if items.len() == 2 => (items[0].as_i64(), items[1].as_i64()),
        Value::Object(map) => (
            map.get(names[0]).and_then(Value::as_i64),
            map.get(names[1]).and_then(Value::as_i64),
        ),
        _ => (None, None),
    };
    match (a, b) {
        (Some(a), Some(b)) => Ok((a, b)),
        _ => Err(RpcError::invalid_params(&format!(
            "expected two integers: [x, y] or {{\"{}\": x, \"{}\": y}}",
            names[0], names[1]
        ))),
    }
}

fn overflow() -> RpcError {
    RpcError::internal("integer overflow")
}

/// The server's methods.
pub fn default_registry() -> Registry {
    let mut registry = Registry::new();
    registry.register("add", |p| {
        let (a, b) = two_ints(&p, ["a", "b"])?;
        a.checked_add(b).map(Value::from).ok_or_else(overflow)
    });
    registry.register("subtract", |p| {
        let (a, b) = two_ints(&p, ["minuend", "subtrahend"])?;
        a.checked_sub(b).map(Value::from).ok_or_else(overflow)
    });
    // `Ok` is itself a function from Value to Result<Value, _>, so it is
    // already a valid handler: the params come straight back.
    registry.register("echo", Ok);
    registry.register("now", |_| {
        let since_epoch = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        Ok(json!(since_epoch.as_secs()))
    });

    // `move` hands this Arc to the closure. The registry is shared by every
    // connection, so they all bump the same number.
    let counter = Arc::new(Mutex::new(0i64));
    registry.register("counter.incr", move |p| {
        let by = match &p {
            Value::Null => Some(1),
            Value::Array(items) if items.len() == 1 => items[0].as_i64(),
            Value::Object(map) => map.get("by").and_then(Value::as_i64),
            _ => None,
        };
        let by =
            by.ok_or_else(|| RpcError::invalid_params("expected no params, [n], or {\"by\": n}"))?;
        let mut count = counter.lock().expect("counter lock");
        *count = count.checked_add(by).ok_or_else(overflow)?;
        Ok(json!(*count))
    });
    registry
}

/// Accept loop: one thread per connection, all sharing one registry.
pub fn serve(listener: TcpListener, registry: Arc<Registry>, idle_timeout: Duration) {
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let registry = Arc::clone(&registry);
                thread::spawn(move || {
                    if let Err(e) = handle(stream, &registry, idle_timeout) {
                        eprintln!("connection error: {e}");
                    }
                });
            }
            Err(e) => eprintln!("accept error: {e}"),
        }
    }
}

fn handle(stream: TcpStream, registry: &Registry, idle_timeout: Duration) -> io::Result<()> {
    let peer = stream.peer_addr()?;
    // A silent client would otherwise hold this thread forever.
    stream.set_read_timeout(Some(idle_timeout))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut writer = stream;
    eprintln!("[{peer}] connected");

    let mut buf = Vec::new();
    loop {
        buf.clear();
        // `BufRead::lines` would buffer a line of any length. `take` caps
        // it, so a peer that never sends '\n' can't eat all our memory.
        // `by_ref` lends the reader to `take` instead of giving it away.
        match reader
            .by_ref()
            .take(MAX_LINE as u64)
            .read_until(b'\n', &mut buf)
        {
            Ok(0) => break,
            Ok(_) => {}
            Err(e) if is_timeout(&e) => {
                eprintln!("[{peer}] idle timeout, closing");
                return Ok(());
            }
            Err(e) => return Err(e),
        }
        if buf.len() == MAX_LINE && !buf.ends_with(b"\n") {
            // We can't find where this message ends, so we can't recover.
            eprintln!("[{peer}] line too long, closing");
            writeln!(writer, "{}", parse_error_with("line too long"))?;
            return Ok(());
        }
        let reply = match std::str::from_utf8(&buf) {
            // Blank lines are harmless (say, an extra Enter in nc): skip them.
            Ok(line) if line.trim().is_empty() => continue,
            Ok(line) => {
                // `{:.100}` truncates the logged line to 100 characters.
                eprintln!("[{peer}] {:.100}", line.trim_end());
                registry.handle_line(line)
            }
            Err(_) => Some(parse_error_with("invalid UTF-8")),
        };
        if let Some(reply) = reply {
            writeln!(writer, "{reply}")?;
        }
    }
    eprintln!("[{peer}] disconnected");
    Ok(())
}

/// An expired read timeout: Unix reports it as WouldBlock, Windows as
/// TimedOut.
fn is_timeout(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
    )
}

/// A Parse error reply line for input we reject before it reaches the
/// JSON parser.
fn parse_error_with(detail: &str) -> String {
    let error = RpcError {
        data: Some(detail.into()),
        ..RpcError::new(PARSE_ERROR, "Parse error")
    };
    error_reply(Value::Null, &error).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs a script in the spec's own notation: `-->` is a line we send,
    /// `<--` the reply we expect, and a `-->` with no `<--` after it must
    /// get no reply at all. Other lines are comments. Replies are compared
    /// as parsed JSON, so key order and spacing don't matter. Returns how
    /// many requests ran, so a typo can't silently skip half the script.
    fn run_script(registry: &Registry, script: &str) -> usize {
        let parse = |s: &str| serde_json::from_str::<Value>(s).unwrap();
        let mut lines = script
            .lines()
            .filter(|l| l.starts_with("--> ") || l.starts_with("<-- "))
            .peekable();
        let mut count = 0;
        while let Some(line) = lines.next() {
            let request = line.strip_prefix("--> ").expect("<-- without a -->");
            let expected = lines.next_if(|l| l.starts_with("<-- ")).map(|l| &l[4..]);
            let got = registry.handle_line(request);
            assert_eq!(got.as_deref().map(parse), expected.map(parse), "{request}");
            count += 1;
        }
        count
    }

    /// Section 7 of the spec, "Examples", word for word. Multi-line
    /// examples are joined onto one line, because a line is our message
    /// boundary.
    const SPEC_EXAMPLES: &str = r#"
rpc call with positional parameters:
--> {"jsonrpc": "2.0", "method": "subtract", "params": [42, 23], "id": 1}
<-- {"jsonrpc": "2.0", "result": 19, "id": 1}
--> {"jsonrpc": "2.0", "method": "subtract", "params": [23, 42], "id": 2}
<-- {"jsonrpc": "2.0", "result": -19, "id": 2}

rpc call with named parameters:
--> {"jsonrpc": "2.0", "method": "subtract", "params": {"subtrahend": 23, "minuend": 42}, "id": 3}
<-- {"jsonrpc": "2.0", "result": 19, "id": 3}
--> {"jsonrpc": "2.0", "method": "subtract", "params": {"minuend": 42, "subtrahend": 23}, "id": 4}
<-- {"jsonrpc": "2.0", "result": 19, "id": 4}

a Notification:
--> {"jsonrpc": "2.0", "method": "update", "params": [1,2,3,4,5]}
--> {"jsonrpc": "2.0", "method": "foobar"}

rpc call of non-existent method:
--> {"jsonrpc": "2.0", "method": "foobar", "id": "1"}
<-- {"jsonrpc": "2.0", "error": {"code": -32601, "message": "Method not found"}, "id": "1"}

rpc call with invalid JSON:
--> {"jsonrpc": "2.0", "method": "foobar, "params": "bar", "baz]
<-- {"jsonrpc": "2.0", "error": {"code": -32700, "message": "Parse error"}, "id": null}

rpc call with invalid Request object:
--> {"jsonrpc": "2.0", "method": 1, "params": "bar"}
<-- {"jsonrpc": "2.0", "error": {"code": -32600, "message": "Invalid Request"}, "id": null}

rpc call Batch, invalid JSON:
--> [{"jsonrpc": "2.0", "method": "sum", "params": [1,2,4], "id": "1"}, {"jsonrpc": "2.0", "method"]
<-- {"jsonrpc": "2.0", "error": {"code": -32700, "message": "Parse error"}, "id": null}

rpc call with an empty Array:
--> []
<-- {"jsonrpc": "2.0", "error": {"code": -32600, "message": "Invalid Request"}, "id": null}

rpc call with an invalid Batch (but not empty):
--> [1]
<-- [{"jsonrpc": "2.0", "error": {"code": -32600, "message": "Invalid Request"}, "id": null}]

rpc call with invalid Batch:
--> [1,2,3]
<-- [{"jsonrpc": "2.0", "error": {"code": -32600, "message": "Invalid Request"}, "id": null}, {"jsonrpc": "2.0", "error": {"code": -32600, "message": "Invalid Request"}, "id": null}, {"jsonrpc": "2.0", "error": {"code": -32600, "message": "Invalid Request"}, "id": null}]

rpc call Batch:
--> [{"jsonrpc": "2.0", "method": "sum", "params": [1,2,4], "id": "1"}, {"jsonrpc": "2.0", "method": "notify_hello", "params": [7]}, {"jsonrpc": "2.0", "method": "subtract", "params": [42,23], "id": "2"}, {"foo": "boo"}, {"jsonrpc": "2.0", "method": "foo.get", "params": {"name": "myself"}, "id": "5"}, {"jsonrpc": "2.0", "method": "get_data", "id": "9"}]
<-- [{"jsonrpc": "2.0", "result": 7, "id": "1"}, {"jsonrpc": "2.0", "result": 19, "id": "2"}, {"jsonrpc": "2.0", "error": {"code": -32600, "message": "Invalid Request"}, "id": null}, {"jsonrpc": "2.0", "error": {"code": -32601, "message": "Method not found"}, "id": "5"}, {"jsonrpc": "2.0", "result": ["hello", 5], "id": "9"}]

rpc call Batch (all notifications):
--> [{"jsonrpc": "2.0", "method": "notify_sum", "params": [1,2,4]}, {"jsonrpc": "2.0", "method": "notify_hello", "params": [7]}]
(Nothing is returned for all notification batches)
"#;

    #[test]
    fn every_example_from_the_spec() {
        // The spec's batch example also calls `sum` and `get_data`.
        // Registering them is exactly what any user of this library does.
        let mut registry = default_registry();
        registry.register("sum", |p| {
            // Summing Options gives None as soon as one isn't an i64.
            let total: Option<i64> = p.as_array().into_iter().flatten().map(Value::as_i64).sum();
            total
                .map(Value::from)
                .ok_or_else(|| RpcError::invalid_params("expected [ints]"))
        });
        registry.register("get_data", |_| Ok(json!(["hello", 5])));
        assert_eq!(run_script(&registry, SPEC_EXAMPLES), 15);
    }

    /// Our own methods, and the edge cases the spec's examples skip. One
    /// registry runs the whole script, so the counter carries over.
    const OUR_EXAMPLES: &str = r#"
both param styles, and a null id (still a request, not a notification):
--> {"jsonrpc":"2.0","method":"add","params":[1,2],"id":1}
<-- {"jsonrpc":"2.0","result":3,"id":1}
--> {"jsonrpc":"2.0","method":"add","params":{"a":5,"b":-7},"id":null}
<-- {"jsonrpc":"2.0","result":-2,"id":null}
--> {"jsonrpc":"2.0","method":"echo","params":{"x":[true,null]},"id":"e"}
<-- {"jsonrpc":"2.0","result":{"x":[true,null]},"id":"e"}

shared counter; the notification in the middle still runs:
--> {"jsonrpc":"2.0","method":"counter.incr","id":2}
<-- {"jsonrpc":"2.0","result":1,"id":2}
--> {"jsonrpc":"2.0","method":"counter.incr","params":[10]}
--> {"jsonrpc":"2.0","method":"counter.incr","params":{"by":-2},"id":3}
<-- {"jsonrpc":"2.0","result":9,"id":3}

bad params, and overflow as an internal error:
--> {"jsonrpc":"2.0","method":"add","params":[1,"2"],"id":4}
<-- {"jsonrpc":"2.0","error":{"code":-32602,"message":"Invalid params","data":"expected two integers: [x, y] or {\"a\": x, \"b\": y}"},"id":4}
--> {"jsonrpc":"2.0","method":"subtract","params":{"a":1,"b":2},"id":5}
<-- {"jsonrpc":"2.0","error":{"code":-32602,"message":"Invalid params","data":"expected two integers: [x, y] or {\"minuend\": x, \"subtrahend\": y}"},"id":5}
--> {"jsonrpc":"2.0","method":"add","params":[1.5,2],"id":6}
<-- {"jsonrpc":"2.0","error":{"code":-32602,"message":"Invalid params","data":"expected two integers: [x, y] or {\"a\": x, \"b\": y}"},"id":6}
--> {"jsonrpc":"2.0","method":"add","params":[9223372036854775807,1],"id":7}
<-- {"jsonrpc":"2.0","error":{"code":-32603,"message":"Internal error","data":"integer overflow"},"id":7}

invalid requests: a usable id is echoed, anything else becomes null:
--> {"jsonrpc":"1.0","method":"add","params":[1,2],"id":8}
<-- {"jsonrpc":"2.0","error":{"code":-32600,"message":"Invalid Request"},"id":8}
--> {"method":"add","params":[1,2]}
<-- {"jsonrpc":"2.0","error":{"code":-32600,"message":"Invalid Request"},"id":null}
--> {"jsonrpc":"2.0","method":"add","params":3,"id":9}
<-- {"jsonrpc":"2.0","error":{"code":-32600,"message":"Invalid Request"},"id":9}
--> {"jsonrpc":"2.0","method":"add","params":[1,2],"id":[10]}
<-- {"jsonrpc":"2.0","error":{"code":-32600,"message":"Invalid Request"},"id":null}
--> "just a string"
<-- {"jsonrpc":"2.0","error":{"code":-32600,"message":"Invalid Request"},"id":null}
--> [[]]
<-- [{"jsonrpc":"2.0","error":{"code":-32600,"message":"Invalid Request"},"id":null}]

not JSON at all:
--> {"jsonrpc":"2.0","method":"add","params":[1,2],"id":11
<-- {"jsonrpc":"2.0","error":{"code":-32700,"message":"Parse error"},"id":null}
"#;

    #[test]
    fn our_methods_and_edge_cases() {
        assert_eq!(run_script(&default_registry(), OUR_EXAMPLES), 17);
    }

    #[test]
    fn now_is_unix_seconds() {
        let reply = default_registry().handle_line(r#"{"jsonrpc":"2.0","method":"now","id":1}"#);
        let reply: Value = serde_json::from_str(&reply.unwrap()).unwrap();
        assert!(reply["result"].as_u64().unwrap() > 1_700_000_000);
    }
}
