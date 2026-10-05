//! `fetchr [-I] [-v] [-L] [-X METHOD] [-d DATA] [-H 'Name: value'] URL`

use std::io;
use std::process::ExitCode;

use fetchr::{Options, Url, fetch};

const USAGE: &str = "usage: fetchr [-I] [-v] [-L] [-X METHOD] [-d DATA] [-H 'Name: value'] URL";

fn main() -> ExitCode {
    let (url, opts) = match parse_args(std::env::args().skip(1)) {
        Ok(parsed) => parsed,
        Err(msg) => {
            eprintln!("fetchr: {msg}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    // Body bytes go to stdout untouched (it may be binary); -v goes to stderr.
    match fetch(&url, &opts, &mut io::stdout().lock(), &mut io::stderr()) {
        Ok(_status) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("fetchr: {e}");
            ExitCode::FAILURE
        }
    }
}

fn parse_args(mut args: impl Iterator<Item = String>) -> Result<(Url, Options), String> {
    let mut opts = Options::default();
    let mut url = None;
    while let Some(arg) = args.next() {
        // A closure that borrows `args` to grab the flag's value.
        let mut value = |flag: &str| args.next().ok_or(format!("{flag} needs a value"));
        match arg.as_str() {
            "-I" | "--head" => opts.head = true,
            "-v" | "--verbose" => opts.verbose = true,
            "-L" | "--location" => opts.follow = true,
            "-X" | "--request" => opts.method = Some(value("-X")?),
            "-d" | "--data" => opts.data = Some(value("-d")?),
            "-H" | "--header" => {
                let header = value("-H")?;
                let (name, v) = header
                    .split_once(':')
                    .filter(|_| !header.bytes().any(|b| b.is_ascii_control()))
                    .ok_or(format!("-H wants 'Name: value', got {header:?}"))?;
                opts.headers
                    .push((name.trim().to_string(), v.trim().to_string()));
            }
            flag if flag.starts_with('-') => return Err(format!("unknown option {flag}")),
            _ if url.is_some() => return Err("only one URL, please".into()),
            _ => url = Some(arg.parse::<Url>()?),
        }
    }
    Ok((url.ok_or("missing URL")?, opts))
}
