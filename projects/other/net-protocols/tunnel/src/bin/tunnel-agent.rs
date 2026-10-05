//! The private side of the tunnel. See the crate docs for the design.

use std::{env, process};

use tunnel::{Config, parse_flags, run_agent};

const USAGE: &str = "usage: tunnel-agent --target ADDR [--relay ADDR] [--token TOKEN]
  defaults: --relay 127.0.0.1:7500 --token dev";

#[tokio::main]
async fn main() {
    let spec = [
        ("--relay", Some("127.0.0.1:7500")),
        ("--target", None),
        ("--token", Some("dev")),
    ];
    let flags = parse_flags(env::args().skip(1), &spec).unwrap_or_else(|e| {
        eprintln!("tunnel-agent: {e}\n{USAGE}");
        process::exit(2);
    });

    let (relay, target) = (&flags["--relay"], &flags["--target"]);
    eprintln!("tunnel-agent: relay {relay}, target {target}");
    let config = Config {
        token: flags["--token"].clone(),
        ..Config::default()
    };
    // `run_agent` only returns when the relay refuses us for good.
    let err = run_agent(relay, target, &config).await;
    eprintln!("tunnel-agent: {err}");
    process::exit(1);
}
