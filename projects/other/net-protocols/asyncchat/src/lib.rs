//! asyncchat — linechat rebuilt on tokio: tasks instead of threads.
//!
//! # Protocol
//!
//! Exactly linechat's: UTF-8 text lines ending in `\n` (a `\r` before it is
//! dropped). A line starting with `/` is a command, any other line is said in
//! your current room, and server notices start with `* `.
//!
//! ```text
//! (connect)        S: * welcome, guest1! you are in lobby. try /nick, /join, /msg, /who, /rooms, /quit
//!                  C: /nick alice
//!                  S: * guest1 is now alice
//!                  C: hi                  bob sees: [lobby] alice: hi
//!                  C: /msg bob psst       bob sees: [pm] alice: psst
//!                  S: [pm -> bob] psst
//!                  C: /who
//!                  S: * users in lobby: alice, bob
//!                  C: /rooms
//!                  S: * rooms: lobby (2)
//!                  C: /join rust          lobby sees: * alice left lobby
//!                  S: * alice joined rust
//!                  C: /quit
//!                  S: * bye               rust sees: * alice left rust
//! (Ctrl-C)         S: * server shutting down
//! ```
//!
//! Nicks and room names are 1–16 characters of `[A-Za-z0-9_-]`. Lines over
//! [`MAX_LINE`] bytes get `* line too long`.
//!
//! # Design: one hub task, one task per connection
//!
//! ```text
//!               Event (one mpsc)              String (one mpsc per client)
//!  conn task 1 ──┐                          ┌──▶ conn task 1 ──▶ socket 1
//!  conn task 2 ──┼──────▶   hub task   ─────┼──▶ conn task 2 ──▶ socket 2
//!  conn task 3 ──┘   (owns all chat state)  └──▶ conn task 3 ──▶ socket 3
//!        ▲
//!        └── stop signal (watch<bool>) from serve() on Ctrl-C
//! ```
//!
//! The [`Hub`] is the same as linechat's: it owns every nick and room, so
//! there is no `Mutex`. The difference is the connection side. A blocking
//! thread can wait for only one thing, so linechat needs two threads per
//! client: one blocked reading the socket, one blocked on the channel. An
//! async task can wait for several things at once with `tokio::select!`,
//! so one task per client does both, and also watches for shutdown.
//!
//! # `select!` and cancellation
//!
//! `select!` polls all of its branches. The first one to finish wins, and
//! the others are *dropped*: cancelled at whatever `.await` they were parked
//! on. So each branch must be **cancel-safe**: dropping it halfway must not
//! lose data.
//! - `watch::Receiver::changed`, `mpsc::Receiver::recv`,
//!   `TcpListener::accept`, and `JoinSet::join_next` are all cancel-safe
//!   (tokio documents this): the item is either returned or left in place.
//! - Reading a line is the tricky one. tokio's `read_line` is *not*
//!   cancel-safe, so we use our own [`LineReader`], which is.
//! - Socket writes never sit in a `select!`. They run in the winning
//!   branch's handler, so a half-written line is never cancelled.
//!
//! # Graceful shutdown
//!
//! 1. Ctrl-C completes the `shutdown` future passed to [`serve`].
//! 2. `serve` stops accepting: it drops the listener, so new connects are refused.
//! 3. It sends `true` on the `watch` channel. Every connection task wakes,
//!    writes `* server shutting down`, and closes its socket.
//! 4. `serve` waits for every connection task (a `JoinSet`), then for the
//!    hub, and returns.

mod hub;
mod lines;

use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::time::Duration;

use tokio::io::AsyncWriteExt;
use tokio::net::tcp::OwnedWriteHalf;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinSet;
use tokio::time::timeout;

pub use hub::{ClientId, Command, Event, Hub, valid_name};
pub use lines::{Line, LineReader};

/// Longest accepted line, counting every byte before the `\n`.
pub const MAX_LINE: usize = 1024;
/// Lines queued for one client before the hub starts dropping them.
const OUTBOX: usize = 64;
/// Events queued for the hub before connection tasks have to wait.
const EVENT_QUEUE: usize = 1024;
/// A client that stops reading is dropped once a write blocks this long.
/// Without it, one stuck client would also stall the graceful shutdown.
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);

/// Accepts connections until `shutdown` completes, then shuts down
/// gracefully (see the module docs). Pass `tokio::signal::ctrl_c()` in the
/// binary, or a oneshot receiver in tests.
pub async fn serve(listener: TcpListener, shutdown: impl Future<Output = ()>) {
    let (events, inbox) = mpsc::channel(EVENT_QUEUE);
    let hub = tokio::spawn(run_hub(inbox));
    let (stop_tx, stop_rx) = watch::channel(false);
    let mut connections = JoinSet::new();
    let mut next_id: ClientId = 0;

    // A future handed to `select!` by value is dropped whenever another
    // branch wins. We need the *same* shutdown future to survive every loop
    // iteration, so we pass `&mut shutdown`. Polling through a reference
    // requires the future to be pinned (it may not move once polled).
    let mut shutdown = std::pin::pin!(shutdown);
    loop {
        tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok((stream, peer)) => {
                    next_id += 1;
                    let task = handle(stream, peer, next_id, events.clone(), stop_rx.clone());
                    connections.spawn(task);
                }
                Err(e) => eprintln!("accept error: {e}"),
            },
            // Reap finished connection tasks so the JoinSet doesn't grow
            // forever. With no tasks, `join_next` returns `None`, which
            // doesn't match `Some(_)`: that just disables the branch this round.
            Some(_) = connections.join_next() => {}
            () = &mut shutdown => break,
        }
    }

    drop(listener);
    eprintln!("shutting down: telling {} connections", connections.len());
    // Ignoring the error is fine: it only means no task is listening.
    let _ = stop_tx.send(true);
    while connections.join_next().await.is_some() {}
    // The hub stops once every Sender is gone: ours is the last one.
    drop(events);
    let _ = hub.await;
}

/// The hub task: apply events one at a time, in arrival order.
async fn run_hub(mut inbox: mpsc::Receiver<Event>) {
    let mut hub = Hub::new();
    while let Some(event) = inbox.recv().await {
        hub.handle(event);
    }
}

/// One connection, start to finish.
async fn handle(
    stream: TcpStream,
    peer: SocketAddr,
    id: ClientId,
    events: mpsc::Sender<Event>,
    mut stop: watch::Receiver<bool>,
) {
    // `into_split` gives two owned halves, so reading and writing can be
    // borrowed separately inside one `select!`.
    let (read_half, mut writer) = stream.into_split();
    let mut lines = LineReader::new(read_half, MAX_LINE);
    let (tx, mut outbox) = mpsc::channel(OUTBOX);

    // Register with the hub, then wait for its answer on a oneshot.
    let (reply, nick) = oneshot::channel();
    if events.send(Event::Connect { id, tx, reply }).await.is_err() {
        return; // the hub is gone: we're shutting down
    }
    let Ok(nick) = nick.await else {
        return;
    };
    eprintln!("[{peer}] connected as {nick}");

    loop {
        tokio::select! {
            // By default `select!` polls branches in random order, for
            // fairness. `biased` polls top to bottom, so a pending stop
            // always wins, and a client never gets more chat after
            // "* server shutting down". The price: a branch that is always
            // ready starves the ones below it. Fine here: `stop` fires once,
            // and an outbox drains far faster than people type.
            biased;

            _ = stop.changed() => {
                let _ = write_line(&mut writer, "* server shutting down").await;
                break;
            }
            message = outbox.recv() => match message {
                Some(line) => {
                    if write_line(&mut writer, &line).await.is_err() {
                        break; // client gone or not reading
                    }
                }
                // The hub dropped our Sender: we sent /quit and got "* bye".
                None => break,
            },
            line = lines.next_line() => {
                let event = match line {
                    Ok(Some(Line::Text(text))) => Event::Line { id, text },
                    Ok(Some(Line::TooLong)) => Event::TooLong { id },
                    // EOF or a read error such as a reset: the client is gone.
                    Ok(None) | Err(_) => break,
                };
                if events.send(event).await.is_err() {
                    break;
                }
            }
        }
    }

    let _ = events.send(Event::Disconnect { id }).await;
    let _ = writer.shutdown().await;
    eprintln!("[{peer}] disconnected");
}

async fn write_line(writer: &mut OwnedWriteHalf, line: &str) -> io::Result<()> {
    let mut bytes = Vec::with_capacity(line.len() + 1);
    bytes.extend_from_slice(line.as_bytes());
    bytes.push(b'\n');
    // `timeout` wraps the write's own Result in an outer one: Err = too slow.
    match timeout(WRITE_TIMEOUT, writer.write_all(&bytes)).await {
        Ok(result) => result,
        Err(_elapsed) => Err(io::ErrorKind::TimedOut.into()),
    }
}
