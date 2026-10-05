use std::net::TcpListener;
use std::process::exit;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use miniredis::{Db, serve};

const DEFAULT_ADDR: &str = "127.0.0.1:6399";
/// Like Redis's `timeout` setting: drop a client that stays silent this long.
const IDLE_TIMEOUT: Duration = Duration::from_secs(300);

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let addr = match args.as_slice() {
        [] => DEFAULT_ADDR,
        [addr] if !addr.starts_with('-') => addr.as_str(),
        _ => {
            eprintln!("usage: miniredis [ADDR]   (default {DEFAULT_ADDR})");
            exit(2);
        }
    };
    let listener = TcpListener::bind(addr).unwrap_or_else(|e| {
        eprintln!("miniredis: cannot listen on {addr}: {e}");
        exit(1);
    });
    eprintln!("listening on {addr}; try: cargo run --bin miniredis-cli -- --addr {addr} PING");
    serve(listener, Arc::new(Mutex::new(Db::new())), IDLE_TIMEOUT);
}
