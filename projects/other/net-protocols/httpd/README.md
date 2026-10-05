# httpd — static file server on hand-written HTTP/1.1

Serves a directory over HTTP/1.1 using only `std`: a request parser over
`BufRead`, keep-alive connections, MIME types, directory listings, path
traversal protection, and a fixed thread pool. Every size is bounded, so
garbage gets a 400/413/414/431 and a closed connection, never a crash.

```sh
cargo run                            # serves ./public on 127.0.0.1:8180
cargo run -- 127.0.0.1:8180 public   # the same, spelled out
cargo test                           # parser, paths, pool, real-socket tests
```

## Try it

With the server running in another terminal (started from this folder):

```sh
curl -v 127.0.0.1:8180/files/hello.txt
# > GET /files/hello.txt HTTP/1.1
# > Host: 127.0.0.1:8180
# > User-Agent: curl/8.7.1
# > Accept: */*
# >
# < HTTP/1.1 200 OK
# < Content-Type: text/plain; charset=utf-8
# < Content-Length: 14
# <
# Hello, world!

curl -I 127.0.0.1:8180/style.css                  # HEAD: headers, no body
# HTTP/1.1 200 OK
# Content-Type: text/css; charset=utf-8
# Content-Length: 900

curl -v --path-as-is 127.0.0.1:8180/../Cargo.toml  # tries to leave the root
# < HTTP/1.1 403 Forbidden

curl -v 127.0.0.1:8180/ 127.0.0.1:8180/style.css -o /dev/null -o /dev/null
# * Connected to 127.0.0.1 (127.0.0.1) port 8180
# * Re-using existing connection with host 127.0.0.1

curl -i 127.0.0.1:8180/files          # 301, Location: /files/
curl -s 127.0.0.1:8180/files/         # HTML listing: no index.html in there
curl -i 127.0.0.1:8180/nope           # 404 Not Found
curl -i -X DELETE 127.0.0.1:8180/     # 405 Method Not Allowed, Allow: GET, HEAD

printf 'hello\r\n\r\n' | nc 127.0.0.1 8180
# HTTP/1.1 400 Bad Request
# Content-Type: text/plain; charset=utf-8
# Connection: close
# Content-Length: 61
#
# 400 Bad Request: request line is not `METHOD target VERSION`

curl -i -H "X-Big: $(head -c 20000 /dev/zero | tr '\0' a)" 127.0.0.1:8180/
# HTTP/1.1 431 Request Header Fields Too Large
```

Open <http://127.0.0.1:8180> in a browser as well. The server logs one line
per request to stderr:

```text
listening on http://127.0.0.1:8180, serving public
127.0.0.1:57254 GET /style.css 200 900B 0.1ms
127.0.0.1:57263 - - 400 61B 0.0ms
```

A kept-alive connection that sends nothing for 5 s is closed, which frees
its worker thread.

## Covers

The anatomy of an HTTP/1.1 message (request line, headers, empty line,
`Content-Length` body) and parsing it from any `BufRead`, so tests use byte
slices. `Read::take` serves both as a size limit and as a body framer. A
`ParseError` enum with `From<io::Error>` lets `?` do the conversion. Also
covered: keep-alive versus `Connection: close` versus HTTP/1.0, read
timeouts, and "lingering close" so error replies aren't lost to a TCP RST.
The thread pool is the Rust book's (`mpsc` plus `Arc<Mutex<Receiver>>`, with
`Box<dyn FnOnce() + Send>` jobs and a `Drop` that closes the channel and
joins). `&TcpStream` acts as both `Read` and `Write`. Path safety comes from
percent-decoding before splitting, lexical `..` handling, and
`canonicalize` against symlinks. Integration tests bind port 0 and use
`tempfile`.

## Rewrite exercises

1. Rewrite from scratch. Write `parse_request` and its byte-slice tests
   first, then the pool, then `respond`, and glue them together last.
2. Stream files instead of `fs::read`: give `Response` a body enum
   (`Bytes(Vec<u8>)` or `File(File, u64)`) and `io::copy` the file
   straight into the socket. Serve a 2 GB file and watch memory stay flat.
3. Add `Date` and `Last-Modified` headers, formatting the IMF-fixdate
   (`Sun, 06 Nov 1994 08:49:37 GMT`) yourself from `SystemTime`. Then
   answer `If-Modified-Since` with `304 Not Modified`.
4. Support `Range: bytes=0-99` with `206 Partial Content` and
   `Content-Range` (and `416` for impossible ranges). `curl -r 0-4` and
   seeking in a browser `<video>` should both work.
5. Slowloris: a client that sends one header byte every 4 s never trips the
   5 s idle timeout, so it can hold a worker for hours. Prove it with a script
   that starves all 8 workers. Then fix it with a total deadline for reading
   the request head, and compare with how an async server (asyncchat) copes.
