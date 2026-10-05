//! wordcount — a `wc` clone: lines, words, chars, bytes.
//!
//!   wordcount file1.txt file2.txt     # per-file rows + total
//!   echo hello | wordcount            # stdin
//!   wordcount -l file.txt             # only lines (also -w, -c, -b)

use std::env;
use std::fs::File;
use std::io::{self, BufRead, BufReader};
use std::ops::Add;
use std::process::ExitCode;

#[derive(Debug, Default, Clone, Copy, PartialEq)]
struct Counts {
    lines: u64,
    words: u64,
    chars: u64,
    bytes: u64,
}

impl Add for Counts {
    type Output = Counts;

    fn add(self, rhs: Counts) -> Counts {
        Counts {
            lines: self.lines + rhs.lines,
            words: self.words + rhs.words,
            chars: self.chars + rhs.chars,
            bytes: self.bytes + rhs.bytes,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Flags {
    lines: bool,
    words: bool,
    chars: bool,
    bytes: bool,
}

impl Default for Flags {
    fn default() -> Self {
        // Like `wc`: no flags means lines + words + bytes.
        Flags { lines: true, words: true, chars: false, bytes: true }
    }
}

impl Counts {
    fn format(&self, flags: Flags, label: &str) -> String {
        let mut cols = Vec::new();
        if flags.lines {
            cols.push(format!("{:>8}", self.lines));
        }
        if flags.words {
            cols.push(format!("{:>8}", self.words));
        }
        if flags.chars {
            cols.push(format!("{:>8}", self.chars));
        }
        if flags.bytes {
            cols.push(format!("{:>8}", self.bytes));
        }
        format!("{} {}", cols.join(""), label)
    }
}

/// Counts everything in one pass over a buffered reader.
/// `read_line` keeps the `\n`, which is exactly what byte counting needs.
fn count(reader: impl BufRead) -> io::Result<Counts> {
    let mut counts = Counts::default();
    let mut line = String::new();
    let mut reader = reader;
    loop {
        line.clear();
        let n = reader.read_line(&mut line)?;
        if n == 0 {
            break;
        }
        counts.bytes += n as u64;
        counts.chars += line.chars().count() as u64;
        counts.words += line.split_whitespace().count() as u64;
        if line.ends_with('\n') {
            counts.lines += 1;
        }
    }
    Ok(counts)
}

fn open(path: &str) -> io::Result<Box<dyn BufRead>> {
    match path {
        "-" => Ok(Box::new(BufReader::new(io::stdin()))),
        _ => Ok(Box::new(BufReader::new(File::open(path)?))),
    }
}

fn parse_args(args: &[String]) -> (Flags, Vec<String>) {
    let mut flags = Flags { lines: false, words: false, chars: false, bytes: false };
    let mut any_flag = false;
    let mut files = Vec::new();

    for arg in args {
        match arg.as_str() {
            "-l" => {
                flags.lines = true;
                any_flag = true;
            }
            "-w" => {
                flags.words = true;
                any_flag = true;
            }
            "-c" => {
                flags.chars = true;
                any_flag = true;
            }
            "-b" => {
                flags.bytes = true;
                any_flag = true;
            }
            _ => files.push(arg.clone()),
        }
    }
    if !any_flag {
        flags = Flags::default();
    }
    if files.is_empty() {
        files.push("-".to_string());
    }
    (flags, files)
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let (flags, files) = parse_args(&args);

    let mut total = Counts::default();
    let mut failed = false;

    for path in &files {
        match open(path).and_then(count) {
            Ok(counts) => {
                println!("{}", counts.format(flags, path));
                total = total + counts;
            }
            Err(e) => {
                eprintln!("wordcount: {path}: {e}");
                failed = true;
            }
        }
    }

    if files.len() > 1 {
        println!("{}", total.format(flags, "total"));
    }
    if failed { ExitCode::FAILURE } else { ExitCode::SUCCESS }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn count_str(s: &str) -> Counts {
        count(Cursor::new(s)).unwrap()
    }

    #[test]
    fn counts_a_simple_file() {
        let c = count_str("hello world\nsecond line\n");
        assert_eq!(c, Counts { lines: 2, words: 4, chars: 24, bytes: 24 });
    }

    #[test]
    fn empty_input_is_all_zeroes() {
        assert_eq!(count_str(""), Counts::default());
    }

    #[test]
    fn missing_trailing_newline_still_counts_words() {
        let c = count_str("one two three");
        assert_eq!(c.lines, 0); // like `wc`: lines = newline count
        assert_eq!(c.words, 3);
        assert_eq!(c.bytes, 13);
    }

    #[test]
    fn multibyte_chars_differ_from_bytes() {
        let c = count_str("привет\n");
        assert_eq!(c.chars, 7);
        assert_eq!(c.bytes, 13); // 6 cyrillic chars × 2 bytes + '\n'
    }

    #[test]
    fn totals_add_up() {
        let a = count_str("one\n");
        let b = count_str("two words\n");
        let sum = a + b;
        assert_eq!(sum.lines, 2);
        assert_eq!(sum.words, 3);
    }

    #[test]
    fn flag_parsing() {
        let args: Vec<String> = ["-l", "foo.txt"].iter().map(|s| s.to_string()).collect();
        let (flags, files) = parse_args(&args);
        assert!(flags.lines && !flags.words && !flags.bytes);
        assert_eq!(files, vec!["foo.txt"]);

        let (flags, files) = parse_args(&[]);
        assert!(flags.lines && flags.words && flags.bytes && !flags.chars);
        assert_eq!(files, vec!["-"]);
    }
}
