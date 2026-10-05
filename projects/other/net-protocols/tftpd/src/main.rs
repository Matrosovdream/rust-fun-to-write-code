//! The server binary: `tftpd [ADDR] [ROOT_DIR]`.

use std::net::UdpSocket;
use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "usage: tftpd [ADDR] [ROOT_DIR]   (defaults: 127.0.0.1:6969 files)";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() > 2 || args.iter().any(|a| a.starts_with('-')) {
        eprintln!("{USAGE}");
        return ExitCode::FAILURE;
    }
    let addr = args.first().map_or("127.0.0.1:6969", String::as_str);
    let root = PathBuf::from(args.get(1).map_or("files", String::as_str));
    if !root.is_dir() {
        eprintln!("tftpd: {} is not a directory\n{USAGE}", root.display());
        return ExitCode::FAILURE;
    }

    let socket = match UdpSocket::bind(addr) {
        Ok(socket) => socket,
        Err(e) => {
            eprintln!("tftpd: cannot bind {addr}: {e}");
            return ExitCode::FAILURE;
        }
    };
    eprintln!(
        "tftpd listening on {addr} (udp), serving {}",
        root.display()
    );
    if let Err(e) = tftpd::serve(socket, root, tftpd::Config::default()) {
        eprintln!("tftpd: {e}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
