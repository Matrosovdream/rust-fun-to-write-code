use std::net::TcpListener;
use std::process::exit;

const DEFAULT_ADDR: &str = "127.0.0.1:7000";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // Slice patterns: match on how many arguments there are and what they look like.
    let addr = match args.as_slice() {
        [] => DEFAULT_ADDR,
        [addr] if !addr.starts_with('-') => addr.as_str(),
        _ => {
            eprintln!("usage: linechat [ADDR]   (default {DEFAULT_ADDR})");
            exit(2);
        }
    };
    let listener = TcpListener::bind(addr).unwrap_or_else(|e| {
        eprintln!("linechat: cannot listen on {addr}: {e}");
        exit(1);
    });
    let (host, port) = addr.rsplit_once(':').unwrap_or((addr, ""));
    eprintln!("listening on {addr}; try: nc {host} {port}");
    linechat::serve(listener);
}
