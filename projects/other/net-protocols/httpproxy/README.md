# httpproxy — forward HTTP proxy with CONNECT tunnels

Point curl (or a browser) at it and it fetches pages for you. A plain-HTTP
request arrives in absolute-form (`GET http://host/path`). The proxy rewrites
it to origin-form, strips hop-by-hop headers, adds `Via`, and streams the
response back. HTTPS goes through `CONNECT`: the proxy opens a TCP tunnel and
copies bytes both ways without understanding them. One request per client
connection, one thread per connection, and `--block` for a deny list.

```sh
cargo run                                        # listens on 127.0.0.1:8888
cargo run -- 127.0.0.1:8888 --block example.com  # deny a host and its subdomains
cargo test                                       # unit + real-socket tests
```

## Try it

Start it with `cargo run`, then in another terminal:

```sh
curl -v -x http://127.0.0.1:8888 http://example.com/
# > GET http://example.com/ HTTP/1.1          <- absolute-form, to the proxy
# > Host: example.com
# > Proxy-Connection: Keep-Alive              <- hop-by-hop: not forwarded
# < HTTP/1.1 200 OK
# < Content-Type: text/html; charset=utf-8
# < Transfer-Encoding: chunked                <- the origin's framing, passed through
# < Connection: close
# ...
# <title>Example Domain</title>
```

```sh
curl -v -x http://127.0.0.1:8888 https://example.com/
# > CONNECT example.com:443 HTTP/1.1
# > Host: example.com:443
# < HTTP/1.1 200 Connection Established
# * CONNECT tunnel established, response 200
# * SSL connection using TLSv1.3 / ...
# * ALPN: server accepted h2                  <- HTTP/2 inside the tunnel; the proxy can't tell
# ...
# <title>Example Domain</title>
```

The proxy logs one line per request. A tunnel's line appears when it closes:

```
127.0.0.1:60829 GET http://example.com/ -> 200
127.0.0.1:60869 CONNECT example.com:443 -> 200 tunnel closed (585 bytes up, 4900 down)
```

Errors:

```sh
curl -i -x http://127.0.0.1:8888 http://127.0.0.1:1/     # nothing listens there
# HTTP/1.1 502 Bad Gateway
# upstream failed: Connection refused (os error 61)
curl -i -x http://127.0.0.1:8888 http://10.255.255.1/   # unroutable on most networks
# HTTP/1.1 504 Gateway Timeout                           (after the 5 s connect timeout)
curl -i 127.0.0.1:8888/                                  # not a proxy request
# HTTP/1.1 400 Bad Request
# expected an absolute http:// URL (https goes through CONNECT)
```

Restart with `cargo run -- 127.0.0.1:8888 --block example.com`:

```sh
curl -i -x http://127.0.0.1:8888 http://www.example.com/
# HTTP/1.1 403 Forbidden
# www.example.com is blocked by this proxy
curl -v -x http://127.0.0.1:8888 https://example.com/
# < HTTP/1.1 403 Forbidden
# * CONNECT tunnel failed, response 403
curl -s -o /dev/null -w '%{http_code}\n' -x http://127.0.0.1:8888 http://example.org/
# 200
```

It also works in front of the other servers in this group, including
long-lived responses. With ssefeed running,
`curl -N -x http://127.0.0.1:8888 127.0.0.1:8182/events` streams events
through the proxy.

## Covers

Request-target forms (absolute, authority, origin) and parsing them into a
`Target`. Hop-by-hop vs end-to-end headers, the `Connection`-named headers,
rewriting `Host`, adding `Via`. Rejecting conflicting `Content-Length`s
(request smuggling). Streaming the body with `Read::take` + `io::copy`
instead of buffering it. Bytes left in a `BufReader` after the head, which
must be forwarded before splicing. `splice` with two threads, `io::copy`, and
`shutdown(Shutdown::Write)` for half-close. `connect_timeout` over every
address DNS returns. Mapping failures to 502/504 (a read timeout is
`WouldBlock` on Unix). Read timeouts only where silence is a problem (the
request, the upstream's answer), not on tunnels. `Arc<Config>` shared
read-only across threads, and a pure `plan()` that decides everything before
any network I/O.

## Rewrite exercises

1. Rewrite from scratch. Write `parse_target` and `rewrite_head` with their
   tests first. They're the whole proxy logic, minus the plumbing.
2. Add `X-Forwarded-For: <client ip>` upstream, and add a `Via` header to
   responses too. That means parsing the response head instead of copying it
   blind.
3. Proxy authentication: with `--user name:pass`, require
   `Proxy-Authorization: Basic …` and answer `407` with
   `Proxy-Authenticate: Basic realm="httpproxy"` otherwise. Test it with
   `curl -x http://name:pass@127.0.0.1:8888 …`. Decode the base64 by hand.
4. Keep-alive on the client side: serve several requests per client
   connection. You'll need to know where each response ends
   (`Content-Length`, chunked, or close), so you'll parse response framing.
5. A tiny cache: store `200` responses to `GET` that carry
   `Cache-Control: max-age=N` in a `HashMap` keyed by URL, serve hits with an
   `Age` header, and skip anything marked `no-store` or `private`.
