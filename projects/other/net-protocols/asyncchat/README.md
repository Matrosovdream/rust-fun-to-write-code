# asyncchat — linechat on tokio

The same multi-room chat as linechat, with the same protocol text, rebuilt on
tokio. Each connection is one async task whose loop `select!`s over three
things at once: a line from the socket, a line from the hub, and the stop
signal. A hub task owns all chat state and is reached over channels. Ctrl-C
shuts down gracefully: stop accepting, tell every client
`* server shutting down`, wait for every connection task, exit.

```sh
cargo run                  # listens on 127.0.0.1:7001
cargo run -- 127.0.0.1:7002
cargo test                 # hub + line reader unit tests, real-socket tests
```

## Try it

```sh
cargo run                       # terminal 1
nc 127.0.0.1 7001               # terminal 2: alice
nc 127.0.0.1 7001               # terminal 3: bob
```

What we saw, typing `/nick alice`, `hi bob`, `/who`, `/rooms` as alice and
`/nick bob`, `/msg alice psst`, `/join rust`, `/nick alice`, `/nick b@d`,
`/foo` as bob, then Ctrl-C in terminal 1:

```text
alice's terminal                              bob's terminal
* welcome, guest1! you are in lobby. try ...  * welcome, guest2! you are in lobby. try ...
* guest2 joined lobby                         * guest1 is now alice
* guest1 is now alice                         * guest2 is now bob
* guest2 is now bob                           [lobby] alice: hi bob
[pm] bob: psst                                [pm -> alice] psst
* bob left lobby                              * bob joined rust
* users in lobby: alice                       * nick taken: alice
* rooms: lobby (1), rust (1)                  * invalid nick
* server shutting down                        * unknown command: /foo
                                              * server shutting down
```

Terminal 1 logs the connections and the shutdown, and both `nc`s exit:

```text
asyncchat listening on 127.0.0.1:7001 (Ctrl-C to stop)
[127.0.0.1:57690] connected as guest1
[127.0.0.1:57691] connected as guest2
^Cshutting down: telling 2 connections
[127.0.0.1:57690] disconnected
[127.0.0.1:57691] disconnected
asyncchat stopped
```

Also try pasting a line over 1024 bytes (`* line too long`, and the
connection keeps working), and `/quit` (`* bye`, then the server closes).

## linechat vs asyncchat

| | linechat (threads) | asyncchat (tokio) |
| --- | --- | --- |
| Per client | 2 OS threads: a reader blocked on the socket, a writer blocked on its channel | 1 task: `select!` waits on the socket, its channel, and the stop signal at once |
| Shared state | hub thread owns it, no `Mutex` | hub task owns it, no `Mutex`; same `Hub` code |
| Events to the hub | `std::sync::mpsc::channel`, unbounded | `tokio::sync::mpsc::channel(1024)`, bounded: senders `.await` when it's full |
| Hub to client | `std::sync::mpsc::Sender<String>`, unbounded | `tokio::sync::mpsc::Sender<String>`, 64 lines; the hub `try_send`s and drops lines for a client that isn't keeping up |
| Registering | `Connect` event | `Connect` event plus a `oneshot` reply with the chosen nick |
| Bounded lines | `take` + `read_until`, then skip the rest | `LineReader` over `fill_buf`/`consume`, written to be cancel-safe inside `select!` |
| Stuck client | `set_write_timeout(10 s)` on the socket | `tokio::time::timeout(10 s, write_all)` |
| Disconnect | reader hits EOF and sends `Disconnect`; the writer ends when the hub drops its `Sender`, then `shutdown(Both)` wakes the reader | one loop, one exit: EOF on the read branch or `None` from the channel `break`s, then `Disconnect` |
| Ctrl-C | not handled: the process dies and clients just see the socket close | `ctrl_c()` → drop the listener → `watch` tells every task → each writes `* server shutting down` → `JoinSet` waits for them all |
| Cost of an idle client | two OS threads, each with its own stack | one task: about 8 KB, mostly its read buffer |

## Covers

`async fn` and `.await`, `#[tokio::main]`, `tokio::spawn` vs. threads,
`TcpStream::into_split` for owned read/write halves, `tokio::select!` (what
happens to the branches that lose, cancel safety, `biased;`, disabling a
branch with a pattern, polling a pinned future by `&mut` across loop
iterations), `mpsc` vs. `oneshot` vs. `watch` channels and when each fits,
`try_send` so an actor never waits on one slow client, `JoinSet` to track and
await tasks, `tokio::time::timeout`, a hand-written bounded line reader over
`AsyncBufRead`, and graceful shutdown in the right order.

## Rewrite exercises

1. Rewrite from scratch. The `Hub` is linechat's with tokio channels, so
   start with its tests. Then write `LineReader` and its "cancelled read loses
   nothing" test before you use it in `select!`.
2. Swap `LineReader` for tokio's `read_line` inside the `select!` and write a
   test where a client sends half a line just before a chat message arrives.
   Watch the half line vanish, then explain why.
3. Idle timeout: disconnect a client that has said nothing for 5 minutes
   with `* idle timeout`. Add a fourth `select!` branch on a pinned
   `tokio::time::sleep`, and `reset` it on every line.
4. Rate limiting: allow at most 5 lines per second per client (a token
   bucket in the connection task); extra lines get `* slow down`. Test it
   with `tokio::time::pause()` so the test doesn't really sleep.
5. Replace the hub with `Arc<Mutex<State>>` plus a `tokio::sync::broadcast`
   channel per room. Compare the code, and find the `.await` you must never
   hold the lock across.
