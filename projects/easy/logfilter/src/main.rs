//! logfilter — analyze an nginx/Apache "combined" access log.
//!
//!   logfilter access.log
//!   logfilter access.log --status 404
//!   logfilter access.log --since 2026-09-20 --until 2026-09-21
//!   logfilter access.log --top 3
//!
//! Malformed lines are skipped and counted, never fatal.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::sync::OnceLock;

use anyhow::{Context, Result};
use chrono::{DateTime, FixedOffset, NaiveDate, Timelike};
use clap::Parser;
use regex::Regex;

/// Summarize an access log
#[derive(Debug, Parser)]
#[command(version, about)]
struct Args {
    /// Log file in combined format
    file: String,

    /// Keep only this status code
    #[arg(long)]
    status: Option<u16>,

    /// Keep entries on/after this date (YYYY-MM-DD)
    #[arg(long)]
    since: Option<NaiveDate>,

    /// Keep entries before this date (YYYY-MM-DD)
    #[arg(long)]
    until: Option<NaiveDate>,

    /// How many top URLs to show
    #[arg(long, default_value_t = 5)]
    top: usize,
}

#[derive(Debug, Clone, PartialEq)]
struct Entry {
    ip: String,
    time: DateTime<FixedOffset>,
    method: String,
    path: String,
    status: u16,
    bytes: u64,
}

fn line_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // ip - user [time] "METHOD path PROTO" status bytes ...
        Regex::new(r#"^(\S+) \S+ \S+ \[([^\]]+)\] "(\S+) (\S+)[^"]*" (\d{3}) (\d+|-)"#)
            .expect("valid regex")
    })
}

/// One line -> Option<Entry>. Garbage in, None out — perfect for filter_map.
fn parse_line(line: &str) -> Option<Entry> {
    let captures = line_regex().captures(line)?;
    let time = DateTime::parse_from_str(&captures[2], "%d/%b/%Y:%H:%M:%S %z").ok()?;
    Some(Entry {
        ip: captures[1].to_string(),
        time,
        method: captures[3].to_string(),
        path: captures[4].to_string(),
        status: captures[5].parse().ok()?,
        bytes: captures[6].parse().unwrap_or(0), // "-" means no body
    })
}

fn keep(entry: &Entry, args: &Args) -> bool {
    let date = entry.time.date_naive();
    args.status.is_none_or(|s| entry.status == s)
        && args.since.is_none_or(|d| date >= d)
        && args.until.is_none_or(|d| date < d)
}

/// Counts occurrences of anything hashable — one helper, three reports.
fn tally<K: std::hash::Hash + Eq>(items: impl Iterator<Item = K>) -> HashMap<K, u64> {
    let mut counts = HashMap::new();
    for item in items {
        *counts.entry(item).or_insert(0) += 1;
    }
    counts
}

fn sorted_desc<K: Clone + Ord>(counts: &HashMap<K, u64>) -> Vec<(K, u64)> {
    let mut pairs: Vec<(K, u64)> = counts.iter().map(|(k, v)| (k.clone(), *v)).collect();
    pairs.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    pairs
}

fn report(entries: &[Entry], skipped: usize, top: usize) -> String {
    let mut out = Vec::new();
    let total_bytes: u64 = entries.iter().map(|e| e.bytes).sum();
    out.push(format!(
        "{} requests, {} skipped lines, {:.1} MB served",
        entries.len(),
        skipped,
        total_bytes as f64 / 1_048_576.0
    ));

    out.push("\nstatus:".to_string());
    for (status, count) in sorted_desc(&tally(entries.iter().map(|e| e.status))) {
        out.push(format!("  {status}  {count}"));
    }

    out.push(format!("\ntop {top} urls:"));
    let by_path = tally(entries.iter().map(|e| e.path.clone()));
    for (path, count) in sorted_desc(&by_path).into_iter().take(top) {
        out.push(format!("  {count:>5}  {path}"));
    }

    out.push("\nrequests per hour:".to_string());
    let by_hour = tally(entries.iter().map(|e| e.time.hour()));
    let max = by_hour.values().copied().max().unwrap_or(1);
    for hour in 0..24 {
        if let Some(&count) = by_hour.get(&hour) {
            let bar = "#".repeat((count * 40 / max).max(1) as usize);
            out.push(format!("  {hour:02}  {bar} {count}"));
        }
    }
    out.join("\n")
}

fn main() -> Result<()> {
    let args = Args::parse();
    let file = File::open(&args.file).with_context(|| format!("failed to open {}", args.file))?;

    let mut skipped = 0;
    // The pipeline: lines -> parsed entries -> filtered entries.
    let entries: Vec<Entry> = BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| {
            parse_line(&line).or_else(|| {
                skipped += 1;
                None
            })
        })
        .filter(|entry| keep(entry, &args))
        .collect();

    println!("{}", report(&entries, skipped, args.top));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINE: &str = r#"203.0.113.9 - frank [21/Sep/2026:10:15:32 +0000] "GET /index.html HTTP/1.1" 200 2326 "-" "Mozilla/5.0""#;

    fn args_default() -> Args {
        Args { file: String::new(), status: None, since: None, until: None, top: 5 }
    }

    #[test]
    fn parses_a_combined_log_line() {
        let entry = parse_line(LINE).unwrap();
        assert_eq!(entry.ip, "203.0.113.9");
        assert_eq!(entry.method, "GET");
        assert_eq!(entry.path, "/index.html");
        assert_eq!(entry.status, 200);
        assert_eq!(entry.bytes, 2326);
        assert_eq!(entry.time.hour(), 10);
        assert_eq!(entry.time.date_naive(), NaiveDate::from_ymd_opt(2026, 9, 21).unwrap());
    }

    #[test]
    fn dash_bytes_means_zero() {
        let line = r#"1.2.3.4 - - [21/Sep/2026:10:00:00 +0000] "HEAD / HTTP/1.1" 301 - "-" "curl""#;
        assert_eq!(parse_line(line).unwrap().bytes, 0);
    }

    #[test]
    fn garbage_lines_return_none() {
        assert!(parse_line("not a log line").is_none());
        assert!(parse_line("").is_none());
        let bad_date = r#"1.2.3.4 - - [99/Nope/2026:10:00:00] "GET / HTTP/1.1" 200 1"#;
        assert!(parse_line(bad_date).is_none());
    }

    #[test]
    fn status_filter() {
        let entry = parse_line(LINE).unwrap();
        let mut args = args_default();
        args.status = Some(200);
        assert!(keep(&entry, &args));
        args.status = Some(404);
        assert!(!keep(&entry, &args));
    }

    #[test]
    fn date_range_filter() {
        let entry = parse_line(LINE).unwrap(); // 2026-09-21
        let mut args = args_default();
        args.since = NaiveDate::from_ymd_opt(2026, 9, 21);
        args.until = NaiveDate::from_ymd_opt(2026, 9, 22);
        assert!(keep(&entry, &args));
        args.since = NaiveDate::from_ymd_opt(2026, 9, 22);
        assert!(!keep(&entry, &args));
    }

    #[test]
    fn tally_and_sort() {
        let counts = tally(["a", "b", "a", "a", "c", "b"].into_iter());
        assert_eq!(sorted_desc(&counts), vec![("a", 3), ("b", 2), ("c", 1)]);
    }

    #[test]
    fn report_mentions_the_top_url() {
        let entries: Vec<Entry> = [LINE, LINE, LINE].iter().filter_map(|l| parse_line(l)).collect();
        let text = report(&entries, 1, 5);
        assert!(text.contains("3 requests, 1 skipped"));
        assert!(text.contains("200  3"));
        assert!(text.contains("/index.html"));
    }
}
