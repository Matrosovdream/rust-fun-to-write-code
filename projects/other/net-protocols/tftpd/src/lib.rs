//! tftpd — a TFTP server (RFC 1350).
//!
//! TFTP is file transfer cut down to the bone: no login and no directory
//! listing, just "send me this file" or "take this file", over UDP. UDP can
//! lose or duplicate datagrams, so TFTP builds its own small reliability
//! layer on top: every DATA packet must be acknowledged before the next one
//! goes out ("lock-step"), and a packet that isn't answered in time is sent
//! again.
//!
//! # Packets (RFC 1350 section 5)
//!
//! Every packet starts with a 2-byte big-endian opcode. Strings end with a
//! zero byte.
//!
//! ```text
//!   RRQ / WRQ   +-------+----------+---+--------+---+
//!               | 01/02 | filename | 0 |  mode  | 0 |   mode: "octet" or "netascii"
//!               +-------+----------+---+--------+---+
//!                2 bytes   string   1    string   1
//!
//!   DATA        +-------+---------+--------------------+
//!               |  03   | block # |  data, 0..512 bytes |
//!               +-------+---------+--------------------+
//!
//!   ACK         +-------+---------+
//!               |  04   | block # |
//!               +-------+---------+
//!
//!   ERROR       +-------+-----------+---------+---+
//!               |  05   | errorcode | message | 0 |
//!               +-------+-----------+---------+---+
//! ```
//!
//! For example, macOS `tftp` asks for `hello.txt` with these bytes (the
//! `tsize`/`rollover` pairs are RFC 2347 options, which we ignore):
//!
//! ```text
//!   00 01 68 65 6c 6c 6f 2e 74 78 74 00 6f 63 74 65 74 00 74 73 69 7a 65 00 30 00 ...
//!   RRQ   h  e  l  l  o  .  t  x  t  \0 o  c  t  e  t  \0 t  s  i  z  e  \0 0  \0
//! ```
//!
//! # A download (RRQ), RFC 1350 sections 4 and 6
//!
//! ```text
//!   client :50000                  server :6969          server :61234
//!     | RRQ "lorem.txt" "octet" ---> |                         |
//!     |                              | new thread, new port -> |
//!     | <------------------------------------ DATA 1 (512 bytes)
//!     | ACK 1 ------------------------------------------------> |
//!     | <------------------------------------ DATA 2 (512 bytes)
//!     |            ... no ACK arrives within 1 s ...            |
//!     | <------------------------------------ DATA 2 (again)    |
//!     | ACK 2 ------------------------------------------------> |
//!     | <------------------------------------ DATA 3 (200 bytes)  shorter than 512:
//!     | ACK 3 ------------------------------------------------> |  the last block
//! ```
//!
//! An upload (WRQ) is the mirror image: the server answers with ACK 0, the
//! client sends DATA 1, the server answers ACK 1, and so on.
//!
//! The rules that matter:
//!
//! - **Transfer IDs (section 4).** The well-known port only receives
//!   requests. Each transfer runs on a fresh port on both sides, and that
//!   pair of ports identifies it. A packet that reaches a transfer port from
//!   any other address gets ERROR 5 ("unknown transfer ID"), and the
//!   transfer carries on.
//! - **End of file (section 6).** A DATA packet shorter than 512 bytes is the
//!   last one. A file whose size is an exact multiple of 512 therefore ends
//!   with an empty DATA packet.
//! - **Timeouts.** If the reply doesn't come, resend the last packet. Never
//!   resend because of a *duplicate* ACK: both sides would then double every
//!   packet forever (the "Sorcerer's Apprentice" bug, RFC 1123 4.2.3.1).
//! - **Block numbers** are 16 bits, so files over 32 MB wrap from 65535 back
//!   to 0. `u16::wrapping_add` does exactly that.
//!
//! Each transfer runs on its own thread with its own socket, so a slow
//! client never holds up the others.

pub mod packet;

pub use packet::{BLOCK_SIZE, ErrorCode, Packet};

use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::path::{Component, Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

/// Retransmission policy. Tests use a short timeout to stay fast.
#[derive(Clone, Copy, Debug)]
pub struct Config {
    /// How long to wait for a reply before resending the last packet.
    pub timeout: Duration,
    /// How many times a packet is sent in total before giving up.
    pub tries: u32,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            timeout: Duration::from_secs(1),
            tries: 5,
        }
    }
}

/// Maps a filename from the network to a path inside `root`, or `None` if
/// it could point anywhere else.
///
/// Only plain names are allowed: `/etc/passwd`, `../secret` and `a/../../b`
/// are all refused. `Path::components` already knows about roots, `..` and
/// `.`, so we don't parse slashes ourselves. Symlinks inside `root` are
/// still followed; a stricter server would canonicalize the path and check
/// that it starts with the canonical root.
pub fn resolve(root: &Path, filename: &str) -> Option<PathBuf> {
    let path = Path::new(filename);
    let plain = path.components().all(|c| matches!(c, Component::Normal(_)));
    (plain && !filename.is_empty()).then(|| root.join(path))
}

/// Runs the server on an already-bound socket (the well-known port).
/// Requests start a transfer thread. Anything else gets an ERROR reply.
/// Only returns if the socket itself fails.
pub fn serve(socket: UdpSocket, root: PathBuf, config: Config) -> io::Result<()> {
    // Transfer sockets bind the same IP as this one, so replies come from
    // the address the client talked to.
    let ip = socket.local_addr()?.ip();
    let mut buf = [0u8; 1024];
    loop {
        let (n, peer) = socket.recv_from(&mut buf)?;
        let (direction, filename, mode) = match Packet::parse(&buf[..n]) {
            Ok(Packet::Rrq { filename, mode }) => (Direction::Download, filename, mode),
            Ok(Packet::Wrq { filename, mode }) => (Direction::Upload, filename, mode),
            // Never answer an ERROR with an ERROR: two servers could bounce
            // them back and forth forever.
            Ok(Packet::Error { .. }) => {
                eprintln!("[{peer}] ignored stray ERROR packet");
                continue;
            }
            Ok(_) => {
                refuse(&socket, peer, "expected RRQ or WRQ");
                continue;
            }
            Err(why) => {
                refuse(&socket, peer, why);
                continue;
            }
        };
        let root = root.clone();
        thread::spawn(move || {
            run_transfer(ip, peer, direction, &filename, &mode, &root, config);
        });
    }
}

/// Answers a packet that doesn't belong on the well-known port.
fn refuse(socket: &UdpSocket, peer: SocketAddr, why: &str) {
    eprintln!("[{peer}] refused: {why}");
    let reply = Packet::error(ErrorCode::IllegalOperation, why);
    // Best effort: nobody acknowledges an ERROR packet anyway.
    let _ = socket.send_to(&reply.encode(), peer);
}

#[derive(Clone, Copy, Debug)]
enum Direction {
    Download,
    Upload,
}

/// Why a transfer stopped before the end.
#[derive(Debug)]
enum Abort {
    /// We refuse or failed: the client is told with an ERROR packet.
    Error(ErrorCode, String),
    /// The client is gone or gave up; there's nobody to tell.
    Silent(String),
}

impl Abort {
    fn refuse(code: ErrorCode, message: &str) -> Abort {
        Abort::Error(code, message.to_string())
    }
}

/// Turns a file-system error into the TFTP error the client should see.
fn file_error(e: io::Error) -> Abort {
    match e.kind() {
        io::ErrorKind::NotFound => Abort::refuse(ErrorCode::FileNotFound, "File not found"),
        io::ErrorKind::PermissionDenied => {
            Abort::refuse(ErrorCode::AccessViolation, "Access violation")
        }
        io::ErrorKind::AlreadyExists => Abort::refuse(ErrorCode::FileExists, "File already exists"),
        _ => Abort::Error(ErrorCode::NotDefined, e.to_string()),
    }
}

/// One transfer: our own socket (the server's TID), the client's address
/// (the client's TID), and the retransmit policy.
struct Transfer {
    socket: UdpSocket,
    peer: SocketAddr,
    config: Config,
}

impl Transfer {
    /// Fire and forget, for packets nobody acknowledges (ERRORs, the final
    /// ACK). If one is lost, the other side's timeout deals with it.
    fn send(&self, to: SocketAddr, packet: &Packet) {
        let _ = self.socket.send_to(&packet.encode(), to);
    }

    /// TFTP's whole reliability layer in one function: send `packet`, then
    /// wait for the reply that `accept` picks out (it returns `Some`). If
    /// nothing acceptable arrives before the timeout, send the same packet
    /// again, up to `config.tries` times.
    ///
    /// Replies that `accept` rejects are duplicates of earlier packets,
    /// which are normal on UDP. They're ignored, not answered.
    fn lockstep<T>(
        &self,
        packet: &Packet,
        mut accept: impl FnMut(Packet) -> Option<T>,
    ) -> Result<T, Abort> {
        let bytes = packet.encode();
        // One byte more than the largest valid packet, so an oversized
        // datagram is noticed by `parse` instead of silently cut to size.
        let mut buf = [0u8; 4 + BLOCK_SIZE + 1];
        let socket_error = |e: io::Error| Abort::Silent(format!("socket error: {e}"));

        for _ in 0..self.config.tries {
            self.socket
                .send_to(&bytes, self.peer)
                .map_err(socket_error)?;
            // A stray packet must not restart the clock, so we wait until a
            // fixed deadline rather than for a fixed duration.
            let deadline = Instant::now() + self.config.timeout;
            loop {
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    break; // timed out: go round and resend
                }
                self.socket
                    .set_read_timeout(Some(left))
                    .map_err(socket_error)?;
                let (n, from) = match self.socket.recv_from(&mut buf) {
                    Ok(got) => got,
                    // Unix reports a read timeout as WouldBlock, Windows as
                    // TimedOut.
                    Err(e)
                        if matches!(
                            e.kind(),
                            io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                        ) =>
                    {
                        break;
                    }
                    Err(e) => return Err(socket_error(e)),
                };
                if from != self.peer {
                    let error = Packet::error(ErrorCode::UnknownTid, "Unknown transfer ID");
                    self.send(from, &error);
                    continue;
                }
                match Packet::parse(&buf[..n]) {
                    Ok(Packet::Error { code, message }) => {
                        return Err(Abort::Silent(format!(
                            "client sent error {code}: {message}"
                        )));
                    }
                    Ok(reply) => {
                        if let Some(value) = accept(reply) {
                            return Ok(value);
                        }
                    }
                    Err(why) => return Err(Abort::refuse(ErrorCode::IllegalOperation, why)),
                }
            }
        }
        Err(Abort::Silent(format!(
            "no reply after {} tries",
            self.config.tries
        )))
    }
}

/// Runs one transfer to the end and logs a single line about it.
fn run_transfer(
    ip: IpAddr,
    peer: SocketAddr,
    direction: Direction,
    filename: &str,
    mode: &str,
    root: &Path,
    config: Config,
) {
    let (verb, done) = match direction {
        Direction::Download => ("RRQ", "sent"),
        Direction::Upload => ("WRQ", "received"),
    };
    // Binding port 0 asks the OS for a free port: this transfer's TID.
    let socket = match UdpSocket::bind((ip, 0)) {
        Ok(socket) => socket,
        Err(e) => return eprintln!("[{peer}] {verb} {filename}: cannot bind: {e}"),
    };
    let transfer = Transfer {
        socket,
        peer,
        config,
    };

    // netascii is served byte for byte; real netascii would turn "\n" into
    // "\r\n" on the wire. "mail" mode is obsolete.
    let result = if !(mode.eq_ignore_ascii_case("octet") || mode.eq_ignore_ascii_case("netascii")) {
        Err(Abort::refuse(
            ErrorCode::IllegalOperation,
            "Unsupported mode",
        ))
    } else {
        match direction {
            Direction::Download => download(&transfer, root, filename),
            Direction::Upload => upload(&transfer, root, filename),
        }
    };

    match result {
        Ok(bytes) => eprintln!("[{peer}] {verb} {filename}: {done} {bytes} bytes"),
        Err(Abort::Error(code, message)) => {
            transfer.send(peer, &Packet::error(code, &message));
            eprintln!(
                "[{peer}] {verb} {filename}: error {} ({message})",
                code as u16
            );
        }
        Err(Abort::Silent(why)) => eprintln!("[{peer}] {verb} {filename}: aborted ({why})"),
    }
}

/// Serves an RRQ: DATA 1, 2, 3, ... each one resent until its ACK arrives.
fn download(t: &Transfer, root: &Path, filename: &str) -> Result<u64, Abort> {
    let path = resolve(root, filename)
        .ok_or_else(|| Abort::refuse(ErrorCode::AccessViolation, "Access violation"))?;
    let mut file = File::open(&path).map_err(file_error)?;
    // Opening a directory succeeds on Unix; reading it would fail later.
    if !file.metadata().is_ok_and(|m| m.is_file()) {
        return Err(Abort::refuse(ErrorCode::FileNotFound, "File not found"));
    }

    let mut block: u16 = 1;
    let mut total = 0;
    loop {
        let mut data = Vec::with_capacity(BLOCK_SIZE);
        // `take` consumes the reader it's called on. `&mut File` is a reader
        // too, so we hand it that and keep the file for the next block.
        // `read_to_end` keeps reading until the block is full or EOF.
        (&mut file)
            .take(BLOCK_SIZE as u64)
            .read_to_end(&mut data)
            .map_err(file_error)?;
        let last = data.len() < BLOCK_SIZE;
        total += data.len() as u64;

        t.lockstep(&Packet::Data { block, data }, |reply| {
            matches!(reply, Packet::Ack { block: acked } if acked == block).then_some(())
        })?;
        if last {
            return Ok(total);
        }
        block = block.wrapping_add(1);
    }
}

/// Serves a WRQ. The data goes into a temporary file that is renamed into
/// place only when complete, so nobody can download half an upload and a
/// failed upload leaves nothing behind.
fn upload(t: &Transfer, root: &Path, filename: &str) -> Result<u64, Abort> {
    let path = resolve(root, filename)
        .ok_or_else(|| Abort::refuse(ErrorCode::AccessViolation, "Access violation"))?;
    if path.exists() {
        return Err(Abort::refuse(ErrorCode::FileExists, "File already exists"));
    }
    // "notes.txt.61234.part": the transfer's port makes the name unique.
    let port = t.socket.local_addr().map_or(0, |a| a.port());
    let mut tmp = OsString::from(&path);
    tmp.push(format!(".{port}.part"));
    let mut file = File::create_new(&tmp).map_err(file_error)?;

    let (total, last_block) = match receive(t, &mut file) {
        Ok(done) => done,
        Err(abort) => {
            let _ = fs::remove_file(&tmp);
            return Err(abort);
        }
    };
    if let Err(e) = fs::rename(&tmp, &path) {
        let _ = fs::remove_file(&tmp);
        return Err(file_error(e));
    }
    // ACK the last block only now: when the client hears it, the file is
    // really there. If this ACK is lost the client times out even though
    // the upload worked. RFC 1350 suggests lingering to re-send it, which we
    // skip.
    t.send(t.peer, &Packet::Ack { block: last_block });
    Ok(total)
}

/// The receiving half of lock-step: ACK n, wait for DATA n+1, write it,
/// repeat until a short block. Returns the byte count and the last block
/// number; the caller sends the final ACK.
fn receive(t: &Transfer, file: &mut File) -> Result<(u64, u16), Abort> {
    let mut block: u16 = 0; // ACK 0 is the "go ahead" for a WRQ
    let mut total = 0;
    loop {
        let next = block.wrapping_add(1);
        let data = t.lockstep(&Packet::Ack { block }, |reply| match reply {
            Packet::Data { block, data } if block == next => Some(data),
            _ => None,
        })?;
        file.write_all(&data).map_err(file_error)?;
        total += data.len() as u64;
        block = next;
        if data.len() < BLOCK_SIZE {
            return Ok((total, block));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_keeps_plain_names_inside_root() {
        let root = Path::new("/srv/tftp");
        assert_eq!(resolve(root, "hello.txt"), Some(root.join("hello.txt")));
        assert_eq!(resolve(root, "boot/pxe.0"), Some(root.join("boot/pxe.0")));
    }

    #[test]
    fn resolve_refuses_escapes() {
        let root = Path::new("/srv/tftp");
        for bad in ["", "/etc/passwd", "..", "../secret", "a/../../b", "./x"] {
            assert_eq!(resolve(root, bad), None, "accepted {bad:?}");
        }
    }
}
