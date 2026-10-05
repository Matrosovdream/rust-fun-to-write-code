use std::net::TcpListener;
use std::process::exit;

use httpproxy::{CONNECT_TIMEOUT, Config, serve};

const USAGE: &str = "usage: httpproxy [ADDR] [--block HOST]...    (default 127.0.0.1:8888)";

fn usage() -> ! {
    eprintln!("{USAGE}");
    exit(2);
}

fn main() {
    let mut addr = None;
    let mut blocked = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--block" {
            // `--block` consumes the next argument as its value.
            blocked.push(args.next().unwrap_or_else(|| usage()));
        } else if arg.starts_with('-') || addr.is_some() {
            usage();
        } else {
            addr = Some(arg);
        }
    }
    let addr = addr.unwrap_or_else(|| "127.0.0.1:8888".to_string());

    let listener = match TcpListener::bind(&addr) {
        Ok(listener) => listener,
        Err(e) => {
            eprintln!("httpproxy: can't listen on {addr}: {e}");
            exit(1);
        }
    };
    eprintln!("listening on {addr} (try: curl -x http://{addr} http://example.com/)");
    if !blocked.is_empty() {
        eprintln!("blocking: {}", blocked.join(", "));
    }
    serve(
        listener,
        Config {
            blocked,
            connect_timeout: CONNECT_TIMEOUT,
        },
    );
}
