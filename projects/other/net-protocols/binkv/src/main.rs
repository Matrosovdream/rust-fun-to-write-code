use std::net::TcpListener;
use std::process::ExitCode;
use std::time::Duration;

use binkv::{Db, serve};

/// Connections that send nothing for this long get closed.
const IDLE_TIMEOUT: Duration = Duration::from_secs(60);

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let addr = args.next().unwrap_or_else(|| "127.0.0.1:7100".to_string());
    if addr.starts_with('-') || args.next().is_some() {
        eprintln!("usage: binkv [ADDR]    (default 127.0.0.1:7100)");
        return ExitCode::from(2);
    }

    let listener = match TcpListener::bind(&addr) {
        Ok(listener) => listener,
        Err(e) => {
            eprintln!("binkv: cannot listen on {addr}: {e}");
            return ExitCode::FAILURE;
        }
    };
    eprintln!("listening on {addr} — try: cargo run --bin binkv-client -- ping");
    serve(listener, Db::default(), IDLE_TIMEOUT);
    ExitCode::SUCCESS
}
