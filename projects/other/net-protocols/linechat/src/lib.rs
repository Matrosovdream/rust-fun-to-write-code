//! linechat — a multi-room chat server over plain TCP text lines.
//!
//! # Protocol
//!
//! Both directions send UTF-8 text lines ending in `\n` (a `\r` before the
//! `\n` is dropped, so telnet works too). A line starting with `/` is a
//! command. Any other line is said out loud in your current room. Messages
//! the server sends on its own start with `* `.
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
//! ```
//!
//! Everyone starts in `lobby` with a `guestN` nick. Nicks and room names are
//! 1–16 characters of `[A-Za-z0-9_-]`. Room messages go to everyone in the
//! room except the sender. Lines over [`MAX_LINE`] bytes get
//! `* line too long`.
//!
//! # Design: one owner, many messengers
//!
//! The [`Hub`] owns *all* chat state (who is connected, their nick, their
//! room) and runs on one thread. No other thread can touch that state, so
//! there's no `Mutex` anywhere. Each connection gets two threads:
//!
//! - a **reader** that turns socket lines into [`Event`]s and sends them to
//!   the hub over an `mpsc` channel;
//! - a **writer** that owns a `Receiver<String>` and copies every line the
//!   hub sends it into the socket.
//!
//! ```text
//! reader 1 ──┐                    ┌──> writer 1 ──> socket 1
//! reader 2 ──┼── Event ──> hub ───┼──> writer 2 ──> socket 2
//! reader 3 ──┘           (String) └──> writer 3 ──> socket 3
//! ```
//!
//! The hub holds the only `Sender` for each writer. Dropping it (on `/quit`
//! or disconnect) ends the writer's loop, and the writer then shuts the
//! socket down.

use std::collections::{BTreeMap, HashMap};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;

/// Longest accepted line, counting every byte before the `\n`.
pub const MAX_LINE: usize = 1024;
/// If a write blocks this long, the peer has stopped reading and is dropped.
/// Without this, a client that never reads would make its queue grow forever.
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);
const LOBBY: &str = "lobby";

pub type ClientId = u64;

/// Everything a connection can tell the hub.
#[derive(Debug)]
pub enum Event {
    /// A new connection, carrying the sending half of its writer's channel.
    Connect {
        id: ClientId,
        tx: Sender<String>,
    },
    Line {
        id: ClientId,
        text: String,
    },
    TooLong {
        id: ClientId,
    },
    /// The reader hit EOF or an error.
    Disconnect {
        id: ClientId,
    },
}

struct Client {
    nick: String,
    room: String,
    tx: Sender<String>,
}

/// All chat state. Plain data with no locks and no sockets: tests drive it by
/// calling [`Hub::handle`] and reading the per-client receivers.
#[derive(Default)]
pub struct Hub {
    clients: HashMap<ClientId, Client>,
    guests: u64,
}

impl Hub {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn handle(&mut self, event: Event) {
        match event {
            Event::Connect { id, tx } => self.connect(id, tx),
            Event::Line { id, text } => self.line(id, &text),
            Event::TooLong { id } => self.tell(id, "* line too long"),
            Event::Disconnect { id } => self.leave(id),
        }
    }

    fn connect(&mut self, id: ClientId, tx: Sender<String>) {
        // Someone may already have picked "guest7" with /nick, so skip taken names.
        let nick = loop {
            self.guests += 1;
            let nick = format!("guest{}", self.guests);
            if self.find(&nick).is_none() {
                break nick;
            }
        };
        let _ = tx.send(format!(
            "* welcome, {nick}! you are in {LOBBY}. try /nick, /join, /msg, /who, /rooms, /quit"
        ));
        // Announce before inserting, so the newcomer doesn't hear about itself.
        self.broadcast(LOBBY, &format!("* {nick} joined {LOBBY}"), None);
        let room = LOBBY.to_string();
        self.clients.insert(id, Client { nick, room, tx });
    }

    fn line(&mut self, id: ClientId, text: &str) {
        // A line can arrive after /quit removed the client: ignore it.
        let Some(me) = self.clients.get(&id) else {
            return;
        };
        // Clone so we stop borrowing `self.clients` before calling `&mut self` methods.
        let (nick, room) = (me.nick.clone(), me.room.clone());

        if !text.starts_with('/') {
            if !text.trim().is_empty() {
                self.broadcast(&room, &format!("[{room}] {nick}: {text}"), Some(id));
            }
            return;
        }
        let (cmd, arg) = text.split_once(' ').unwrap_or((text, ""));
        let arg = arg.trim();
        match cmd {
            "/nick" => self.rename(id, arg),
            "/join" => self.join(id, arg),
            "/msg" => self.private(id, &nick, arg),
            "/who" => {
                let mut nicks: Vec<&str> = self
                    .clients
                    .values()
                    .filter(|c| c.room == room)
                    .map(|c| c.nick.as_str())
                    .collect();
                nicks.sort_unstable();
                self.tell(id, &format!("* users in {room}: {}", nicks.join(", ")));
            }
            "/rooms" => {
                // A BTreeMap iterates in key order, so the list comes out sorted.
                // Rooms only exist while someone is in them.
                let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
                for client in self.clients.values() {
                    *counts.entry(&client.room).or_default() += 1;
                }
                let list: Vec<String> = counts.iter().map(|(r, n)| format!("{r} ({n})")).collect();
                self.tell(id, &format!("* rooms: {}", list.join(", ")));
            }
            "/quit" => {
                // `leave` drops our Sender. The writer still gets "* bye":
                // a receiver sees every queued message before it sees the disconnect.
                self.tell(id, "* bye");
                self.leave(id);
            }
            _ => self.tell(id, &format!("* unknown command: {cmd}")),
        }
    }

    fn rename(&mut self, id: ClientId, new: &str) {
        if !valid_name(new) {
            self.tell(id, "* invalid nick");
        } else if self.find(new).is_some() {
            self.tell(id, &format!("* nick taken: {new}"));
        } else if let Some(me) = self.clients.get_mut(&id) {
            let old = std::mem::replace(&mut me.nick, new.to_string());
            let room = me.room.clone();
            self.broadcast(&room, &format!("* {old} is now {new}"), None);
        }
    }

    fn join(&mut self, id: ClientId, room: &str) {
        if !valid_name(room) {
            self.tell(id, "* invalid room");
            return;
        }
        let Some(me) = self.clients.get_mut(&id) else {
            return;
        };
        if me.room == room {
            return; // already there: nothing changes
        }
        let old = std::mem::replace(&mut me.room, room.to_string());
        let nick = me.nick.clone();
        // We've already moved, so the old room's announcement skips us and
        // the new room's includes us (that's our confirmation).
        self.broadcast(&old, &format!("* {nick} left {old}"), None);
        self.broadcast(room, &format!("* {nick} joined {room}"), None);
    }

    fn private(&self, id: ClientId, from: &str, arg: &str) {
        let (to, text) = arg.split_once(' ').unwrap_or((arg, ""));
        match self.find(to) {
            Some(target) => {
                let _ = target.tx.send(format!("[pm] {from}: {text}"));
                self.tell(id, &format!("[pm -> {to}] {text}"));
            }
            None => self.tell(id, &format!("* no such user: {to}")),
        }
    }

    fn leave(&mut self, id: ClientId) {
        // Removing the client drops its Sender, which ends its writer thread.
        // A second Disconnect for the same id (after /quit) finds nothing.
        if let Some(gone) = self.clients.remove(&id) {
            let room = &gone.room;
            self.broadcast(room, &format!("* {} left {room}", gone.nick), None);
        }
    }

    fn find(&self, nick: &str) -> Option<&Client> {
        self.clients.values().find(|c| c.nick == nick)
    }

    fn tell(&self, id: ClientId, line: &str) {
        if let Some(client) = self.clients.get(&id) {
            let _ = client.tx.send(line.to_string());
        }
    }

    fn broadcast(&self, room: &str, line: &str, except: Option<ClientId>) {
        for (&id, client) in &self.clients {
            if client.room == room && Some(id) != except {
                // A failed send means that writer has already gone. Its reader
                // will report Disconnect soon, so there's nothing to do here.
                let _ = client.tx.send(line.to_string());
            }
        }
    }
}

/// Nick and room rule: 1–16 characters of `[A-Za-z0-9_-]`.
pub fn valid_name(name: &str) -> bool {
    (1..=16).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Reads one line without its `\n` / `\r\n`. Returns `Ok(None)` at EOF.
///
/// The peer decides how long a line is, so we never buffer more than
/// `max + 1` bytes of it. If the line is longer, the rest is read and thrown
/// away, and the caller can spot it with `line.len() > max`.
pub fn read_line_limited(r: &mut impl BufRead, max: usize) -> io::Result<Option<Vec<u8>>> {
    let mut line = Vec::new();
    // `take` caps how many bytes `read_until` may consume.
    r.by_ref()
        .take(max as u64 + 1)
        .read_until(b'\n', &mut line)?;
    if line.is_empty() {
        return Ok(None);
    }
    if line.last() == Some(&b'\n') {
        line.pop();
        if line.last() == Some(&b'\r') {
            line.pop();
        }
    } else if line.len() > max {
        skip_line(r)?;
    }
    Ok(Some(line))
}

/// Discards input up to and including the next `\n`, one buffer at a time.
fn skip_line(r: &mut impl BufRead) -> io::Result<()> {
    loop {
        let buf = r.fill_buf()?;
        if buf.is_empty() {
            return Ok(());
        }
        if let Some(i) = buf.iter().position(|&b| b == b'\n') {
            r.consume(i + 1);
            return Ok(());
        }
        let n = buf.len();
        r.consume(n);
    }
}

/// Starts the hub thread, then accepts connections forever.
pub fn serve(listener: TcpListener) {
    let (events, inbox) = mpsc::channel();
    thread::spawn(move || run_hub(inbox));

    let mut next_id: ClientId = 0;
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                next_id += 1;
                let (id, events) = (next_id, events.clone());
                thread::spawn(move || {
                    if let Err(e) = handle(stream, id, events) {
                        eprintln!("connection error: {e}");
                    }
                });
            }
            Err(e) => eprintln!("accept error: {e}"),
        }
    }
}

/// The hub's thread: apply events one at a time, in arrival order.
fn run_hub(inbox: Receiver<Event>) {
    let mut hub = Hub::new();
    for event in inbox {
        hub.handle(event);
    }
}

/// The reader side of one connection. It also starts the writer thread.
///
/// There's no read timeout: an idle chatter is normal, not an attack.
fn handle(stream: TcpStream, id: ClientId, events: Sender<Event>) -> io::Result<()> {
    let peer = stream.peer_addr()?;
    eprintln!("[{peer}] connected as client {id}");
    stream.set_write_timeout(Some(WRITE_TIMEOUT))?;

    let (tx, rx) = mpsc::channel();
    let writer = stream.try_clone()?;
    thread::spawn(move || write_lines(writer, rx));
    if events.send(Event::Connect { id, tx }).is_err() {
        return Ok(()); // the hub is gone, nobody to talk to
    }

    let mut reader = BufReader::new(stream);
    // A read error (reset, shutdown by our writer...) counts as a disconnect, like EOF.
    while let Ok(Some(line)) = read_line_limited(&mut reader, MAX_LINE) {
        let event = if line.len() > MAX_LINE {
            Event::TooLong { id }
        } else {
            let text = String::from_utf8_lossy(&line).into_owned();
            Event::Line { id, text }
        };
        if events.send(event).is_err() {
            break;
        }
    }
    let _ = events.send(Event::Disconnect { id });
    eprintln!("[{peer}] disconnected");
    Ok(())
}

/// The writer side: drains this client's channel into its socket.
fn write_lines(mut stream: TcpStream, lines: Receiver<String>) {
    // The loop ends when the hub drops our Sender (quit / disconnect).
    for line in lines {
        let mut bytes = line.into_bytes();
        bytes.push(b'\n');
        if stream.write_all(&bytes).is_err() {
            break; // peer gone or stopped reading
        }
    }
    // Shutting down both directions also wakes our reader with EOF.
    let _ = stream.shutdown(Shutdown::Both);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// Connects a fake client and returns its inbox: what its writer would send.
    fn connect(hub: &mut Hub, id: ClientId) -> Receiver<String> {
        let (tx, rx) = mpsc::channel();
        hub.handle(Event::Connect { id, tx });
        rx
    }

    fn say(hub: &mut Hub, id: ClientId, text: &str) {
        hub.handle(Event::Line {
            id,
            text: text.to_string(),
        });
    }

    fn drain(rx: &Receiver<String>) -> Vec<String> {
        rx.try_iter().collect()
    }

    #[test]
    fn welcome_and_join_notice() {
        let mut hub = Hub::new();
        let a = connect(&mut hub, 1);
        assert_eq!(
            drain(&a),
            ["* welcome, guest1! you are in lobby. try /nick, /join, /msg, /who, /rooms, /quit"]
        );
        let b = connect(&mut hub, 2);
        assert!(drain(&b)[0].starts_with("* welcome, guest2!"));
        assert_eq!(drain(&a), ["* guest2 joined lobby"]);
    }

    #[test]
    fn room_messages_skip_sender_and_other_rooms() {
        let mut hub = Hub::new();
        let (a, b, c) = (
            connect(&mut hub, 1),
            connect(&mut hub, 2),
            connect(&mut hub, 3),
        );
        say(&mut hub, 3, "/join rust");
        drain(&a);
        drain(&b);
        drain(&c);

        say(&mut hub, 1, "hi all");
        say(&mut hub, 1, "   "); // blank lines are ignored
        assert_eq!(drain(&a), Vec::<String>::new());
        assert_eq!(drain(&b), ["[lobby] guest1: hi all"]);
        assert_eq!(drain(&c), Vec::<String>::new());
    }

    #[test]
    fn nick_rules() {
        let mut hub = Hub::new();
        let (a, b) = (connect(&mut hub, 1), connect(&mut hub, 2));
        drain(&a);
        say(&mut hub, 1, "/nick alice");
        assert_eq!(drain(&a), ["* guest1 is now alice"]);
        assert_eq!(drain(&b).last().unwrap(), "* guest1 is now alice");

        for bad in [
            "/nick",
            "/nick has space",
            "/nick way-too-long-nickname",
            "/nick émile",
        ] {
            say(&mut hub, 2, bad);
            assert_eq!(drain(&b), ["* invalid nick"], "{bad}");
        }
        say(&mut hub, 2, "/nick alice");
        assert_eq!(drain(&b), ["* nick taken: alice"]);

        // A later guest skips numbers someone has already claimed.
        say(&mut hub, 2, "/nick guest3");
        let c = connect(&mut hub, 3);
        assert!(drain(&c)[0].starts_with("* welcome, guest4!"));
    }

    #[test]
    fn join_announces_on_both_sides() {
        let mut hub = Hub::new();
        let (a, b, c) = (
            connect(&mut hub, 1),
            connect(&mut hub, 2),
            connect(&mut hub, 3),
        );
        say(&mut hub, 3, "/join rust");
        drain(&a);
        drain(&b);
        drain(&c);

        say(&mut hub, 1, "/join rust");
        assert_eq!(drain(&a), ["* guest1 joined rust"]);
        assert_eq!(drain(&b), ["* guest1 left lobby"]);
        assert_eq!(drain(&c), ["* guest1 joined rust"]);

        say(&mut hub, 1, "/join rust"); // already there
        say(&mut hub, 1, "/join no spaces");
        assert_eq!(drain(&a), ["* invalid room"]);
    }

    #[test]
    fn private_messages() {
        let mut hub = Hub::new();
        let (a, b) = (connect(&mut hub, 1), connect(&mut hub, 2));
        say(&mut hub, 1, "/nick alice");
        say(&mut hub, 2, "/nick bob");
        drain(&a);
        drain(&b);

        say(&mut hub, 1, "/msg bob hi there");
        assert_eq!(drain(&a), ["[pm -> bob] hi there"]);
        assert_eq!(drain(&b), ["[pm] alice: hi there"]);
        say(&mut hub, 1, "/msg carol hi");
        assert_eq!(drain(&a), ["* no such user: carol"]);
    }

    #[test]
    fn who_and_rooms_are_sorted() {
        let mut hub = Hub::new();
        let rxs: Vec<_> = (1..=3).map(|id| connect(&mut hub, id)).collect();
        say(&mut hub, 2, "/nick zed");
        say(&mut hub, 3, "/nick amy");
        say(&mut hub, 1, "/join rust");
        say(&mut hub, 1, "/join rust"); // no-op
        say(&mut hub, 2, "/who");
        say(&mut hub, 2, "/rooms");
        let got = drain(&rxs[1]);
        assert_eq!(
            got[got.len() - 2..],
            ["* users in lobby: amy, zed", "* rooms: lobby (2), rust (1)"]
        );
    }

    #[test]
    fn quit_says_bye_then_closes_the_channel() {
        let mut hub = Hub::new();
        let (a, b) = (connect(&mut hub, 1), connect(&mut hub, 2));
        drain(&b);
        say(&mut hub, 1, "/quit");
        assert_eq!(a.try_iter().last().unwrap(), "* bye");
        // The hub dropped the Sender: this is what ends the writer thread.
        assert!(a.recv().is_err());
        assert_eq!(drain(&b), ["* guest1 left lobby"]);

        // The reader's EOF arrives afterwards and must be harmless.
        hub.handle(Event::Disconnect { id: 1 });
        say(&mut hub, 1, "ghost line");
        assert_eq!(drain(&b), Vec::<String>::new());
    }

    #[test]
    fn errors() {
        let mut hub = Hub::new();
        let a = connect(&mut hub, 1);
        drain(&a);
        say(&mut hub, 1, "/foo bar");
        hub.handle(Event::TooLong { id: 1 });
        assert_eq!(drain(&a), ["* unknown command: /foo", "* line too long"]);
    }

    #[test]
    fn read_line_limited_handles_endings_partial_and_oversized() {
        let long = "x".repeat(MAX_LINE + 10);
        let input = format!("hi\r\nplain\n{long}\nafter\nno newline");
        // A tiny buffer makes the reader see the input in small pieces.
        let mut r = BufReader::with_capacity(4, Cursor::new(input));
        let mut next = || read_line_limited(&mut r, MAX_LINE).unwrap();
        assert_eq!(next().unwrap(), b"hi");
        assert_eq!(next().unwrap(), b"plain");
        assert_eq!(next().unwrap().len(), MAX_LINE + 1); // cut, flagged as too long
        assert_eq!(next().unwrap(), b"after"); // the rest of the long line was skipped
        assert_eq!(next().unwrap(), b"no newline");
        assert_eq!(next(), None);

        let exact = format!("{}\n", "y".repeat(MAX_LINE));
        let mut r = Cursor::new(exact);
        assert_eq!(
            read_line_limited(&mut r, MAX_LINE).unwrap().unwrap().len(),
            MAX_LINE
        );
    }
}
