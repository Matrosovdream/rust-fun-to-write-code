# loadbal — L4 TCP load balancer

Accepts TCP connections on `:9400`, picks a healthy backend (round-robin or
least-connections), and splices the two sockets together without ever parsing
the bytes. A failed connect marks that backend down and fails over to the next
one, so the client never notices. A health thread probes every backend every 2 s
and lets dead ones back in when they recover. Per-backend counters are atomics,
shown as a plain-text table on the admin port `:9490`. A tiny `loadbal-backend`
HTTP server is included to balance across.

```sh
cargo run --bin loadbal-backend -- 127.0.0.1:9401      # also :9402 and :9403
cargo run -- 127.0.0.1:9400 --backends 127.0.0.1:9401,127.0.0.1:9402,127.0.0.1:9403
cargo run -- 127.0.0.1:9400 --backends ... --strategy least-conn --admin 127.0.0.1:9490
cargo test                                             # unit + real-socket tests
```

## Try it

```sh
# terminal 1: three backends as background jobs
cargo run -q --bin loadbal-backend -- 127.0.0.1:9401 &
cargo run -q --bin loadbal-backend -- 127.0.0.1:9402 &
cargo run -q --bin loadbal-backend -- 127.0.0.1:9403 &

# terminal 2: the balancer
cargo run -q -- 127.0.0.1:9400 --backends 127.0.0.1:9401,127.0.0.1:9402,127.0.0.1:9403
# loadbal listening on 127.0.0.1:9400, admin on 127.0.0.1:9490, RoundRobin over 3 backends
# [127.0.0.1:62036] -> 127.0.0.1:9401
# [127.0.0.1:62038] -> 127.0.0.1:9402

# terminal 3: requests rotate
for i in 1 2 3 4 5 6; do curl -s 127.0.0.1:9400; done
# hello from 127.0.0.1:9401
# hello from 127.0.0.1:9402
# hello from 127.0.0.1:9403
# hello from 127.0.0.1:9401
# hello from 127.0.0.1:9402
# hello from 127.0.0.1:9403

# kill the :9402 backend (in terminal 1: kill %2), then repeat
for i in 1 2 3 4; do curl -s 127.0.0.1:9400; done
# hello from 127.0.0.1:9401      every request still succeeds; terminal 2 logs
# hello from 127.0.0.1:9401      "backend 127.0.0.1:9402 is down: Connection refused"
# hello from 127.0.0.1:9403
# hello from 127.0.0.1:9401

curl 127.0.0.1:9490
# backend                state active  total
# 127.0.0.1:9401         up         0      5
# 127.0.0.1:9402         down       0      2
# 127.0.0.1:9403         up         0      3

# restart it (cargo run -q --bin loadbal-backend -- 127.0.0.1:9402 &):
# within 2 s terminal 2 logs "backend 127.0.0.1:9402 is up" and it rejoins
for i in 1 2 3; do curl -s 127.0.0.1:9400; done
# hello from 127.0.0.1:9403
# hello from 127.0.0.1:9401
# hello from 127.0.0.1:9402
```

Start the backends first. A backend that comes up after the balancer starts
out marked down and joins within one health interval (2 s).

To see least-conn at work, restart the balancer with `--strategy least-conn`
and hold one connection open with `nc 127.0.0.1 9400` in another terminal.
The backend serving it shows `active 1` in the stats, and curl requests
alternate between the other two.

## Covers

`AtomicBool`/`AtomicUsize` and what `Ordering` actually means (atomicity
vs. visibility, and why `Relaxed` is right when an atomic doesn't publish
other data); `swap` to detect a state change exactly once; an RAII guard that
decrements a counter on every exit path; strategy selection as a pure function
over plain snapshots; failover; a background health thread; splicing two
sockets with `thread::scope` and `impl Read for &TcpStream`, plus half-close
via `Shutdown::Write`; `connect_timeout`; a closure-parameterized accept loop
shared by two tiny HTTP servers; why closing with unread input sends an RST.
Compare with httpproxy: layer 4 decides before reading a byte, layer 7 reads
the request first.

## Rewrite exercises

1. Rewrite from scratch, starting with `pick` and its unit tests: it's the
   whole balancing policy, and it has no sockets.
2. Weighted backends: `--backends 127.0.0.1:9401=3,127.0.0.1:9402=1`. Make
   round-robin honor the weights without bursts (look up nginx's "smooth
   weighted round-robin") and test the sequence it produces.
3. Make health checks smarter: send `GET /health` and require a `200`, and add
   rise/fall thresholds (down after 2 failed probes, up after 3 good ones).
   Keep the transition a pure function of `(state, probe result)`.
4. Add an idle timeout to `splice` (a connection with no bytes in either
   direction for 30 s is closed) and a `--max-conns` limit per backend that
   both strategies respect.
5. Graceful drain: `curl -X POST 127.0.0.1:9490/drain/127.0.0.1:9402` stops
   new connections to that backend while existing ones finish. The stats
   table shows `draining` until `active` reaches 0, then `drained`.
