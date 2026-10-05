# tftpd — TFTP server over UDP

An RFC 1350 server: downloads (RRQ) and uploads (WRQ) of files in a root
directory, moved in 512-byte blocks that are acknowledged one at a time.
Each transfer gets its own thread and its own ephemeral port (its "TID").
Lost packets are retransmitted after 1 s, 5 tries in total. Uploads go to
a temporary file that is renamed into place when complete, and paths that
try to leave the root are refused.

```sh
cargo run -- 127.0.0.1:6969 files     # the defaults: plain `cargo run` does the same
cargo test                            # codec unit tests + real UDP transfers
```

## Try it

Start the server in the `tftpd` folder, then use the `tftp` client that
ships with macOS from a second terminal. The client saves downloads in the
current directory (and leaves an empty file behind when a `get` fails), so
run it from a scratch directory:

```sh
FILES=$PWD/files; cd "$(mktemp -d)"   # run this in the tftpd folder

printf 'binary\nget hello.txt\nquit\n' | tftp 127.0.0.1 6969
# Received 18 bytes during 0.0 seconds in 1 blocks
printf 'binary\nget lorem.txt\nquit\n' | tftp 127.0.0.1 6969
# Received 2021 bytes during 0.0 seconds in 4 blocks      (512 + 512 + 512 + 485)
cmp lorem.txt $FILES/lorem.txt && echo same
# same

echo 'remember the milk' > notes.txt
printf 'binary\nput notes.txt\nquit\n' | tftp 127.0.0.1 6969
# Sent 18 bytes during 0.0 seconds in 1 blocks
cmp notes.txt $FILES/notes.txt && echo same
# same
printf 'binary\nput notes.txt\nquit\n' | tftp 127.0.0.1 6969
# Got ERROR packet: File already exists

printf 'get nope.txt\nquit\n' | tftp 127.0.0.1 6969
# Got ERROR packet: File not found
# Error code 256: File not found
printf 'get ../Cargo.toml\nquit\n' | tftp 127.0.0.1 6969
# Got ERROR packet: Access violation
# Error code 512: Access violation
```

"Error code 256" is a byte-order bug in the macOS client. The server sends
code 1 as the big-endian bytes `00 01`, and the client prints them as a
little-endian number without converting.

To see the packets, use `debug packet`. The DATA packet is 22 bytes: the
4-byte header plus the 18-byte file.

```sh
printf 'debug packet\nget hello.txt\nquit\n' | tftp 127.0.0.1 6969
# Sending RRQ: filename: 'hello.txt', mode 'octet'
# Received 22 bytes in a DATA packet
# Sending ACK for block 1
```

To watch retransmission, have the client drop 30% of its packets. The
output changes from run to run. When an ACK is dropped, the server sends
the same block again after 1 s:

```sh
printf 'packetdrop 30\nget lorem.txt\nquit\n' | tftp 127.0.0.1 6969
# Artificial packet drop in send_ack
# Expected DATA block 2, got block 1
# ...
# Received 2021 bytes during 11.0 seconds in 9 blocks
```

The server logs one line per transfer:

```
[127.0.0.1:61241] WRQ notes.txt: received 18 bytes
[127.0.0.1:49707] WRQ notes.txt: error 6 (File already exists)
[127.0.0.1:50230] RRQ ../Cargo.toml: error 2 (Access violation)
```

`nc` can't speak TFTP. The reply comes from the transfer's new port, not
from 6969, and `nc -u` only accepts replies from the address it sent to.
That's the TID rule doing its job. Remove `files/notes.txt` when you're
done.

## Covers

`UdpSocket::send_to`/`recv_from`, and a fresh socket bound to port 0 for
every transfer (the TFTP transfer ID). Reliability built on top of UDP:
lock-step ACKs, retransmission with `set_read_timeout` and an `Instant`
deadline, ignoring duplicates instead of answering them, and giving up
after N tries. A `Packet` enum with `parse`/`encode` over byte slices
(`split_first_chunk`, `try_into` for fixed-size arrays,
`to_be_bytes`/`from_be_bytes`), enum discriminants as wire codes
(`ErrorCode::FileExists as u16`), and 16-bit block numbers with
`u16::wrapping_add`. A generic helper that takes a closure
(`impl FnMut(Packet) -> Option<T>`) so one function serves both
directions. Sandboxing with `Path::components`, temp file plus rename for
atomic uploads, and integration tests on port 0 with a 50 ms timeout and
`tempfile` directories.

## Rewrite exercises

1. Rewrite from scratch. Start with `Packet::parse`/`encode` and their
   tests, including the real RRQ bytes from macOS `tftp`.
2. Add a `--readonly` flag that refuses every WRQ with error 2, and a
   maximum upload size that aborts with error 3 ("Disk full or allocation
   exceeded").
3. "Dally" after an upload (RFC 1350 section 6): keep the transfer socket
   open for one more timeout and re-send the final ACK if the last DATA
   arrives again. Write a test client that ignores the first final ACK.
4. Option negotiation (RFC 2347) with the blksize option (RFC 2348): parse
   the option pairs after the mode and answer with an OACK. Try it with
   `blocksize 1428` in macOS `tftp`.
5. The windowsize option (RFC 7440): send N blocks before waiting for an
   ACK, and restart from the first unacknowledged block on timeout. Compare
   transfer times with `windowsize 8` and `packetdrop 10`.
