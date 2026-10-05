//! echoserver — a line-based TCP echo server with a few commands.
//!
//! The protocol: one request line in, one reply line out.
//!   /time   -> server time (unix seconds)
//!   /stats  -> connections and lines served so far
//!   /quit   -> goodbye + close
//!   else    -> "you said: <line>"
//!
//! The logic lives in this library so both binaries and the integration
//! tests share it.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

/// Shared across all connection threads: Arc gives shared ownership,
/// Mutex makes the mutation safe. The compiler will not let you forget
/// either one.
#[derive(Debug, Default)]
pub struct Stats {
    pub connections: u64,
    pub lines: u64,
}

pub type SharedStats = Arc<Mutex<Stats>>;

#[derive(Debug, PartialEq)]
pub enum Reply {
    Line(String),
    Goodbye,
}

/// Pure request -> reply logic; no sockets, fully testable.
pub fn respond(line: &str, stats: &SharedStats) -> Reply {
    match line.trim() {
        "/quit" => Reply::Goodbye,
        "/time" => {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock after 1970")
                .as_secs();
            Reply::Line(format!("time: {now}"))
        }
        "/stats" => {
            let stats = stats.lock().expect("stats lock");
            Reply::Line(format!(
                "connections: {}, lines: {}",
                stats.connections, stats.lines
            ))
        }
        other => Reply::Line(format!("you said: {other}")),
    }
}

fn handle(stream: TcpStream, stats: SharedStats) -> std::io::Result<()> {
    stats.lock().expect("stats lock").connections += 1;
    let peer = stream.peer_addr()?;
    let reader = BufReader::new(stream.try_clone()?);
    let mut writer = stream;

    for line in reader.lines() {
        let line = line?;
        stats.lock().expect("stats lock").lines += 1;
        match respond(&line, &stats) {
            Reply::Line(reply) => writeln!(writer, "{reply}")?,
            Reply::Goodbye => {
                writeln!(writer, "goodbye")?;
                break;
            }
        }
    }
    eprintln!("[{peer}] disconnected");
    Ok(())
}

/// Accept loop: one thread per connection. Runs until the listener dies.
pub fn serve(listener: TcpListener, stats: SharedStats) {
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let stats = Arc::clone(&stats);
                thread::spawn(move || {
                    if let Err(e) = handle(stream, stats) {
                        eprintln!("connection error: {e}");
                    }
                });
            }
            Err(e) => eprintln!("accept error: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_stats() -> SharedStats {
        Arc::new(Mutex::new(Stats::default()))
    }

    #[test]
    fn echoes_ordinary_lines() {
        assert_eq!(
            respond("hello there", &fresh_stats()),
            Reply::Line("you said: hello there".into())
        );
    }

    #[test]
    fn quit_says_goodbye() {
        assert_eq!(respond("/quit", &fresh_stats()), Reply::Goodbye);
        assert_eq!(respond("  /quit  ", &fresh_stats()), Reply::Goodbye);
    }

    #[test]
    fn time_is_numeric() {
        let Reply::Line(reply) = respond("/time", &fresh_stats()) else {
            panic!("expected a line");
        };
        let secs: u64 = reply.strip_prefix("time: ").unwrap().parse().unwrap();
        assert!(secs > 1_700_000_000);
    }

    #[test]
    fn stats_reports_counters() {
        let stats = fresh_stats();
        stats.lock().unwrap().connections = 2;
        stats.lock().unwrap().lines = 7;
        assert_eq!(
            respond("/stats", &stats),
            Reply::Line("connections: 2, lines: 7".into())
        );
    }
}
