//! uniqr — a `uniq` clone: collapse adjacent duplicate lines.

use std::fs::File;
use std::io::{self, BufRead, BufReader, BufWriter, Write};
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::Parser;

/// Filter adjacent repeated lines
#[derive(Debug, Parser)]
#[command(version, about)]
struct Args {
    /// Input file ("-" means stdin)
    #[arg(default_value = "-")]
    input: String,

    /// Output file (stdout if omitted)
    output: Option<String>,

    /// Prefix lines with occurrence counts
    #[arg(short = 'c', long = "count")]
    count: bool,

    /// Only print duplicated lines
    #[arg(short = 'd', long = "repeated", conflicts_with = "unique")]
    repeated: bool,

    /// Only print lines that are not repeated
    #[arg(short = 'u', long = "unique")]
    unique: bool,
}

/// Streaming: one line of lookbehind, never the whole file in memory.
fn uniq(reader: impl BufRead, args: &Args, out: &mut impl Write) -> Result<()> {
    let mut previous: Option<String> = None;
    let mut count: u64 = 0;

    let flush = |line: &str, count: u64, out: &mut dyn Write| -> Result<()> {
        let keep = if args.repeated {
            count > 1
        } else if args.unique {
            count == 1
        } else {
            true
        };
        if !keep {
            return Ok(());
        }
        if args.count {
            writeln!(out, "{count:>7} {line}")?;
        } else {
            writeln!(out, "{line}")?;
        }
        Ok(())
    };

    for line in reader.lines() {
        let line = line.context("failed to read line")?;
        match &previous {
            Some(prev) if *prev == line => count += 1,
            Some(prev) => {
                flush(prev, count, out)?;
                previous = Some(line);
                count = 1;
            }
            None => {
                previous = Some(line);
                count = 1;
            }
        }
    }
    if let Some(prev) = previous {
        flush(&prev, count, out)?;
    }
    Ok(())
}

fn run(args: &Args) -> Result<()> {
    let reader: Box<dyn BufRead> = match args.input.as_str() {
        "-" => Box::new(BufReader::new(io::stdin())),
        path => Box::new(BufReader::new(
            File::open(path).with_context(|| format!("failed to open {path}"))?,
        )),
    };
    // One abstract writer for both destinations: this is `dyn` paying rent.
    let mut writer: Box<dyn Write> = match &args.output {
        None => Box::new(io::stdout()),
        Some(path) => Box::new(BufWriter::new(
            File::create(path).with_context(|| format!("failed to create {path}"))?,
        )),
    };
    uniq(reader, args, &mut writer)?;
    writer.flush()?;
    Ok(())
}

fn main() -> ExitCode {
    let args = Args::parse();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("uniqr: {e:#}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use io::Cursor;

    fn run_uniq(input: &str, count: bool, repeated: bool, unique: bool) -> String {
        let args = Args {
            input: "-".into(),
            output: None,
            count,
            repeated,
            unique,
        };
        let mut out = Vec::new();
        uniq(Cursor::new(input), &args, &mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn collapses_adjacent_duplicates() {
        assert_eq!(run_uniq("a\na\nb\na\n", false, false, false), "a\nb\na\n");
    }

    #[test]
    fn counts_occurrences() {
        assert_eq!(
            run_uniq("a\na\na\nb\n", true, false, false),
            "      3 a\n      1 b\n"
        );
    }

    #[test]
    fn repeated_only() {
        assert_eq!(run_uniq("a\na\nb\nc\nc\n", false, true, false), "a\nc\n");
    }

    #[test]
    fn unique_only() {
        assert_eq!(run_uniq("a\na\nb\nc\nc\n", false, false, true), "b\n");
    }

    #[test]
    fn empty_input_produces_nothing() {
        assert_eq!(run_uniq("", false, false, false), "");
    }

    #[test]
    fn blank_lines_are_lines_too() {
        assert_eq!(run_uniq("\n\n\na\n", true, false, false), "      3 \n      1 a\n");
    }
}
