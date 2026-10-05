# linechat — multi-room TCP chat over plain lines

A chat server you can use with nothing but `nc`. It supports nicknames,
rooms, private messages, `/who`, and `/rooms`. One hub thread owns all of the
state. Each connection has a reader thread that sends events to the hub and a
writer thread that drains that client's own channel. There's no `Mutex`
anywhere.

```sh
cargo run                       # listens on 127.0.0.1:7000
cargo run -- 127.0.0.1:7010     # or pick an address
cargo test                      # hub unit tests + real-socket tests
```

## Try it

Run `cargo run`, then open three terminals and connect each with
`nc 127.0.0.1 7000`. Lines starting with `>` are what you type. Some
join and rename notices are left out:

```text
terminal 1                                terminal 2
> /nick alice                             > /nick bob
* guest1 is now alice                     * guest2 is now bob
* guest2 is now bob                       [lobby] alice: hi everyone
> hi everyone                             [pm] alice: psst
> /msg bob psst                           * alice left lobby
[pm -> bob] psst
> /who                                    terminal 3
* users in lobby: alice, bob              > /nick carol
> /rooms                                  > /join rust
* rooms: lobby (2), rust (1)              * carol joined rust
> /nick bob
* nick taken: bob
> /foo
* unknown command: /foo
> /quit
* bye
```

Every client first gets
`* welcome, guest1! you are in lobby. try /nick, /join, /msg, /who, /rooms, /quit`.
Watch the server's stderr: there's one line per connect and one per
disconnect, each with the peer address.

## Covers

Message passing instead of shared memory: `std::sync::mpsc` channels, one
`Event` enum as the hub's whole input, and one `Receiver<String>` per client.
It shows who owns what. Only the hub thread touches the client map, and it
holds the only `Sender` for each writer, so dropping that `Sender` is how a
connection gets closed. Cleanup on disconnect: the reader hits EOF and sends
`Disconnect`, while `/quit` ends the writer, whose `shutdown` wakes the
reader. It also covers `try_clone` to split a socket into a read half and a
write half, bounded line reads with `Read::take` (so a peer can't make us
buffer a huge line), write timeouts for clients that stop reading,
`BTreeMap` for sorted output, `std::mem::replace`, and slice patterns for
argument parsing.

## Rewrite exercises

1. Rewrite from scratch, starting with the `Hub` and its unit tests (feed
   events, read the receivers). Add sockets last.
2. Add `/me waves`, which shows `* alice waves` to the room, and
   `/topic TEXT`, which is shown to anyone who joins. Decide what happens to
   a topic when its room empties.
3. Replay the last 10 lines of a room to whoever joins it. Keep a
   `VecDeque` per room inside the hub.
4. Bound each client's queue: switch to `mpsc::sync_channel(64)` and
   `try_send`, and kick a client whose queue is full ("slow consumer")
   instead of buffering for it forever.
5. Add an admin port (`:7002`) that asks the hub questions (`stats`,
   `kick NICK`) by sending `Event::Query { reply: Sender<String> }` and
   waiting for the answer. That's request/response over channels, and it's
   the actor pattern in full.
