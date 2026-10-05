//! The public side of the tunnel. See the crate docs for the design.

use std::{env, io, process};

use tokio::net::TcpListener;
use tunnel::{Config, parse_flags, run_relay};

const USAGE: &str = "usage: tunnel-relay [--control ADDR] [--public ADDR] [--token TOKEN]
  defaults: --control 127.0.0.1:7500 --public 127.0.0.1:8090 --token dev";

#[tokio::main]
async fn main() -> io::Result<()> {
    let spec = [
        ("--control", Some("127.0.0.1:7500")),
        ("--public", Some("127.0.0.1:8090")),
        ("--token", Some("dev")),
    ];
    let flags = parse_flags(env::args().skip(1), &spec).unwrap_or_else(|e| {
        eprintln!("tunnel-relay: {e}\n{USAGE}");
        process::exit(2);
    });

    let control = TcpListener::bind(&flags["--control"]).await?;
    let public = TcpListener::bind(&flags["--public"]).await?;
    eprintln!(
        "tunnel-relay listening on {} (control) and {} (public)",
        control.local_addr()?,
        public.local_addr()?
    );
    let config = Config {
        token: flags["--token"].clone(),
        ..Config::default()
    };
    run_relay(control, public, config).await;
    Ok(())
}
