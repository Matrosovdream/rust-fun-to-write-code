use std::net::{SocketAddr, TcpListener};
use std::sync::Arc;
use std::{env, io, process, thread};

use loadbal::{Config, Pool, Strategy, serve, serve_admin};

const USAGE: &str = "usage: loadbal [LISTEN_ADDR] --backends ADDR,ADDR,... \
                     [--strategy round-robin|least-conn] [--admin ADDR]
  defaults: LISTEN_ADDR 127.0.0.1:9400, --strategy round-robin, --admin 127.0.0.1:9490";

struct Args {
    listen: String,
    admin: String,
    backends: Vec<SocketAddr>,
    strategy: Strategy,
}

fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Args, String> {
    let mut parsed = Args {
        listen: "127.0.0.1:9400".into(),
        admin: "127.0.0.1:9490".into(),
        backends: Vec::new(),
        strategy: Strategy::RoundRobin,
    };
    let mut args = args.into_iter().peekable();
    // The first argument is the listen address, unless it's already a flag.
    if let Some(first) = args.next_if(|a| !a.starts_with("--")) {
        parsed.listen = first;
    }
    while let Some(flag) = args.next() {
        let value = args.next().ok_or_else(|| format!("{flag} needs a value"))?;
        match flag.as_str() {
            "--backends" => {
                parsed.backends = value
                    .split(',')
                    .map(|a| a.parse().map_err(|_| format!("bad backend address: {a}")))
                    .collect::<Result<_, _>>()?;
            }
            "--strategy" => parsed.strategy = value.parse()?,
            "--admin" => parsed.admin = value,
            other => return Err(format!("unknown flag: {other}")),
        }
    }
    if parsed.backends.is_empty() {
        return Err("--backends is required".into());
    }
    Ok(parsed)
}

fn main() -> io::Result<()> {
    let args = parse_args(env::args().skip(1)).unwrap_or_else(|msg| {
        eprintln!("loadbal: {msg}\n{USAGE}");
        process::exit(2);
    });

    let listener = TcpListener::bind(&args.listen)?;
    let admin = TcpListener::bind(&args.admin)?;
    let config = Config {
        strategy: args.strategy,
        ..Config::default()
    };
    let pool = Arc::new(Pool::new(args.backends, config));
    eprintln!(
        "loadbal listening on {}, admin on {}, {:?} over {} backends",
        listener.local_addr()?,
        admin.local_addr()?,
        args.strategy,
        pool.backends().len()
    );

    let health = Arc::clone(&pool);
    thread::spawn(move || health.run_health_checks());
    let stats = Arc::clone(&pool);
    thread::spawn(move || serve_admin(admin, stats));
    serve(listener, pool);
    Ok(())
}
