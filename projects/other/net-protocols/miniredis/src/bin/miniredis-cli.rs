//! miniredis-cli — a tiny redis-cli.
//!
//! ```text
//! miniredis-cli [--addr HOST:PORT] [COMMAND ARG...]
//! ```
//!
//! With a command, it sends the command, prints the reply, and exits.
//! Without one, it reads commands from stdin, one per line. It only shows a
//! prompt when stdin is a terminal, so piped input gives clean output.

use std::io::{self, BufRead, IsTerminal, Read, Write};
use std::net::TcpStream;
use std::process::exit;

use miniredis::{Frame, parse};

const DEFAULT_ADDR: &str = "127.0.0.1:6399";

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut addr = DEFAULT_ADDR.to_string();
    if args.first().is_some_and(|a| a == "--addr") {
        if args.len() < 2 {
            usage();
        }
        addr = args[1].clone();
        args.drain(..2);
    }
    if args.first().is_some_and(|a| a.starts_with('-')) {
        usage();
    }

    let stream = TcpStream::connect(&addr).unwrap_or_else(|e| {
        eprintln!("miniredis-cli: cannot connect to {addr}: {e}");
        exit(1);
    });
    let mut conn = Conn {
        stream,
        buf: Vec::new(),
    };

    if args.is_empty() {
        repl(&mut conn, &addr);
    } else {
        match conn.call(&args) {
            Ok(reply) => println!("{}", render(&reply)),
            Err(e) => {
                eprintln!("miniredis-cli: {e}");
                exit(1);
            }
        }
    }
}

fn usage() -> ! {
    eprintln!(
        "usage: miniredis-cli [--addr HOST:PORT] [COMMAND ARG...]   (default {DEFAULT_ADDR})"
    );
    exit(2);
}

fn repl(conn: &mut Conn, addr: &str) {
    let interactive = io::stdin().is_terminal();
    let mut lines = io::stdin().lock().lines();
    loop {
        if interactive {
            print!("{addr}> ");
            let _ = io::stdout().flush(); // print! doesn't flush without a newline
        }
        let Some(Ok(line)) = lines.next() else {
            break; // EOF (Ctrl-D) or unreadable input
        };
        let words: Vec<&str> = line.split_whitespace().collect();
        match words.first() {
            None => continue,
            Some(w) if w.eq_ignore_ascii_case("quit") || w.eq_ignore_ascii_case("exit") => break,
            Some(_) => {}
        }
        match conn.call(&words) {
            Ok(reply) => println!("{}", render(&reply)),
            Err(e) => {
                eprintln!("miniredis-cli: {e}");
                break;
            }
        }
    }
}

struct Conn {
    stream: TcpStream,
    /// Bytes read but not parsed yet.
    buf: Vec<u8>,
}

impl Conn {
    /// Sends one command as an array of bulk strings and waits for its reply.
    fn call(&mut self, args: &[impl AsRef<str>]) -> io::Result<Frame> {
        let parts = args
            .iter()
            .map(|a| Frame::Bulk(a.as_ref().as_bytes().to_vec()));
        let mut request = Vec::new();
        Frame::Array(parts.collect()).encode(&mut request);
        self.stream.write_all(&request)?;

        // The same streaming parser as the server: read until a whole reply is here.
        let mut chunk = [0u8; 4096];
        loop {
            match parse(&self.buf) {
                Ok(Some((frame, used))) => {
                    self.buf.drain(..used);
                    return Ok(frame);
                }
                Ok(None) => {}
                Err(e) => return Err(io::Error::other(e.0)),
            }
            let n = self.stream.read(&mut chunk)?;
            if n == 0 {
                return Err(io::Error::other("server closed the connection"));
            }
            self.buf.extend_from_slice(&chunk[..n]);
        }
    }
}

/// Formats a reply the way redis-cli does.
fn render(frame: &Frame) -> String {
    match frame {
        Frame::Simple(s) => s.clone(),
        Frame::Error(e) => format!("(error) {e}"),
        Frame::Integer(n) => format!("(integer) {n}"),
        // Debug formatting adds the quotes and escapes unprintable characters.
        Frame::Bulk(bytes) => format!("{:?}", String::from_utf8_lossy(bytes)),
        Frame::Null => "(nil)".into(),
        Frame::Array(items) if items.is_empty() => "(empty array)".into(),
        Frame::Array(items) => {
            let lines: Vec<String> = items
                .iter()
                .enumerate()
                .map(|(i, item)| format!("{}) {}", i + 1, render(item)))
                .collect();
            lines.join("\n")
        }
    }
}
