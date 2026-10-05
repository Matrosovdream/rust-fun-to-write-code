use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs, UdpSocket};
use std::process::ExitCode;
use std::str::FromStr;
use std::time::Duration;

use udpping::{Outcome, PROBE_LEN, PingConfig, ms, ping};

const USAGE: &str = "usage: udpping-client [ADDR] [-c COUNT] [-i INTERVAL_MS] [-W TIMEOUT_MS]
  ADDR defaults to 127.0.0.1:7300; defaults: -c 5 -i 1000 -W 1000";

fn parse_args() -> Option<(String, PingConfig)> {
    // A generic helper: `T` is whatever type the caller's field needs, as
    // long as it can be parsed from a string.
    fn value<T: FromStr>(args: &mut impl Iterator<Item = String>) -> Option<T> {
        args.next()?.parse().ok()
    }

    let mut addr = None;
    let mut config = PingConfig {
        count: 5,
        interval: Duration::from_millis(1000),
        timeout: Duration::from_millis(1000),
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-c" => config.count = value(&mut args)?,
            "-i" => config.interval = Duration::from_millis(value(&mut args)?),
            "-W" => config.timeout = Duration::from_millis(value(&mut args)?),
            _ if addr.is_none() && !arg.starts_with('-') => addr = Some(arg),
            _ => return None,
        }
    }
    if config.timeout.is_zero() {
        return None;
    }
    Some((addr.unwrap_or_else(|| "127.0.0.1:7300".to_string()), config))
}

/// Our own socket's address: loopback for a loopback server (no macOS
/// firewall prompt), "any" otherwise. Port 0 lets the OS pick one.
fn local_addr_for(server: SocketAddr) -> SocketAddr {
    let ip: IpAddr = match server.ip() {
        ip if ip.is_loopback() => ip,
        IpAddr::V4(_) => Ipv4Addr::UNSPECIFIED.into(),
        IpAddr::V6(_) => Ipv6Addr::UNSPECIFIED.into(),
    };
    SocketAddr::new(ip, 0)
}

fn main() -> ExitCode {
    let Some((addr, config)) = parse_args() else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    // We compare every reply's source with this address, so resolve a
    // name like `localhost:7300` to one concrete address up front.
    let server = match addr.to_socket_addrs().map(|mut addrs| addrs.next()) {
        Ok(Some(server)) => server,
        Ok(None) | Err(_) => {
            eprintln!("udpping-client: cannot resolve {addr}");
            return ExitCode::from(2);
        }
    };
    let socket = match UdpSocket::bind(local_addr_for(server)) {
        Ok(socket) => socket,
        Err(e) => {
            eprintln!("udpping-client: cannot bind: {e}");
            return ExitCode::FAILURE;
        }
    };

    println!("UDPPING {server}: {PROBE_LEN} data bytes");
    let result = ping(&socket, server, &config, |outcome| match outcome {
        Outcome::Reply { seq, rtt } => {
            println!(
                "{PROBE_LEN} bytes from {server}: seq={seq} time={} ms",
                ms(rtt)
            )
        }
        Outcome::Timeout { seq } => println!("seq={seq} timeout"),
    });
    match result {
        Ok(stats) => {
            println!("\n--- {server} udpping statistics ---\n{}", stats.summary());
            // Like ping: a failure exit status when nothing came back.
            if stats.received > 0 {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(e) => {
            eprintln!("udpping-client: {e}");
            ExitCode::FAILURE
        }
    }
}
