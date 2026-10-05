//! Integration tests over real localhost sockets.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;

use echoserver::{SharedStats, Stats, serve};

/// Binds port 0 (OS picks a free one), serves in a background thread.
fn start_server() -> (std::net::SocketAddr, SharedStats) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let stats: SharedStats = Arc::new(Mutex::new(Stats::default()));
    let server_stats = Arc::clone(&stats);
    thread::spawn(move || serve(listener, server_stats));
    (addr, stats)
}

fn send_line(reader: &mut impl BufRead, writer: &mut impl Write, line: &str) -> String {
    writeln!(writer, "{line}").unwrap();
    let mut reply = String::new();
    reader.read_line(&mut reply).unwrap();
    reply.trim_end().to_string()
}

#[test]
fn echoes_and_commands_over_a_real_socket() {
    let (addr, _stats) = start_server();
    let stream = TcpStream::connect(addr).unwrap();
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut writer = stream;

    assert_eq!(send_line(&mut reader, &mut writer, "hi"), "you said: hi");
    assert!(send_line(&mut reader, &mut writer, "/time").starts_with("time: "));
    assert_eq!(send_line(&mut reader, &mut writer, "/quit"), "goodbye");

    // After goodbye the server closes: the next read returns 0 bytes.
    let mut rest = String::new();
    assert_eq!(reader.read_line(&mut rest).unwrap(), 0);
}

#[test]
fn concurrent_connections_share_stats() {
    let (addr, stats) = start_server();

    let mut handles = Vec::new();
    for i in 0..3 {
        let handle = thread::spawn(move || {
            let stream = TcpStream::connect(addr).unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut writer = stream;
            let reply = send_line(&mut reader, &mut writer, &format!("hello from {i}"));
            assert_eq!(reply, format!("you said: hello from {i}"));
        });
        handles.push(handle);
    }
    for handle in handles {
        handle.join().unwrap();
    }

    // All three connections and lines were counted exactly once.
    let stats = stats.lock().unwrap();
    assert_eq!(stats.connections, 3);
    assert_eq!(stats.lines, 3);
}
