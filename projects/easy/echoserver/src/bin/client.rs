use std::io::{self, BufRead, BufReader, Write};
use std::net::TcpStream;

fn main() -> io::Result<()> {
    let addr = std::env::args().nth(1).unwrap_or_else(|| "127.0.0.1:7878".to_string());
    let stream = TcpStream::connect(&addr)?;
    println!("connected to {addr} — type lines, /time, /stats, /quit");

    let mut server_replies = BufReader::new(stream.try_clone()?);
    let mut to_server = stream;
    let stdin = io::stdin();

    for line in stdin.lock().lines() {
        writeln!(to_server, "{}", line?)?;
        let mut reply = String::new();
        if server_replies.read_line(&mut reply)? == 0 {
            println!("server closed the connection");
            break;
        }
        print!("< {reply}");
        if reply.trim() == "goodbye" {
            break;
        }
    }
    Ok(())
}
