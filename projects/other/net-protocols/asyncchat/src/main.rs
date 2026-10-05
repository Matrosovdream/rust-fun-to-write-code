use std::{env, io, process};

use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> io::Result<()> {
    let args: Vec<String> = env::args().skip(1).collect();
    let addr = match args.as_slice() {
        [] => "127.0.0.1:7001",
        [addr] if !addr.starts_with('-') => addr.as_str(),
        _ => {
            eprintln!("usage: asyncchat [LISTEN_ADDR]   (default 127.0.0.1:7001)");
            process::exit(2);
        }
    };
    let listener = TcpListener::bind(addr).await?;
    eprintln!(
        "asyncchat listening on {} (Ctrl-C to stop)",
        listener.local_addr()?
    );

    // If the Ctrl-C handler can't be installed, `ctrl_c()` fails at once and
    // we shut down right away. That's a fine fallback.
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    asyncchat::serve(listener, ctrl_c).await;
    eprintln!("asyncchat stopped");
    Ok(())
}
