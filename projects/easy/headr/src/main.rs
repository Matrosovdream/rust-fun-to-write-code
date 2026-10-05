//! headr — a `head` clone: first N lines or first N bytes.

use std::fs::File;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::Parser;

/// Print the first part of files
#[derive(Debug, Parser)]
#[command(version, about)]
struct Args {
    /// Files to print ("-" means stdin)
    #[arg(default_value = "-")]
    files: Vec<String>,

    /// Number of lines to print
    #[arg(short = 'n', long = "lines", default_value_t = 10, value_parser = clap::value_parser!(u64).range(1..))]
    lines: u64,

    /// Number of bytes to print (instead of lines)
    #[arg(short = 'c', long = "bytes", conflicts_with = "lines", value_parser = clap::value_parser!(u64).range(1..))]
    bytes: Option<u64>,
}

fn open(path: &str) -> Result<Box<dyn BufRead>> {
    match path {
        "-" => Ok(Box::new(BufReader::new(io::stdin()))),
        _ => {
            let file = File::open(path).with_context(|| format!("failed to open {path}"))?;
            Ok(Box::new(BufReader::new(file)))
        }
    }
}

/// `read_line` instead of `lines()`: we must keep the original line endings,
/// and a final line without '\n' must come out without one.
fn head_lines(mut reader: impl BufRead, n: u64, out: &mut impl Write) -> Result<()> {
    let mut line = String::new();
    for _ in 0..n {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            break;
        }
        out.write_all(line.as_bytes())?;
    }
    Ok(())
}

/// `Read::take` caps the reader at n bytes. Splitting a multi-byte char is
/// possible — that's what from_utf8_lossy is for.
fn head_bytes(reader: impl BufRead, n: u64, out: &mut impl Write) -> Result<()> {
    let mut buf = Vec::new();
    reader.take(n).read_to_end(&mut buf)?;
    out.write_all(String::from_utf8_lossy(&buf).as_bytes())?;
    Ok(())
}

fn run(args: &Args) -> Result<bool> {
    let many = args.files.len() > 1;
    let mut failed = false;
    let stdout = io::stdout();
    let mut out = stdout.lock();

    for (i, path) in args.files.iter().enumerate() {
        match open(path) {
            Ok(reader) => {
                if many {
                    let gap = if i > 0 { "\n" } else { "" };
                    writeln!(out, "{gap}==> {path} <==")?;
                }
                match args.bytes {
                    Some(n) => head_bytes(reader, n, &mut out)?,
                    None => head_lines(reader, args.lines, &mut out)?,
                }
            }
            Err(e) => {
                eprintln!("headr: {e:#}");
                failed = true;
            }
        }
    }
    Ok(failed)
}

fn main() -> ExitCode {
    let args = Args::parse();
    match run(&args) {
        Ok(false) => ExitCode::SUCCESS,
        Ok(true) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("headr: {e:#}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use io::Cursor;

    fn lines_of(input: &str, n: u64) -> String {
        let mut out = Vec::new();
        head_lines(Cursor::new(input), n, &mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    fn bytes_of(input: &str, n: u64) -> String {
        let mut out = Vec::new();
        head_bytes(Cursor::new(input), n, &mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn takes_first_lines() {
        assert_eq!(lines_of("a\nb\nc\n", 2), "a\nb\n");
    }

    #[test]
    fn short_input_is_fine() {
        assert_eq!(lines_of("a\n", 10), "a\n");
    }

    #[test]
    fn keeps_missing_trailing_newline() {
        assert_eq!(lines_of("a\nb", 5), "a\nb");
    }

    #[test]
    fn takes_first_bytes() {
        assert_eq!(bytes_of("abcdef", 3), "abc");
    }

    #[test]
    fn split_utf8_char_becomes_replacement() {
        // 'п' is two bytes; cutting after one byte yields U+FFFD.
        assert_eq!(bytes_of("привет", 3), "п\u{FFFD}");
    }
}
