use std::net::UdpSocket;
use std::process::ExitCode;
use std::time::Duration;

use udpping::{ServerConfig, XorShift, serve};

const USAGE: &str = "usage: udpping [ADDR] [--drop P] [--delay MS]
  ADDR defaults to 127.0.0.1:7300; P is a probability from 0.0 to 1.0";

/// Returns the address and config, or `None` for any bad argument.
fn parse_args() -> Option<(String, f64, Duration)> {
    let mut addr = None;
    let mut drop = 0.0;
    let mut delay = Duration::ZERO;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--drop" => drop = args.next()?.parse().ok()?,
            "--delay" => delay = Duration::from_millis(args.next()?.parse().ok()?),
            _ if addr.is_none() && !arg.starts_with('-') => addr = Some(arg),
            _ => return None,
        }
    }
    // `contains` on a range also rejects NaN, which every comparison fails.
    if !(0.0..=1.0).contains(&drop) {
        return None;
    }
    Some((
        addr.unwrap_or_else(|| "127.0.0.1:7300".to_string()),
        drop,
        delay,
    ))
}

fn main() -> ExitCode {
    let Some((addr, drop, delay)) = parse_args() else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let socket = match UdpSocket::bind(&addr) {
        Ok(socket) => socket,
        Err(e) => {
            eprintln!("udpping: cannot bind {addr}: {e}");
            return ExitCode::FAILURE;
        }
    };
    eprintln!(
        "listening on {addr}/udp (drop {:.0}%, delay {} ms) — try: cargo run --bin udpping-client",
        drop * 100.0,
        delay.as_millis()
    );

    // Seeded from the clock so each run drops a different set of packets.
    let seed = XorShift::from_clock().next_u64();
    if let Err(e) = serve(socket, ServerConfig { drop, delay, seed }) {
        eprintln!("udpping: {e}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
