use std::net::TcpListener;
use std::process::exit;
use std::time::Duration;

use smtpd::{Config, serve};

const DEFAULT_ADDR: &str = "127.0.0.1:2525";
const DEFAULT_MAILDIR: &str = "maildir";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (addr, maildir) = match args.as_slice() {
        [] => (DEFAULT_ADDR, DEFAULT_MAILDIR),
        [addr] if !addr.starts_with('-') => (addr.as_str(), DEFAULT_MAILDIR),
        [addr, dir] if !addr.starts_with('-') => (addr.as_str(), dir.as_str()),
        _ => {
            eprintln!("usage: smtpd [ADDR] [MAILDIR]   (default {DEFAULT_ADDR} {DEFAULT_MAILDIR})");
            exit(2);
        }
    };
    let listener = TcpListener::bind(addr).unwrap_or_else(|e| {
        eprintln!("smtpd: cannot listen on {addr}: {e}");
        exit(1);
    });
    eprintln!("listening on {addr}, saving mail to {maildir}/");
    // RFC 5321 suggests a 5-minute server timeout.
    let config = Config {
        maildir: maildir.into(),
        timeout: Duration::from_secs(300),
    };
    serve(listener, config);
}
