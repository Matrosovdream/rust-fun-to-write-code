# binkv — key-value store over a binary protocol

A TCP key-value store that speaks a small binary protocol designed for
this project: a `BKV1` magic, then length-prefixed frames
(`[u32 BE length][u8 opcode][payload]`) carrying GET, PUT, DEL, LIST, and
PING. Strings are `[u16 BE len][bytes]`, so keys and values can hold any
bytes. Frames over 1 MiB are rejected before anything is allocated. The
full byte layout is documented at the top of `src/lib.rs`.

```sh
cargo run                                        # server on 127.0.0.1:7100
cargo run --bin binkv-client -- put name rust    # in another terminal
cargo test                                       # unit + real-socket tests
```

## Try it

With the server running (`cargo run`):

```sh
cargo run --bin binkv-client -- put name rust
# OK
cargo run --bin binkv-client -- get name
# rust
cargo run --bin binkv-client -- list
# name
cargo run --bin binkv-client -- del name        # then `get name` prints (not found), exit 1
# OK
```

`--hex` shows the raw frames: the magic, then `length 0x0d`, opcode `02`
(PUT), and two strings; the reply is `length 1`, status `00` (OK).

```sh
cargo run --bin binkv-client -- --hex put name rust
# > 42 4b 56 31
# > 00 00 00 0d 02 00 04 6e 61 6d 65 00 04 72 75 73 74
# < 00 00 00 01 00
# OK
```

No client needed: write the bytes yourself. `BKV1`, then a frame of
length 1 holding opcode `05` (PING):

```sh
printf 'BKV1\0\0\0\1\5' | nc -w 2 127.0.0.1 7100 | xxd
# 00000000: 0000 0007 0000 0450 4f4e 47              .......PONG
printf 'HELO' | nc -w 2 127.0.0.1 7100 | xxd       # wrong magic: ERROR, then close
# 00000000: 0000 001b 0200 1862 6164 206d 6167 6963  .......bad magic
# 00000010: 3a20 6578 7065 6374 6564 2042 4b56 31    : expected BKV1
```

The server logs every request with the peer address:

```text
[127.0.0.1:57149] PUT "name" (4 bytes)
[127.0.0.1:57154] bad magic "HELO", closing
```

## Covers

Designing a binary wire format and documenting it byte by byte;
length-prefix framing; big-endian integers with `u32::to_be_bytes` /
`from_be_bytes`; a magic number as a protocol version; a `read_frame` that
tells a clean EOF (`Ok(None)`) apart from a peer dying mid-frame
(`UnexpectedEof`); never trusting a length off the wire (check it first,
then grow the buffer with `Read::take` only as bytes arrive); a borrowing
`Cursor<'a>` that turns every short read into an error instead of a panic;
`Request`/`Response` enums with `encode`/`decode` round trips; slice
patterns for argument parsing; `Arc<Mutex<HashMap<Vec<u8>, Vec<u8>>>>`
shared by a thread per connection; read timeouts for idle clients.

## Rewrite exercises

1. Rewrite from scratch, starting with `Cursor` and the round-trip tests;
   only then add sockets.
2. Add `INCR key` (opcode `0x06`): parse the value as a decimal `i64`, add
   one, and reply with the new value. Reply ERROR when the value isn't a
   number.
3. Add a request id: a `u32` after the opcode that the server copies into
   the response. Then pipeline: let the client send ten GETs before reading
   any replies, and match them up by id.
4. Make LIST take an optional prefix string and stream its answer as
   several frames ending with an empty one, instead of failing when the key
   list passes 1 MiB.
5. Bump the protocol to `BKV2` with `u32` value lengths, and make the
   server accept both magics, choosing the codec per connection. Keep the
   `BKV1` tests passing unchanged.
