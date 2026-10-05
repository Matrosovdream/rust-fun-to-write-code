# udpping — ping over UDP, with a lossy server

A UDP echo server and a `ping`-style client. The client sends 12-byte
`[u32 seq][u64 send-time nanos]` probes and prints one line per probe:
the round-trip time, or `timeout`. At the end it prints sent, received,
loss %, and min/avg/max RTT. The server echoes any datagram up to 1500
bytes. Two flags make the network worse on purpose: `--drop 0.3` loses 30%
of datagrams (a hand-written xorshift PRNG decides which), and
`--delay 50` adds 50 ms. Late or duplicate replies are recognised by
their sequence number and ignored.

```sh
cargo run                                   # server on 127.0.0.1:7300/udp
cargo run --bin udpping-client              # in another terminal: 5 probes, 1 s apart
cargo test
```

Client flags: `-c COUNT` (default 5), `-i INTERVAL_MS` (default 1000),
`-W TIMEOUT_MS` (default 1000). Server flags: `--drop P` (0.0–1.0),
`--delay MS`.

## Try it

A lossy server and 20 probes:

```sh
cargo run -- 127.0.0.1:7300 --drop 0.3                  # terminal 1
cargo run --bin udpping-client -- 127.0.0.1:7300 -c 20  # terminal 2
# UDPPING 127.0.0.1:7300: 12 data bytes
# 12 bytes from 127.0.0.1:7300: seq=0 time=0.130 ms
# seq=1 timeout
# seq=2 timeout
# 12 bytes from 127.0.0.1:7300: seq=3 time=0.208 ms
# …
# --- 127.0.0.1:7300 udpping statistics ---
# 20 probes sent, 14 received, 30.0% loss
# rtt min/avg/max = 0.130/0.227/0.303 ms
```

The seed comes from the clock, so each run loses different probes. The
server logs each datagram: `[127.0.0.1:51865] 12 bytes, dropped`.

Added latency, and what happens when the timeout is shorter than it:

```sh
cargo run -- --delay 50
cargo run --bin udpping-client -- -c 3 -i 200
# 12 bytes from 127.0.0.1:7300: seq=0 time=55.323 ms
# …
# rtt min/avg/max = 52.899/53.856/55.323 ms
cargo run --bin udpping-client -- -c 3 -i 100 -W 20
# seq=0 timeout
# seq=1 timeout
# seq=2 timeout
# 3 probes sent, 0 received, 100.0% loss
```

In that last run the server does answer, but 50 ms later, after the
20 ms timeout. The reply to probe 0 arrives while the client waits for
probe 1, and the client ignores it because the seq doesn't match.

The server never looks inside the datagram, so `nc` works as a client:

```sh
echo hello | nc -u -w 1 127.0.0.1 7300
# hello
```

`nc -u 127.0.0.1 7300` without `-w` gives an interactive echo: each line
you type is one datagram.

## Covers

`UdpSocket::{bind, send_to, recv_from}`: no `accept` and no thread per
client, because the peer address arrives with every datagram. Datagram
boundaries (one `send_to`, one `recv_from`) versus TCP's byte stream.
Detecting an oversized datagram with a buffer one byte too big.
`set_read_timeout` and the WouldBlock/TimedOut it produces. Computing a
shrinking deadline so ignored datagrams can't stretch the wait. Matching
replies to requests by sequence number. A monotonic clock (`Instant`)
versus wall-clock time. Big-endian encoding with `to_be_bytes` and
slice-to-array `try_into`. A `Stats` struct that keeps min/max/sum rather
than every sample. A callback parameter (`impl FnMut(Outcome)`) so the
same loop drives printing and tests. A xorshift PRNG and turning `u64`
bits into a uniform `f64`.

## Rewrite exercises

1. Rewrite from scratch, starting with `Probe` and `Stats` and their
   tests, then the server, then `ping`.
2. Report duplicates the way real `ping` does: remember which seqs have
   been answered, and print `seq=3 time=… ms (DUP!)` for a second reply.
   Count late replies in the summary too.
3. Add `--dup P` to the server (send some datagrams twice) and
   `--reorder`, which holds a datagram back and sends it after the next
   one. Check that the client's numbers stay right.
4. Make `--delay` stop blocking the server: put pending replies in a
   `BinaryHeap` keyed by send time and use the read timeout to wake up
   when the earliest one is due.
5. Send probes on a fixed schedule without waiting for replies (like real
   `ping`), with one thread sending and another receiving. Share the
   in-flight table, which maps seq to send time and expires old entries,
   between them.
