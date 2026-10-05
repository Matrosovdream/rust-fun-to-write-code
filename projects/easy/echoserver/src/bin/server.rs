use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use echoserver::{Stats, serve};

fn main() -> std::io::Result<()> {
    let addr = std::env::args().nth(1).unwrap_or_else(|| "127.0.0.1:7878".to_string());
    let listener = TcpListener::bind(&addr)?;
    println!("listening on {addr} — try: cargo run --bin client {addr}");
    serve(listener, Arc::new(Mutex::new(Stats::default())));
    Ok(())
}
