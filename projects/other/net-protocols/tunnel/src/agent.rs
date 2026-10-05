//! The private side: dials out to the relay, keeps the control connection
//! alive, and opens one data connection per public client.

use std::io;
use std::time::Duration;

use tokio::io::copy_bidirectional;
use tokio::net::TcpStream;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::time::{Instant, interval, sleep, timeout};

use crate::proto::{Msg, MsgReader, send};
use crate::{Config, backoff};

/// Connects, serves, and reconnects with exponential backoff, forever.
/// Returns only if the relay refuses us (`ERR`): retrying won't fix a bad
/// token.
pub async fn run_agent(relay: &str, target: &str, config: &Config) -> io::Error {
    let mut failures = 0;
    loop {
        let reason = match hello(relay, config).await {
            Ok((msgs, write)) => {
                eprintln!("connected to relay {relay}, forwarding to {target}");
                failures = 0;
                match control_loop(msgs, write, relay, target, config).await {
                    Ok(()) => "relay closed the connection".to_string(),
                    Err(e) => e.to_string(),
                }
            }
            Err(e) if e.kind() == io::ErrorKind::PermissionDenied => return e,
            Err(e) => e.to_string(),
        };
        let delay = backoff(failures, config.backoff_min, config.backoff_max);
        failures = failures.saturating_add(1);
        eprintln!("relay {relay}: {reason}; retrying in {delay:?}");
        sleep(delay).await;
    }
}

/// Opens the control connection: `HELLO <token>`, expect `OK`.
async fn hello(
    relay: &str,
    config: &Config,
) -> io::Result<(MsgReader<OwnedReadHalf>, OwnedWriteHalf)> {
    // `timeout` wraps the result in another `Result`: the first `?` is for
    // the deadline (tokio's `Elapsed` converts into an `io::Error`), the
    // second for the connect itself.
    let stream = timeout(config.connect_timeout, TcpStream::connect(relay)).await??;
    let (read, mut write) = stream.into_split();
    let mut msgs = MsgReader::new(read);
    send(&mut write, &Msg::Hello(config.token.clone())).await?;
    match timeout(config.connect_timeout, msgs.next()).await?? {
        Some(Msg::Ok) => Ok((msgs, write)),
        Some(Msg::Err(reason)) => Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("relay refused us: {reason}"),
        )),
        Some(other) => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unexpected {other:?}"),
        )),
        None => Err(io::ErrorKind::UnexpectedEof.into()),
    }
}

/// The agent's half of the control connection: PING on a timer, open a data
/// connection per CONNECT, and treat a silent relay as a dead one.
async fn control_loop(
    mut msgs: MsgReader<OwnedReadHalf>,
    mut write: OwnedWriteHalf,
    relay: &str,
    target: &str,
    config: &Config,
) -> io::Result<()> {
    let mut ping = interval(config.heartbeat);
    let silence_limit = config.heartbeat * 3;
    let silence = sleep(silence_limit);
    tokio::pin!(silence);
    loop {
        // Cancel safety: if `ping.tick()` loses, no tick is consumed (tokio
        // documents this); `MsgReader::next` keeps partial lines; and the
        // silence timer is only borrowed, never dropped.
        tokio::select! {
            _ = ping.tick() => send(&mut write, &Msg::Ping).await?,
            msg = msgs.next() => {
                match msg? {
                    Some(Msg::Connect(id)) => {
                        // A task per data connection: a slow target never
                        // holds up the control connection. These tasks are
                        // detached, so open tunnels survive a reconnect.
                        let (relay, target) = (relay.to_string(), target.to_string());
                        tokio::spawn(open_data(id, relay, target, config.connect_timeout));
                    }
                    Some(Msg::Pong) => {}
                    Some(other) => {
                        let e = format!("unexpected {other:?}");
                        return Err(io::Error::new(io::ErrorKind::InvalidData, e));
                    }
                    None => return Ok(()),
                }
                silence.as_mut().reset(Instant::now() + silence_limit);
            }
            () = &mut silence => {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "no PONG for too long"));
            }
        }
    }
}

async fn open_data(id: u64, relay: String, target: String, connect_timeout: Duration) {
    match splice_data(id, &relay, &target, connect_timeout).await {
        Ok((sent, received)) => {
            eprintln!("#{id} done: {sent} bytes to {target}, {received} bytes back");
        }
        Err(e) => eprintln!("#{id} failed: {e}"),
    }
}

async fn splice_data(
    id: u64,
    relay: &str,
    target: &str,
    connect_timeout: Duration,
) -> io::Result<(u64, u64)> {
    let mut data = timeout(connect_timeout, TcpStream::connect(relay)).await??;
    send(&mut data, &Msg::Data(id)).await?;
    // If the target is down, the early return drops `data`, so the relay
    // closes the public client right away instead of making it wait.
    let mut local = timeout(connect_timeout, TcpStream::connect(target)).await??;
    // Copies both ways at once until both directions hit EOF, passing each
    // EOF on as a half-close.
    copy_bidirectional(&mut data, &mut local).await
}
