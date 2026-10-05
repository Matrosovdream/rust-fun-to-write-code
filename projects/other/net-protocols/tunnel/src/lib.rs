//! tunnel — an ngrok-lite reverse tunnel.
//!
//! You have a service on a private machine (`127.0.0.1:18080` on your
//! laptop) and want it reachable from a public one. The private side can't
//! accept connections from outside, but it *can* dial out. So the **agent**
//! (private side) dials the **relay** (public side) and keeps that
//! connection open. The relay then uses it to ask the agent to dial back
//! once per public client.
//!
//! ```text
//!                      PUBLIC MACHINE                     PRIVATE MACHINE
//!                ┌──────────────────────┐            ┌──────────────────────┐
//!                │     tunnel-relay     │            │     tunnel-agent     │
//!                │                      │  control   │                      │
//!                │  :7500 control ◀─────┼────────────┼── dials out at start │
//!                │                      │ HELLO, OK, │  (one, long-lived)   │
//!                │                      │ CONNECT,   │                      │
//!                │                      │ PING, PONG │                      │
//!  curl ────────▶│  :8090 public        │            │                      │
//!   (one public  │     ▲                │  data      │                      │     local target
//!    connection) │     └─ spliced ◀─────┼────────────┼── one per public  ───┼──▶ :18080
//!                │        with          │  DATA <id> │   connection,        │   (http.server)
//!                │                      │  then raw  │   spliced with the   │
//!                │                      │  bytes     │   target             │
//!                └──────────────────────┘            └──────────────────────┘
//! ```
//!
//! # What happens when a public client connects
//!
//! 1. curl connects to the relay's public port. The relay gives the
//!    connection an id (say 7), stores a `oneshot::Sender<TcpStream>` in
//!    `pending[7]`, and sends `CONNECT 7` over the control connection.
//! 2. The agent reads `CONNECT 7`, opens a *new* connection to the relay's
//!    control port, and writes `DATA 7` as its first line.
//! 3. The relay reads that first line, removes `pending[7]`, and sends the
//!    new socket through the oneshot to the task holding curl's connection.
//! 4. The agent connects to its local target.
//! 5. Both sides splice with `copy_bidirectional`: the relay joins curl's
//!    socket to the data connection, the agent joins the data connection to
//!    the target. Bytes flow curl → relay → agent → target and back.
//! 6. When one end closes, the EOF travels through both splices as a
//!    half-close, and everything shuts down in order.
//!
//! If no data connection arrives within 5 s, the relay drops `pending[7]`
//! and closes curl's connection. If no agent is connected at all, the
//! client waits the same 5 s: an agent that connects in the meantime is told
//! about every pending id right after its `OK`.
//!
//! Why a separate data connection per client, rather than sending
//! everything over the control connection? Mixing many streams on one
//! connection needs framing, stream ids, and flow control (that's what
//! HTTP/2 and SSH channels do). One TCP connection per stream lets the
//! kernel do all of it. FTP and frp use the same control/data split.
//!
//! # Control protocol
//!
//! Text lines ending in `\n`, at most [`MAX_LINE`] bytes:
//!
//! | Line           | Direction                   | Meaning                                          |
//! | -------------- | --------------------------- | ------------------------------------------------ |
//! | `HELLO <token>`| agent → relay, first line   | "I'm the agent". The token is a shared secret.   |
//! | `OK`           | relay → agent               | accepted                                         |
//! | `ERR <reason>` | relay → agent               | refused (bad token, agent already connected); the relay closes |
//! | `CONNECT <id>` | relay → agent               | a public client is waiting: open a data connection for it |
//! | `DATA <id>`    | agent → relay, first line of a new connection | "this connection is for client `<id>`"; raw bytes follow |
//! | `PING`         | agent → relay               | heartbeat, every 5 s                             |
//! | `PONG`         | relay → agent               | heartbeat reply                                  |
//!
//! Anything else, or a line that's too long, closes that connection.
//!
//! Heartbeat: the agent sends `PING` every [`Config::heartbeat`], and the
//! relay answers `PONG`. If either side hears nothing for three heartbeats,
//! it treats the connection as dead. TCP alone can take a very long time to
//! notice a peer that vanished without closing. The relay then frees the
//! agent slot, and the agent reconnects with exponential backoff (see
//! [`backoff`]).

mod agent;
mod proto;
mod relay;

use std::collections::HashMap;
use std::time::Duration;

pub use agent::run_agent;
pub use proto::{MAX_LINE, Msg, MsgReader, read_first_line, send};
pub use relay::run_relay;

/// Settings for both sides. Tests shrink every duration to milliseconds.
#[derive(Debug, Clone)]
pub struct Config {
    /// Shared secret: the agent sends it in `HELLO`, the relay checks it.
    pub token: String,
    /// Relay: how long a public client waits for its data connection.
    pub pending_timeout: Duration,
    /// Agent: limit for each outgoing connect and for the `HELLO` reply.
    /// Relay: limit for a new control-port connection's first line.
    pub connect_timeout: Duration,
    /// Agent: `PING` interval. Both: silence for 3× this means dead.
    pub heartbeat: Duration,
    /// Agent: first reconnect delay; doubles per failure up to `backoff_max`.
    pub backoff_min: Duration,
    pub backoff_max: Duration,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            token: "dev".to_string(),
            pending_timeout: Duration::from_secs(5),
            connect_timeout: Duration::from_secs(5),
            heartbeat: Duration::from_secs(5),
            backoff_min: Duration::from_millis(250),
            backoff_max: Duration::from_secs(8),
        }
    }
}

/// Delay before reconnect attempt number `failures` (0-based):
/// `min`, `2·min`, `4·min`, … capped at `max`.
pub fn backoff(failures: u32, min: Duration, max: Duration) -> Duration {
    // `saturating_*` pins at the maximum instead of overflowing (which would
    // panic in debug builds) after many failures.
    min.saturating_mul(2u32.saturating_pow(failures)).min(max)
}

/// Hand-rolled `--name value` parsing for the binaries. `spec` lists every
/// allowed flag with its default; `None` means the flag is required. Every
/// flag in `spec` is in the result, so indexing it can't fail.
pub fn parse_flags(
    args: impl IntoIterator<Item = String>,
    spec: &[(&'static str, Option<&str>)],
) -> Result<HashMap<&'static str, String>, String> {
    let mut values = HashMap::new();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let Some(&(name, _)) = spec.iter().find(|(name, _)| *name == arg) else {
            return Err(format!("unknown argument: {arg}"));
        };
        let value = args.next().ok_or_else(|| format!("{name} needs a value"))?;
        values.insert(name, value);
    }
    for &(name, default) in spec {
        if !values.contains_key(name) {
            let default = default.ok_or_else(|| format!("{name} is required"))?;
            values.insert(name, default.to_string());
        }
    }
    Ok(values)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_up_to_the_cap() {
        let (min, max) = (Duration::from_millis(250), Duration::from_secs(8));
        let delays: Vec<u128> = (0..8).map(|n| backoff(n, min, max).as_millis()).collect();
        assert_eq!(delays, [250, 500, 1000, 2000, 4000, 8000, 8000, 8000]);
        assert_eq!(backoff(u32::MAX, min, max), max);
    }

    #[test]
    fn flags_get_defaults_and_reject_junk() {
        let spec = [("--relay", Some("127.0.0.1:7500")), ("--target", None)];
        let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();

        let flags = parse_flags(args(&["--target", "127.0.0.1:18080"]), &spec).unwrap();
        assert_eq!(flags["--relay"], "127.0.0.1:7500");
        assert_eq!(flags["--target"], "127.0.0.1:18080");

        assert!(
            parse_flags(args(&[]), &spec)
                .unwrap_err()
                .contains("--target is required")
        );
        assert!(
            parse_flags(args(&["--target"]), &spec)
                .unwrap_err()
                .contains("needs a value")
        );
        assert!(
            parse_flags(args(&["--nope", "x"]), &spec)
                .unwrap_err()
                .contains("unknown")
        );
    }
}
