//! A tiny HTTP server to balance across: answers every request with
//! `hello from <address>`, so you can see which backend served you.

use std::net::TcpListener;
use std::{env, io, process};

fn main() -> io::Result<()> {
    let args: Vec<String> = env::args().skip(1).collect();
    let addr = match args.as_slice() {
        [] => "127.0.0.1:9401",
        [addr] if !addr.starts_with('-') => addr.as_str(),
        _ => {
            eprintln!("usage: loadbal-backend [LISTEN_ADDR]   (default 127.0.0.1:9401)");
            process::exit(2);
        }
    };
    let listener = TcpListener::bind(addr)?;
    eprintln!("loadbal-backend listening on {}", listener.local_addr()?);
    loadbal::serve_hello(listener);
    Ok(())
}
