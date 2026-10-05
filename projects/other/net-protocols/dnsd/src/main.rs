//! The server binary: `dnsd [ADDR] [ZONE_FILE]`.

use std::net::UdpSocket;
use std::process::ExitCode;

use dnsd::Zone;

const USAGE: &str = "usage: dnsd [ADDR] [ZONE_FILE]   (defaults: 127.0.0.1:10053 zone.txt)";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() > 2 || args.iter().any(|a| a.starts_with('-')) {
        eprintln!("{USAGE}");
        return ExitCode::FAILURE;
    }
    let addr = args.first().map_or("127.0.0.1:10053", String::as_str);
    let zone_path = args.get(1).map_or("zone.txt", String::as_str);

    let zone = match std::fs::read_to_string(zone_path) {
        Ok(text) => Zone::parse(&text),
        Err(e) => Err(e.to_string()),
    };
    let zone = match zone {
        Ok(zone) => zone,
        Err(e) => {
            eprintln!("dnsd: {zone_path}: {e}");
            return ExitCode::FAILURE;
        }
    };

    let socket = match UdpSocket::bind(addr) {
        Ok(socket) => socket,
        Err(e) => {
            eprintln!("dnsd: cannot bind {addr}: {e}");
            return ExitCode::FAILURE;
        }
    };
    eprintln!(
        "dnsd listening on {addr} (udp), {} records from {zone_path}",
        zone.record_count()
    );
    if let Err(e) = dnsd::serve(socket, &zone) {
        eprintln!("dnsd: {e}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
