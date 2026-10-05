//! `restapi [ADDR]`: the todo API on ADDR (default 127.0.0.1:8181).

use std::net::TcpListener;
use std::process::ExitCode;

use restapi::todos::SharedStore;
use restapi::{WORKERS, app, serve};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() > 1 || args.iter().any(|a| a.starts_with('-')) {
        eprintln!("usage: restapi [ADDR]    (default: 127.0.0.1:8181)");
        return ExitCode::from(2);
    }
    let addr = args.first().map_or("127.0.0.1:8181", String::as_str);
    let listener = match TcpListener::bind(addr) {
        Ok(listener) => listener,
        Err(e) => {
            eprintln!("restapi: cannot listen on {addr}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let local = listener
        .local_addr()
        .map_or(addr.to_string(), |a| a.to_string());
    eprintln!("listening on http://{local}");
    serve(listener, app(SharedStore::default()), WORKERS);
    ExitCode::SUCCESS
}
