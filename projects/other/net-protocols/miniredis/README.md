# miniredis — Redis-compatible server over RESP2

A key-value server that speaks the real Redis wire protocol (RESP2). It
supports `PING`, `ECHO`, `SET` with `EX`/`PX`, `GET`, `DEL`, `EXISTS`,
`INCR`, `EXPIRE`, `TTL`, `KEYS` with globs, `DBSIZE`, and `FLUSHALL`.
Inline commands work for typing into `nc`, and pipelined requests are
answered in order. A small `miniredis-cli` is bundled for one-shot commands
and a REPL.

```sh
cargo run                                   # listens on 127.0.0.1:6399
cargo run --bin miniredis-cli -- PING       # one-shot client
cargo test                                  # parser, commands, expiry, sockets
```

## Try it

With the server running, inline commands work with plain `nc`. Several
lines in one write are pipelined:

```sh
printf 'PING\r\n' | nc 127.0.0.1 6399
# +PONG
printf 'SET greeting hello\r\nGET greeting\r\nINCR visits\r\nINCR visits\r\nKEYS *\r\n' | nc 127.0.0.1 6399
# +OK
# $5
# hello
# :1
# :2
# *2
# $8
# greeting
# $6
# visits
printf '*2\r\n$4\r\nECHO\r\n$5\r\nhello\r\n' | nc 127.0.0.1 6399     # a real RESP array
# $5
# hello
printf '*1\r\n$abc\r\n' | nc 127.0.0.1 6399                          # garbage
# -ERR Protocol error: invalid integer
```

The bundled client prints replies the way `redis-cli` does:

```sh
cargo run --bin miniredis-cli -- SET a 1            # OK
cargo run --bin miniredis-cli -- INCR a             # (integer) 2
cargo run --bin miniredis-cli -- SET t x EX 100     # OK
cargo run --bin miniredis-cli -- TTL t              # (integer) 100
cargo run --bin miniredis-cli -- KEYS '*'           # 1) "a"  2) "greeting"  3) "t" ...
cargo run --bin miniredis-cli -- GET nope           # (nil)
cargo run --bin miniredis-cli -- FOO bar            # (error) ERR unknown command 'FOO'
cargo run --bin miniredis-cli                       # REPL; `quit` or Ctrl-D to leave
```

```text
127.0.0.1:6399> SET counter 10
OK
127.0.0.1:6399> INCR counter
(integer) 11
127.0.0.1:6399> EXISTS counter nope
(integer) 1
```

Use `--addr HOST:PORT` to reach a server somewhere else. If you have Redis
installed, `redis-cli -p 6399` should work too.

## Covers

Streaming parsers over partial reads: `parse(&[u8]) ->
Result<Option<(Frame, usize)>, ProtocolError>`, where `None` means "read
more". Bytes instead of `String`: keys and values are binary-safe
`Vec<u8>`s. Never trusting a length from the wire: bulk lengths are checked
before any bytes are waited for, there's no `with_capacity` from a
peer-supplied count, recursion depth is capped, and each connection's buffer
is capped. Pipelining: every complete frame is answered and all the replies
go out in one write. `Arc<Mutex<Db>>` is locked per command, never across
I/O. Time-based expiry uses `Instant` deadlines, lazy deletion, and
`checked_add`, which can't panic on overflow. Passing `now` into
`Db::execute` makes expiry testable without sleeping. Also covered: slice
patterns that check a command's arity and bind its arguments in one `match`,
an iterative glob matcher that can't be pushed into exponential
backtracking, and `IsTerminal` to show a prompt only to humans.

## Rewrite exercises

1. Rewrite from scratch, parser first: make `every_prefix_is_incomplete`
   and `garbage_is_rejected` pass before you write any socket code.
2. Add `MGET`, `MSET`, `INCRBY`, `DECR`, and `SET … NX|XX`. Each arity rule
   is one slice-pattern arm.
3. Active expiry: a background thread that wakes every 100 ms, samples 20
   keys that have deadlines, and deletes the expired ones, as Redis does.
   Show it frees keys that nobody ever reads again.
4. Fix the quadratic re-parse. The parser restarts from byte 0 after every
   read, so a 1 MiB request arriving in 4 KiB pieces is scanned about 256
   times. Remember where the last scan stopped, or make the parser
   resumable.
5. Persistence: `SAVE` writes every key as a RESP `SET … PX …` command into
   a file, and startup replays the file through the same `parse` and
   `execute`. That's your own tiny append-only file.
