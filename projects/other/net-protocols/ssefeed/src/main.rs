use std::net::TcpListener;
use std::process::exit;
use std::sync::{Arc, Mutex};

use ssefeed::{HEARTBEAT, Hub, serve};

const USAGE: &str = "usage: ssefeed [ADDR]    (default 127.0.0.1:8182)";

fn main() {
    let mut args = std::env::args().skip(1);
    let addr = args.next().unwrap_or_else(|| "127.0.0.1:8182".to_string());
    if addr.starts_with('-') || args.next().is_some() {
        eprintln!("{USAGE}");
        exit(2);
    }
    let listener = match TcpListener::bind(&addr) {
        Ok(listener) => listener,
        Err(e) => {
            eprintln!("ssefeed: can't listen on {addr}: {e}");
            exit(1);
        }
    };
    eprintln!("listening on http://{addr} (try: curl -N {addr}/events)");
    serve(listener, Arc::new(Mutex::new(Hub::default())), HEARTBEAT);
}
