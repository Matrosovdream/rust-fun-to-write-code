//! catr — a `cat` clone with line numbering.

use std::fs::File;
use std::io::{self, BufRead, BufReader};
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::Parser;

/// Concatenate files to stdout
#[derive(Debug, Parser)]
#[command(version, about)]
struct Args {
    /// Files to print ("-" means stdin)
    #[arg(default_value = "-")]
    files: Vec<String>,

    /// Number all output lines
    #[arg(short = 'n', long = "number", conflicts_with = "number_nonblank")]
    number: bool,

    /// Number nonempty output lines only
    #[arg(short = 'b', long = "number-nonblank")]
    number_nonblank: bool,
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

/// Prints one source. The line counter is shared across files, like `cat -n`.
fn print_file(reader: impl BufRead, args: &Args, line_number: &mut usize, out: &mut impl io::Write) -> Result<()> {
    for line in reader.lines() {
        let line = line.context("failed to read line")?;
        if args.number || (args.number_nonblank && !line.is_empty()) {
            *line_number += 1;
            writeln!(out, "{line_number:>6}\t{line}")?;
        } else {
            writeln!(out, "{line}")?;
        }
    }
    Ok(())
}

fn run(args: &Args) -> Result<bool> {
    let mut line_number = 0;
    let mut failed = false;
    let stdout = io::stdout();
    let mut out = stdout.lock();

    for path in &args.files {
        match open(path) {
            Ok(reader) => print_file(reader, args, &mut line_number, &mut out)?,
            Err(e) => {
                // cat keeps going after a missing file; so do we.
                eprintln!("catr: {e:#}");
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
            eprintln!("catr: {e:#}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_on(input: &str, number: bool, nonblank: bool) -> String {
        let args = Args { files: vec![], number, number_nonblank: nonblank };
        let mut out = Vec::new();
        let mut n = 0;
        print_file(io::Cursor::new(input), &args, &mut n, &mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn plain_output() {
        assert_eq!(run_on("a\nb\n", false, false), "a\nb\n");
    }

    #[test]
    fn numbering_counts_all_lines() {
        assert_eq!(run_on("a\n\nb\n", true, false), "     1\ta\n     2\t\n     3\tb\n");
    }

    #[test]
    fn nonblank_skips_empty_lines() {
        assert_eq!(run_on("a\n\nb\n", false, true), "     1\ta\n\n     2\tb\n");
    }
}
