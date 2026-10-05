# smtpd — SMTP mail sink

An SMTP server (a subset of RFC 5321) that accepts mail for anyone and
saves each message as an `.eml` file, with `X-Envelope-From/To` headers on
top. The protocol is an explicit state machine: `Session::step` consumes the
current state and one line, then returns the next state plus what to do. It
does no I/O, so every transition is unit-tested.

```sh
cargo run                                # 127.0.0.1:2525, mail goes to ./maildir
cargo run -- 127.0.0.1:2526 /tmp/mail    # pick an address and a mail directory
cargo test                               # transition table + real-socket sessions
```

## Try it

With the server running, send the sample message using curl's built-in SMTP
client:

```sh
curl smtp://127.0.0.1:2525 --mail-from alice@example.com --mail-rcpt bob@example.com -T examples/hello.eml
ls maildir/
# 1791191626864-0.eml
cat maildir/*.eml
# X-Envelope-From: <alice@example.com>
# X-Envelope-To: <bob@example.com>
# From: Alice <alice@example.com>
# ...
# .This line starts with a dot: curl sends it as "..This", smtpd strips it back.
```

Add `-v` to watch the conversation:

```text
< 220 smtpd ESMTP ready
> EHLO hello.eml
< 250-smtpd
< 250 SIZE 1048576
> MAIL FROM:<alice@example.com> SIZE=213
< 250 OK
> RCPT TO:<bob@example.com>
< 250 OK
> DATA
< 354 end data with <CR><LF>.<CR><LF>
< 250 OK: queued as 1791191626871-1
```

`examples/hello.eml` uses CRLF line endings, like real mail. curl only
dot-stuffs a `.` that comes right after a CRLF, so with an LF-only file the
leading dot would silently vanish. Pass `--crlf` when you send LF files.

You can also play the client by hand with `nc -c 127.0.0.1 2525` (`-c` sends
CRLF). Plain `nc` works too, because the server accepts a bare LF:

```text
220 smtpd ESMTP ready
> EHLO me
250-smtpd
250 SIZE 1048576
> MAIL FROM:<alice@example.com>
250 OK
> RCPT TO:<bob@example.com>
250 OK
> DATA
354 end data with <CR><LF>.<CR><LF>
> Subject: hi
>
> hello from nc
> .
250 OK: queued as 1791191626881-2
> QUIT
221 bye
```

Out-of-order and unknown commands get errors: `DATA` before `HELO` gets
`503 send HELO or EHLO first`, `RCPT` before `MAIL` gets
`503 need MAIL before RCPT`, and `FOO` gets `500 unknown command`.

## Covers

The protocol as an explicit state machine. It's an enum whose variants carry
only the data that's valid in that state, so there's no `DATA` before `RCPT`
to forget to check. `step(self, …)` consumes the old state by value, and one
`match` on `(state, command)` relies on arm order for the error cases. Line
endings on the wire: the server accepts CRLF or LF, always sends CRLF, and
handles multi-line `250-` replies and dot-stuffing. It also covers bounded
line reads with `Read::take`, `set_read_timeout` (a silent client gets `421`
instead of holding a thread forever), `Arc<Config>` shared across connection
threads, `fs::create_dir_all` / `fs::write`, a `static AtomicU64` counter for
unique message ids, and integration tests that check the saved file inside a
`tempfile` directory.

## Rewrite exercises

1. Rewrite from scratch, starting with the transition-table test. Fill in
   the table first, then write `step` until it passes.
2. Add `VRFY` (always `252`) and `HELP`. Then honour the `SIZE=` parameter
   on `MAIL FROM`: refuse an oversized message with `552` before any data is
   sent.
3. Save the real Maildir way: write to `maildir/tmp/`, then `rename` into
   `maildir/new/`, so a mail reader never sees a half-written file.
4. Accept `RCPT` only for one configured domain (`550 no such user here`
   otherwise). Put the rule in `step`, not in the I/O code, and extend the
   table test.
5. Add `AUTH PLAIN` with a hard-coded user list. That means base64 decoding
   by hand, a new state, and `530 authentication required` for `MAIL` before
   a successful `AUTH`.
