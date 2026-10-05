//! `httpd [ADDR] [ROOT]`: serve the directory ROOT over HTTP on ADDR.

use std::net::TcpListener;
use std::path::Path;
use std::process::ExitCode;

use httpd::{Config, serve};

const USAGE: &str = "usage: httpd [ADDR] [ROOT]    (defaults: 127.0.0.1:8180 public)";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() > 2 || args.iter().any(|a| a.starts_with('-')) {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    }
    let addr = args.first().map_or("127.0.0.1:8180", String::as_str);
    let root = args.get(1).map_or("public", String::as_str);
    if !Path::new(root).is_dir() {
        eprintln!("httpd: {root} is not a directory\n{USAGE}");
        return ExitCode::from(2);
    }

    let listener = match TcpListener::bind(addr) {
        Ok(listener) => listener,
        Err(e) => {
            eprintln!("httpd: cannot listen on {addr}: {e}");
            return ExitCode::FAILURE;
        }
    };
    // local_addr shows the real port when ADDR asked for port 0.
    let local = listener
        .local_addr()
        .map_or(addr.to_string(), |a| a.to_string());
    eprintln!("listening on http://{local}, serving {root}");
    match serve(listener, Config::new(root)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("httpd: {e}");
            ExitCode::FAILURE
        }
    }
}
