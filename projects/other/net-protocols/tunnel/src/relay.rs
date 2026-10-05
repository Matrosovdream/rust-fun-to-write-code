//! The public side. The control port takes agents (`HELLO`) and data
//! connections (`DATA <id>`); the public port takes anyone.

use std::collections::HashMap;
use std::io;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use tokio::io::copy_bidirectional;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinSet;
use tokio::time::{Instant, sleep, timeout};

use crate::Config;
use crate::proto::{Msg, MsgReader, read_first_line, send};

/// CONNECTs that can queue for the agent. Past that, new public clients
/// just wait out their timeout.
const CONNECT_QUEUE: usize = 256;

#[derive(Default)]
struct State {
    /// Where to send `CONNECT` ids while an agent is connected.
    agent: Option<mpsc::Sender<u64>>,
    /// Public connections waiting for their data connection. The oneshot
    /// carries the data connection's socket to the task that holds the
    /// public socket.
    pending: HashMap<u64, oneshot::Sender<TcpStream>>,
    next_id: u64,
}

struct Relay {
    config: Config,
    /// A plain `std` Mutex is right here: every critical section is a few
    /// lines with no `.await`. We *can't* hold it across an `.await` by
    /// mistake, either: `MutexGuard` isn't `Send`, so a spawned task that
    /// kept one alive over an `.await` would not compile.
    state: Mutex<State>,
}

impl Relay {
    fn state(&self) -> MutexGuard<'_, State> {
        // Poisoned only if code panicked while holding the lock; ours can't.
        self.state.lock().expect("relay state lock poisoned")
    }
}

/// Runs the relay until this future is dropped.
///
/// One `select!` loop accepts on both ports. Each connection runs as a task
/// in a `JoinSet`. Dropping the `JoinSet` aborts all of its tasks, so when
/// this future is dropped (a test aborts it, say), every connection goes
/// with it. Nothing outlives the relay.
pub async fn run_relay(control: TcpListener, public: TcpListener, config: Config) {
    let relay = Arc::new(Relay {
        config,
        state: Mutex::default(),
    });
    let mut tasks = JoinSet::new();
    loop {
        // `accept` and `join_next` are cancel-safe: when one branch wins,
        // the losers are dropped without losing a connection.
        tokio::select! {
            accepted = control.accept() => match accepted {
                Ok((stream, peer)) => {
                    tasks.spawn(on_control_port(stream, peer, Arc::clone(&relay)));
                }
                Err(e) => eprintln!("control accept error: {e}"),
            },
            accepted = public.accept() => match accepted {
                Ok((stream, peer)) => {
                    tasks.spawn(on_public_port(stream, peer, Arc::clone(&relay)));
                }
                Err(e) => eprintln!("public accept error: {e}"),
            },
            // Reap finished tasks. With none, `None` doesn't match `Some(_)`,
            // and the branch is just disabled for this round.
            Some(_) = tasks.join_next() => {}
        }
    }
}

/// The first line says what a control-port connection is.
async fn on_control_port(mut stream: TcpStream, peer: SocketAddr, relay: Arc<Relay>) {
    // A connection that never finishes its first line must not linger.
    let first = timeout(relay.config.connect_timeout, read_first_line(&mut stream)).await;
    match first {
        Ok(Ok(Msg::Hello(token))) => agent_session(stream, peer, &token, &relay).await,
        Ok(Ok(Msg::Data(id))) => {
            let waiting = relay.state().pending.remove(&id);
            match waiting {
                // If the public client gave up a moment ago, `send` hands the
                // stream back, and dropping it closes the connection.
                Some(public_task) => drop(public_task.send(stream)),
                None => eprintln!("[{peer}] DATA {id}: no such pending connection"),
            }
        }
        Ok(Ok(other)) => eprintln!("[{peer}] unexpected first message {other:?}, closing"),
        Ok(Err(e)) => eprintln!("[{peer}] bad first line ({e}), closing"),
        Err(_) => eprintln!("[{peer}] no first line in time, closing"),
    }
}

async fn agent_session(stream: TcpStream, peer: SocketAddr, token: &str, relay: &Relay) {
    let (read, mut write) = stream.into_split();
    let (tx, ids) = mpsc::channel(CONNECT_QUEUE);
    let refusal = if token != relay.config.token {
        Some("bad token")
    } else {
        // Check and register under one lock, so two agents can't both win.
        let mut state = relay.state();
        if state.agent.is_some() {
            Some("agent already connected")
        } else {
            // Public clients that arrived while no agent was connected are
            // still waiting: ask for their data connections right away.
            for &id in state.pending.keys() {
                let _ = tx.try_send(id);
            }
            state.agent = Some(tx);
            None
        }
    }; // the guard died with its block, before any `.await`

    if let Some(reason) = refusal {
        eprintln!("[{peer}] agent refused: {reason}");
        let _ = send(&mut write, &Msg::Err(reason.to_string())).await;
        return;
    }
    eprintln!("[{peer}] agent connected");
    let silence_limit = relay.config.heartbeat * 3;
    let result = serve_agent(MsgReader::new(read), write, ids, silence_limit).await;
    relay.state().agent = None;
    match result {
        Ok(()) => eprintln!("[{peer}] agent disconnected"),
        Err(e) => eprintln!("[{peer}] agent dropped: {e}"),
    }
}

/// The relay's half of a control connection: pass on CONNECTs, answer
/// PINGs, and give up on an agent that has gone quiet.
async fn serve_agent(
    mut msgs: MsgReader<OwnedReadHalf>,
    mut write: OwnedWriteHalf,
    mut ids: mpsc::Receiver<u64>,
    silence_limit: Duration,
) -> io::Result<()> {
    send(&mut write, &Msg::Ok).await?;
    // One timer, pinned once and pushed back whenever the agent speaks.
    // Passing `&mut silence` to `select!` keeps it alive across iterations;
    // by value, it would be dropped and restarted every time.
    let silence = sleep(silence_limit);
    tokio::pin!(silence);
    loop {
        // All three branches are cancel-safe: `recv` leaves unreceived ids
        // in the channel, `MsgReader::next` keeps partial lines (see
        // proto.rs), and the timer is only borrowed.
        tokio::select! {
            Some(id) = ids.recv() => send(&mut write, &Msg::Connect(id)).await?,
            msg = msgs.next() => {
                match msg? {
                    Some(Msg::Ping) => send(&mut write, &Msg::Pong).await?,
                    Some(other) => {
                        let e = format!("unexpected {other:?}");
                        return Err(io::Error::new(io::ErrorKind::InvalidData, e));
                    }
                    None => return Ok(()),
                }
                silence.as_mut().reset(Instant::now() + silence_limit);
            }
            () = &mut silence => {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "no PING for too long"));
            }
        }
    }
}

/// A public client: park it in `pending`, ask the agent for a data
/// connection, and splice the two together once it arrives.
async fn on_public_port(mut public: TcpStream, peer: SocketAddr, relay: Arc<Relay>) {
    let (tx, rx) = oneshot::channel();
    let (id, asked) = {
        let mut state = relay.state();
        let id = state.next_id;
        state.next_id += 1;
        state.pending.insert(id, tx);
        // `try_send`, not `send().await`: we're holding a std Mutex.
        let asked = state
            .agent
            .as_ref()
            .is_some_and(|agent| agent.try_send(id).is_ok());
        (id, asked)
    };
    let note = if asked {
        "asking the agent"
    } else {
        "no agent yet, waiting"
    };
    eprintln!("[{peer}] public connection #{id}: {note}");

    match timeout(relay.config.pending_timeout, rx).await {
        Ok(Ok(mut data)) => match copy_bidirectional(&mut public, &mut data).await {
            Ok((up, down)) => eprintln!("[{peer}] #{id} done: {up} bytes in, {down} bytes out"),
            Err(e) => eprintln!("[{peer}] #{id} ended: {e}"),
        },
        // Timed out. (A dropped sender, `Ok(Err(_))`, can't happen: the map
        // holds the sender until a DATA claims it or we remove it here.)
        _ => {
            relay.state().pending.remove(&id);
            eprintln!("[{peer}] #{id} timed out waiting for the agent, closing");
        }
    }
}
