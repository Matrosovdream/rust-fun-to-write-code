//! One request per run: connect, send the magic and one frame, print the
//! reply, exit. Exit status is 0 for OK, 1 for NOT_FOUND / ERROR.

use std::io::{self, Write};
use std::net::{SocketAddr, TcpStream};
use std::process::ExitCode;
use std::time::Duration;

use binkv::{MAGIC, MAX_STR, Request, Response, frame, read_frame};

const USAGE: &str =
    "usage: binkv-client [ADDR] [--hex] <ping | get KEY | put KEY VALUE | del KEY | list>
  ADDR defaults to 127.0.0.1:7100; --hex dumps the raw bytes sent (>) and received (<) to stderr";

fn main() -> ExitCode {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let hex = args.iter().any(|a| a == "--hex");
    args.retain(|a| a != "--hex");
    // The address is optional, so only treat the first word as one if it
    // really parses as `ip:port`; otherwise it's the command.
    let addr = match args.first() {
        Some(first) if first.parse::<SocketAddr>().is_ok() => args.remove(0),
        _ => "127.0.0.1:7100".to_string(),
    };
    if args.iter().any(|a| a.len() > MAX_STR) {
        eprintln!("binkv-client: keys and values are limited to {MAX_STR} bytes");
        return ExitCode::from(2);
    }
    let Some(request) = parse_command(&args) else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };

    match run(&addr, &request, hex) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("binkv-client: {addr}: {e}");
            ExitCode::FAILURE
        }
    }
}

fn parse_command(args: &[String]) -> Option<Request> {
    // Slice patterns match on both the length and the contents at once.
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    let bytes = |s: &str| s.as_bytes().to_vec();
    let request = match words.as_slice() {
        ["ping"] => Request::Ping,
        ["list"] => Request::List,
        ["get", key] => Request::Get(bytes(key)),
        ["put", key, value] => Request::Put(bytes(key), bytes(value)),
        ["del", key] => Request::Del(bytes(key)),
        _ => return None,
    };
    Some(request)
}

fn run(addr: &str, request: &Request, hex: bool) -> io::Result<ExitCode> {
    let mut stream = TcpStream::connect(addr)?;
    // Don't hang forever on a server that never answers.
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;

    let request_frame = frame(&request.encode());
    if hex {
        eprintln!("> {}", hex_bytes(&MAGIC));
        eprintln!("> {}", hex_bytes(&request_frame));
    }
    let mut out = MAGIC.to_vec();
    out.extend_from_slice(&request_frame);
    stream.write_all(&out)?;

    let Some(body) = read_frame(&mut stream)? else {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "server closed the connection",
        ));
    };
    if hex {
        eprintln!("< {}", hex_bytes(&frame(&body)));
    }
    let response =
        Response::decode(&body).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

    let mut stdout = io::stdout().lock();
    match (request, response) {
        (_, Response::Error(message)) => {
            eprintln!("error: {message}");
            return Ok(ExitCode::FAILURE);
        }
        (_, Response::NotFound) => {
            eprintln!("(not found)");
            return Ok(ExitCode::FAILURE);
        }
        (Request::Put(..) | Request::Del(_), Response::Ok(_)) => writeln!(stdout, "OK")?,
        // PING, GET and LIST: print each string as-is, one per line. Raw
        // bytes, not a lossy String, so binary values come out unchanged.
        (_, Response::Ok(items)) => {
            for item in items {
                stdout.write_all(&item)?;
                stdout.write_all(b"\n")?;
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn hex_bytes(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}
