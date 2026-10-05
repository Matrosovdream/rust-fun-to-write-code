//! Makes one call and prints the result. Each PARAM is parsed as JSON when
//! it can be (`1`, `true`, `[1,2]`, `"x"`), and is sent as a string
//! otherwise, so `echo hi 2` sends `["hi", 2]`.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::process::ExitCode;
use std::time::Duration;

use jsonrpc::MAX_LINE;
use serde_json::{Value, json};

const USAGE: &str = "usage: jsonrpc-client [ADDR] METHOD [PARAM...]
  ADDR defaults to 127.0.0.1:7200; e.g. jsonrpc-client add 1 2";

fn main() -> ExitCode {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    // The address is optional: only treat the first word as one if it
    // really parses as `ip:port`.
    let addr = match args.first() {
        Some(first) if first.parse::<SocketAddr>().is_ok() => args.remove(0),
        _ => "127.0.0.1:7200".to_string(),
    };
    let Some((method, params)) = args.split_first() else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    if method.starts_with('-') {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    }

    let params: Vec<Value> = params
        .iter()
        .map(|p| serde_json::from_str(p).unwrap_or_else(|_| Value::String(p.clone())))
        .collect();
    let mut request = json!({ "jsonrpc": "2.0", "method": method, "id": 1 });
    if !params.is_empty() {
        request["params"] = Value::Array(params);
    }

    let reply = match call(&addr, &request) {
        Ok(reply) => reply,
        Err(e) => {
            eprintln!("jsonrpc-client: {addr}: {e}");
            return ExitCode::FAILURE;
        }
    };
    if let Some(result) = reply.get("result") {
        println!("{result}");
        ExitCode::SUCCESS
    } else {
        let error = &reply["error"];
        eprint!(
            "error {}: {}",
            error["code"],
            error["message"].as_str().unwrap_or("?")
        );
        match error.get("data") {
            Some(data) => eprintln!(" ({data})"),
            None => eprintln!(),
        }
        ExitCode::FAILURE
    }
}

/// Sends one request line and reads one reply line.
fn call(addr: &str, request: &Value) -> io::Result<Value> {
    let mut stream = TcpStream::connect(addr)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    writeln!(stream, "{request}")?;

    let mut line = String::new();
    BufReader::new(stream)
        .take(MAX_LINE as u64)
        .read_line(&mut line)?;
    if line.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "server closed the connection",
        ));
    }
    serde_json::from_str(&line).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}
