# fetchr — curl-lite HTTP/1.1 client

A tiny `curl` for `http://` URLs, built on a plain `TcpStream`. It parses
the URL, writes the request by hand, and reads the body however the server
framed it: by `Content-Length`, chunked, or until the connection closes.
`-L` follows redirects, `-v` shows both sides of the conversation, and
`https://` is refused with a clear message (TLS is out of scope).

```sh
cargo run -- -v http://127.0.0.1:8180/    # with httpd running
cargo run -- -I http://example.com/
cargo test                                # URL table, chunked decoder, canned server
```

## Try it

Start httpd first (`cd ../httpd && cargo run`), then from this folder:

```sh
cargo run -q -- -v http://127.0.0.1:8180/files/hello.txt
# * Connected to 127.0.0.1 (127.0.0.1) port 8180
# > GET /files/hello.txt HTTP/1.1
# > Host: 127.0.0.1:8180
# > User-Agent: fetchr/0.1.0
# > Accept: */*
# > Connection: close
# >
# < HTTP/1.1 200 OK
# < Content-Type: text/plain; charset=utf-8
# < Connection: close
# < Content-Length: 14
# <
# Hello, world!

cargo run -q -- -v -L http://127.0.0.1:8180/files   # httpd redirects to /files/
# < HTTP/1.1 301 Moved Permanently
# < Location: /files/
# * Following redirect to http://127.0.0.1:8180/files/
# ...
# < HTTP/1.1 200 OK
# <!doctype html> ... <title>Index of /files/</title> ...

cargo run -q -- -I http://example.com/
# HTTP/1.1 200 OK
# Date: Mon, 05 Oct 2026 09:14:34 GMT
# Content-Type: text/html; charset=utf-8
# Connection: close
# Server: cloudflare
# ...

cargo run -q -- -I -L http://google.com/            # a public HTTP redirect
# HTTP/1.1 301 Moved Permanently
# Location: http://www.google.com/
# ...
# HTTP/1.1 200 OK
# ...

cargo run -q -- https://example.com/
# fetchr: https:// needs TLS, which fetchr doesn't do; use http://
```

example.com sends its page chunked (`fetchr -v` shows
`< Transfer-Encoding: chunked`), and the decoded body matches curl byte for
byte:

```sh
cmp <(cargo run -q -- http://example.com/) <(curl -s http://example.com/) && echo same
# same
```

With restapi running, `-X`, `-d`, and `-H` work too:

```sh
cargo run -q -- -d '{"title":"from fetchr"}' http://127.0.0.1:8181/todos
# {"id":1,"title":"from fetchr","done":false}
cargo run -q -- -X DELETE -v http://127.0.0.1:8181/todos/1 2>&1 | grep '^< HTTP'
# < HTTP/1.1 204 No Content
```

## Covers

The client side of HTTP: writing a request by hand, then reading a response
whose length you only learn from its head. A `Url` type with `FromStr` and
`Display`, tested with a table. A `ChunkedReader<R: BufRead>` that
implements `Read`, so decoded chunks flow into `io::copy` like any other
bytes. Composing readers: the same `BufReader` is wrapped in `take(n)`, in a
`ChunkedReader`, or used as is, depending on a `Framing` enum built from
RFC 9112 §6.3's precedence rules. Also: `&TcpStream` as both `Read` and
`Write`, `impl Write` parameters so tests can capture "stdout" and "stderr"
in a `Vec<u8>`, a redirect loop with a hop limit, relative `Location`
resolution, and the 303 switch to GET. An in-test canned server returns
fixed-length, chunked, close-delimited, truncated, and redirecting replies.

## Rewrite exercises

1. Rewrite from scratch. Write the `Url` table test and the `ChunkedReader`
   tests first, since the rest is glue.
2. IPv6 literals: `http://[::1]:8180/` breaks the `rsplit_once(':')` port
   split. Fix the parser, add rows to the table, and try it against
   `httpd [::1]:8180`.
3. Add `-o FILE` with a live `received 1.2 MB` line on stderr. Do it with a
   counting `Write` wrapper, an adapter on the write side this time.
4. Keep-alive: stop sending `Connection: close` and reuse one connection for
   a same-host redirect chain. Now you *must* read each redirect's body
   completely. Why?
5. Add `https://` with the `rustls` crate: wrap the `TcpStream` in a
   `rustls::StreamOwned`. Notice how little above the socket changes,
   because everything only asks for `Read` and `Write`.
