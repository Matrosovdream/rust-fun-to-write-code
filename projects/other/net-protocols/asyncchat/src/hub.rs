//! The hub: all chat state, owned by one task. Pure logic: no sockets and no
//! `.await`, so tests drive it with plain `#[test]`s.

use std::collections::{BTreeMap, HashMap};

use tokio::sync::{mpsc, oneshot};

const LOBBY: &str = "lobby";

pub type ClientId = u64;

/// Everything a connection task can tell the hub.
#[derive(Debug)]
pub enum Event {
    /// A new connection. `tx` is where the hub sends this client's lines;
    /// `reply` is a one-shot answer channel: the hub sends back the guest
    /// nick it picked (request/response with an actor).
    Connect {
        id: ClientId,
        tx: mpsc::Sender<String>,
        reply: oneshot::Sender<String>,
    },
    Line {
        id: ClientId,
        text: String,
    },
    TooLong {
        id: ClientId,
    },
    Disconnect {
        id: ClientId,
    },
}

/// One input line, parsed. The `&'a str`s borrow from the line itself, so
/// parsing allocates nothing; the lifetime says "valid as long as the line".
#[derive(Debug, PartialEq, Eq)]
pub enum Command<'a> {
    Say(&'a str),
    Nick(&'a str),
    Join(&'a str),
    Msg { to: &'a str, text: &'a str },
    Who,
    Rooms,
    Quit,
    Unknown(&'a str),
    Blank,
}

impl<'a> Command<'a> {
    pub fn parse(line: &'a str) -> Self {
        if !line.starts_with('/') {
            return if line.trim().is_empty() {
                Command::Blank
            } else {
                Command::Say(line)
            };
        }
        let (cmd, arg) = line.split_once(' ').unwrap_or((line, ""));
        let arg = arg.trim();
        match cmd {
            "/nick" => Command::Nick(arg),
            "/join" => Command::Join(arg),
            "/msg" => {
                let (to, text) = arg.split_once(' ').unwrap_or((arg, ""));
                Command::Msg { to, text }
            }
            "/who" => Command::Who,
            "/rooms" => Command::Rooms,
            "/quit" => Command::Quit,
            other => Command::Unknown(other),
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

struct Client {
    nick: String,
    room: String,
    tx: mpsc::Sender<String>,
}

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
            Event::Connect { id, tx, reply } => {
                let nick = self.connect(id, tx);
                // The connection task may already be gone; then nobody cares.
                let _ = reply.send(nick);
            }
            Event::Line { id, text } => self.line(id, &text),
            Event::TooLong { id } => self.tell(id, "* line too long"),
            Event::Disconnect { id } => self.leave(id),
        }
    }

    fn connect(&mut self, id: ClientId, tx: mpsc::Sender<String>) -> String {
        // Someone may already have picked "guest7" with /nick, so skip taken names.
        let nick = loop {
            self.guests += 1;
            let nick = format!("guest{}", self.guests);
            if self.find(&nick).is_none() {
                break nick;
            }
        };
        let _ = tx.try_send(format!(
            "* welcome, {nick}! you are in {LOBBY}. try /nick, /join, /msg, /who, /rooms, /quit"
        ));
        // Announce before inserting, so the newcomer doesn't hear about itself.
        self.broadcast(LOBBY, &format!("* {nick} joined {LOBBY}"), None);
        let room = LOBBY.to_string();
        self.clients.insert(
            id,
            Client {
                nick: nick.clone(),
                room,
                tx,
            },
        );
        nick
    }

    fn line(&mut self, id: ClientId, text: &str) {
        // A line can arrive after /quit removed the client: ignore it.
        let Some(me) = self.clients.get(&id) else {
            return;
        };
        // Clone so we stop borrowing `self.clients` before calling `&mut self` methods.
        let (nick, room) = (me.nick.clone(), me.room.clone());

        match Command::parse(text) {
            Command::Blank => {}
            Command::Say(text) => {
                self.broadcast(&room, &format!("[{room}] {nick}: {text}"), Some(id))
            }
            Command::Nick(new) => self.rename(id, new),
            Command::Join(new_room) => self.join(id, new_room),
            Command::Msg { to, text } => self.private(id, &nick, to, text),
            Command::Who => {
                let mut nicks: Vec<&str> = (self.clients.values())
                    .filter(|c| c.room == room)
                    .map(|c| c.nick.as_str())
                    .collect();
                nicks.sort_unstable();
                self.tell(id, &format!("* users in {room}: {}", nicks.join(", ")));
            }
            Command::Rooms => {
                // A BTreeMap iterates in key order, so the list comes out sorted.
                // Rooms only exist while someone is in them.
                let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
                for client in self.clients.values() {
                    *counts.entry(&client.room).or_default() += 1;
                }
                let list: Vec<String> = counts.iter().map(|(r, n)| format!("{r} ({n})")).collect();
                self.tell(id, &format!("* rooms: {}", list.join(", ")));
            }
            Command::Quit => {
                // `leave` drops our Sender. The task still gets "* bye":
                // `recv` returns every queued message before it returns None.
                self.tell(id, "* bye");
                self.leave(id);
            }
            Command::Unknown(cmd) => self.tell(id, &format!("* unknown command: {cmd}")),
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

    fn private(&self, id: ClientId, from: &str, to: &str, text: &str) {
        match self.find(to) {
            Some(target) => {
                let _ = target.tx.try_send(format!("[pm] {from}: {text}"));
                self.tell(id, &format!("[pm -> {to}] {text}"));
            }
            None => self.tell(id, &format!("* no such user: {to}")),
        }
    }

    fn leave(&mut self, id: ClientId) {
        // Removing the client drops its Sender, which ends its task's loop.
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
            let _ = client.tx.try_send(line.to_string());
        }
    }

    /// `try_send`, never `send().await`: the hub serves everyone, so it must
    /// never wait on one client. If a client's queue is full, it has stopped
    /// reading; the line is dropped, and its own write timeout will soon
    /// disconnect it. A closed queue means that client is already leaving.
    fn broadcast(&self, room: &str, line: &str, except: Option<ClientId>) {
        for (&id, client) in &self.clients {
            if client.room == room && Some(id) != except {
                let _ = client.tx.try_send(line.to_string());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Connects a fake client and returns its queue: what its task would write.
    /// Tokio channels work without a runtime as long as nobody `.await`s.
    fn connect(hub: &mut Hub, id: ClientId) -> mpsc::Receiver<String> {
        let (tx, rx) = mpsc::channel(64);
        let (reply, _) = oneshot::channel();
        hub.handle(Event::Connect { id, tx, reply });
        rx
    }

    /// A hub with clients 1..=n connected. `rx[i]` is client `i + 1`'s queue.
    fn hub_with(n: ClientId) -> (Hub, Vec<mpsc::Receiver<String>>) {
        let mut hub = Hub::new();
        let mut rx: Vec<_> = (1..=n).map(|id| connect(&mut hub, id)).collect();
        drain_all(&mut rx);
        (hub, rx)
    }

    fn say(hub: &mut Hub, id: ClientId, text: &str) {
        let text = text.to_string();
        hub.handle(Event::Line { id, text });
    }

    fn drain(rx: &mut mpsc::Receiver<String>) -> Vec<String> {
        std::iter::from_fn(|| rx.try_recv().ok()).collect()
    }

    fn drain_all(rx: &mut [mpsc::Receiver<String>]) {
        rx.iter_mut().for_each(|rx| drop(drain(rx)));
    }

    #[test]
    fn parses_commands() {
        let msg = |to, text| Command::Msg { to, text };
        let cases = [
            ("hello /there", Command::Say("hello /there")),
            ("   ", Command::Blank),
            ("/nick  alice ", Command::Nick("alice")),
            ("/join", Command::Join("")),
            ("/msg bob hi there", msg("bob", "hi there")),
            ("/msg bob", msg("bob", "")),
            ("/who", Command::Who),
            ("/rooms", Command::Rooms),
            ("/quit", Command::Quit),
            ("/foo bar", Command::Unknown("/foo")),
        ];
        for (line, expected) in cases {
            assert_eq!(Command::parse(line), expected, "{line:?}");
        }
    }

    #[test]
    fn welcome_and_join_notice_and_nick_reply() {
        let mut hub = Hub::new();
        let (tx, mut a) = mpsc::channel(64);
        let (reply, mut nick) = oneshot::channel();
        hub.handle(Event::Connect { id: 1, tx, reply });
        assert_eq!(nick.try_recv().unwrap(), "guest1");
        let welcome =
            "* welcome, guest1! you are in lobby. try /nick, /join, /msg, /who, /rooms, /quit";
        assert_eq!(drain(&mut a), [welcome]);
        let mut b = connect(&mut hub, 2);
        assert!(drain(&mut b)[0].starts_with("* welcome, guest2!"));
        assert_eq!(drain(&mut a), ["* guest2 joined lobby"]);
    }

    #[test]
    fn room_messages_skip_sender_and_other_rooms() {
        let (mut hub, mut rx) = hub_with(3);
        say(&mut hub, 3, "/join rust");
        drain_all(&mut rx);
        say(&mut hub, 1, "hi all");
        say(&mut hub, 1, "   ");
        assert!(drain(&mut rx[0]).is_empty());
        assert_eq!(drain(&mut rx[1]), ["[lobby] guest1: hi all"]);
        assert!(drain(&mut rx[2]).is_empty());
    }

    #[test]
    fn nick_rules() {
        let (mut hub, mut rx) = hub_with(2);
        say(&mut hub, 1, "/nick alice");
        assert_eq!(drain(&mut rx[0]), ["* guest1 is now alice"]);
        assert_eq!(drain(&mut rx[1]), ["* guest1 is now alice"]);

        for bad in [
            "/nick",
            "/nick has space",
            "/nick way-too-long-nickname",
            "/nick émile",
        ] {
            say(&mut hub, 2, bad);
            assert_eq!(drain(&mut rx[1]), ["* invalid nick"], "{bad}");
        }
        say(&mut hub, 2, "/nick alice");
        assert_eq!(drain(&mut rx[1]), ["* nick taken: alice"]);
        say(&mut hub, 1, "/nick alice"); // even your own
        assert_eq!(drain(&mut rx[0]), ["* nick taken: alice"]);

        // A later guest skips numbers someone has already claimed.
        say(&mut hub, 2, "/nick guest3");
        let mut c = connect(&mut hub, 3);
        assert!(drain(&mut c)[0].starts_with("* welcome, guest4!"));
    }

    #[test]
    fn join_announces_on_both_sides() {
        let (mut hub, mut rx) = hub_with(3);
        say(&mut hub, 3, "/join rust");
        drain_all(&mut rx);
        say(&mut hub, 1, "/join rust");
        assert_eq!(drain(&mut rx[0]), ["* guest1 joined rust"]);
        assert_eq!(drain(&mut rx[1]), ["* guest1 left lobby"]);
        assert_eq!(drain(&mut rx[2]), ["* guest1 joined rust"]);

        say(&mut hub, 1, "/join rust"); // already there: silent
        say(&mut hub, 1, "/join no spaces");
        assert_eq!(drain(&mut rx[0]), ["* invalid room"]);
    }

    #[test]
    fn replies_to_the_sender() {
        let (mut hub, mut rx) = hub_with(3);
        say(&mut hub, 1, "/nick zed");
        say(&mut hub, 2, "/nick amy");
        say(&mut hub, 3, "/join rust");
        drain_all(&mut rx);

        say(&mut hub, 1, "/msg guest3 psst");
        assert_eq!(drain(&mut rx[0]), ["[pm -> guest3] psst"]);
        assert_eq!(drain(&mut rx[2]), ["[pm] zed: psst"]);
        say(&mut hub, 1, "/msg guest3"); // no text: an empty message
        assert_eq!(drain(&mut rx[0]), ["[pm -> guest3] "]);
        assert_eq!(drain(&mut rx[2]), ["[pm] zed: "]);
        for line in ["/msg carol hi", "/who", "/rooms", "/foo bar"] {
            say(&mut hub, 1, line);
        }
        hub.handle(Event::TooLong { id: 1 });
        let expected = [
            "* no such user: carol",
            "* users in lobby: amy, zed",
            "* rooms: lobby (2), rust (1)",
            "* unknown command: /foo",
            "* line too long",
        ];
        assert_eq!(drain(&mut rx[0]), expected);
    }

    #[test]
    fn quit_says_bye_then_closes_the_queue() {
        let (mut hub, mut rx) = hub_with(2);
        say(&mut hub, 1, "/quit");
        assert_eq!(drain(&mut rx[0]), ["* bye"]);
        // The hub dropped the Sender: this is what ends the connection's loop.
        assert_eq!(
            rx[0].try_recv(),
            Err(mpsc::error::TryRecvError::Disconnected)
        );
        assert_eq!(drain(&mut rx[1]), ["* guest1 left lobby"]);

        // The task's own Disconnect arrives afterwards and must be harmless.
        hub.handle(Event::Disconnect { id: 1 });
        say(&mut hub, 1, "ghost line");
        assert!(drain(&mut rx[1]).is_empty());
    }

    #[test]
    fn a_full_queue_drops_lines_instead_of_blocking() {
        let mut hub = Hub::new();
        let (tx, mut slow) = mpsc::channel(1);
        let (reply, _) = oneshot::channel();
        hub.handle(Event::Connect { id: 1, tx, reply }); // the welcome fills it
        let _b = connect(&mut hub, 2);
        say(&mut hub, 2, "hello?"); // must not block
        assert_eq!(drain(&mut slow).len(), 1);
    }
}
