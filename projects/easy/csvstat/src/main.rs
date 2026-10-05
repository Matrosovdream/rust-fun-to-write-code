//! csvstat — per-column statistics for a CSV file.
//!
//! Numeric columns get count/min/max/mean/median; text columns get a
//! distinct-value count and the most common value. Empty cells are treated
//! as missing, not as errors.

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Read};

use anyhow::{Context, Result};
use clap::Parser;

/// Per-column statistics for a CSV file
#[derive(Debug, Parser)]
#[command(version, about)]
struct Args {
    /// CSV file ("-" means stdin)
    #[arg(default_value = "-")]
    file: String,

    /// Treat the first row as data, not headers
    #[arg(long)]
    no_header: bool,
}

/// Generic min/max in one pass — works for any ordered Copy type.
fn min_max<T: PartialOrd + Copy>(values: &[T]) -> Option<(T, T)> {
    let first = *values.first()?;
    Some(values.iter().fold((first, first), |(lo, hi), &v| {
        (if v < lo { v } else { lo }, if v > hi { v } else { hi })
    }))
}

#[derive(Debug, PartialEq)]
enum ColumnStats {
    Numeric { count: usize, missing: usize, min: f64, max: f64, mean: f64, median: f64 },
    Text { count: usize, missing: usize, distinct: usize, top: String, top_count: usize },
    Empty,
}

/// Decides numeric vs text by trying to parse every non-empty cell.
fn analyze(cells: &[String]) -> ColumnStats {
    let present: Vec<&str> = cells.iter().map(String::as_str).filter(|s| !s.trim().is_empty()).collect();
    let missing = cells.len() - present.len();
    if present.is_empty() {
        return ColumnStats::Empty;
    }

    let numbers: Option<Vec<f64>> = present.iter().map(|s| s.trim().parse().ok()).collect();

    match numbers {
        Some(mut numbers) => {
            let (min, max) = min_max(&numbers).expect("non-empty");
            let mean = numbers.iter().sum::<f64>() / numbers.len() as f64;
            numbers.sort_by(f64::total_cmp);
            let mid = numbers.len() / 2;
            let median = if numbers.len() % 2 == 1 {
                numbers[mid]
            } else {
                (numbers[mid - 1] + numbers[mid]) / 2.0
            };
            ColumnStats::Numeric { count: numbers.len(), missing, min, max, mean, median }
        }
        None => {
            let mut freq: HashMap<&str, usize> = HashMap::new();
            for value in &present {
                *freq.entry(value).or_insert(0) += 1;
            }
            // Highest count wins; ties go to the lexicographically smaller
            // value so the output is deterministic.
            let mut top = "";
            let mut top_count = 0;
            for (&name, &count) in &freq {
                if count > top_count || (count == top_count && name < top) {
                    top = name;
                    top_count = count;
                }
            }
            ColumnStats::Text {
                count: present.len(),
                missing,
                distinct: freq.len(),
                top: top.to_string(),
                top_count,
            }
        }
    }
}

fn render(name: &str, stats: &ColumnStats) -> String {
    match stats {
        ColumnStats::Numeric { count, missing, min, max, mean, median } => format!(
            "{name}\n  numeric  count {count}  missing {missing}\n  min {min}  max {max}  mean {mean:.3}  median {median}"
        ),
        ColumnStats::Text { count, missing, distinct, top, top_count } => format!(
            "{name}\n  text     count {count}  missing {missing}\n  distinct {distinct}  top '{top}' ×{top_count}"
        ),
        ColumnStats::Empty => format!("{name}\n  (all values missing)"),
    }
}

/// Reads the whole CSV into columns. `headers` is None with --no-header.
fn read_columns(reader: impl Read, has_header: bool) -> Result<(Vec<String>, Vec<Vec<String>>)> {
    let mut csv_reader = csv::ReaderBuilder::new()
        .has_headers(has_header)
        .flexible(true)
        .from_reader(reader);

    let headers: Vec<String> = if has_header {
        csv_reader.headers()?.iter().map(str::to_string).collect()
    } else {
        Vec::new()
    };

    let mut columns: Vec<Vec<String>> = Vec::new();
    for (row_number, record) in csv_reader.records().enumerate() {
        let record = record.with_context(|| format!("bad CSV around row {}", row_number + 1))?;
        if columns.len() < record.len() {
            columns.resize_with(record.len(), Vec::new);
        }
        for (i, column) in columns.iter_mut().enumerate() {
            column.push(record.get(i).unwrap_or("").to_string());
        }
    }

    let headers = if has_header {
        headers
    } else {
        (1..=columns.len()).map(|i| format!("column {i}")).collect()
    };
    Ok((headers, columns))
}

fn main() -> Result<()> {
    let args = Args::parse();
    let reader: Box<dyn Read> = match args.file.as_str() {
        "-" => Box::new(io::stdin()),
        path => Box::new(File::open(path).with_context(|| format!("failed to open {path}"))?),
    };

    let (headers, columns) = read_columns(reader, !args.no_header)?;
    if columns.is_empty() {
        println!("no data rows");
        return Ok(());
    }
    for (i, column) in columns.iter().enumerate() {
        let name = headers.get(i).cloned().unwrap_or_else(|| format!("column {}", i + 1));
        println!("{}", render(&name, &analyze(column)));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cells(values: &[&str]) -> Vec<String> {
        values.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn numeric_column_stats() {
        let stats = analyze(&cells(&["1", "2", "3", "4"]));
        assert_eq!(
            stats,
            ColumnStats::Numeric { count: 4, missing: 0, min: 1.0, max: 4.0, mean: 2.5, median: 2.5 }
        );
    }

    #[test]
    fn odd_count_median_is_middle_value() {
        let ColumnStats::Numeric { median, .. } = analyze(&cells(&["10", "30", "20"])) else {
            panic!("expected numeric");
        };
        assert_eq!(median, 20.0);
    }

    #[test]
    fn gaps_count_as_missing_not_text() {
        let stats = analyze(&cells(&["1", "", "3", "  "]));
        assert_eq!(
            stats,
            ColumnStats::Numeric { count: 2, missing: 2, min: 1.0, max: 3.0, mean: 2.0, median: 2.0 }
        );
    }

    #[test]
    fn mixed_column_falls_back_to_text() {
        let stats = analyze(&cells(&["1", "two", "3"]));
        let ColumnStats::Text { count, distinct, .. } = stats else {
            panic!("expected text");
        };
        assert_eq!((count, distinct), (3, 3));
    }

    #[test]
    fn text_top_value() {
        let ColumnStats::Text { top, top_count, distinct, .. } =
            analyze(&cells(&["cat", "dog", "cat", "cat", "fish"]))
        else {
            panic!("expected text");
        };
        assert_eq!((top.as_str(), top_count, distinct), ("cat", 3, 3));
    }

    #[test]
    fn all_empty_column() {
        assert_eq!(analyze(&cells(&["", ""])), ColumnStats::Empty);
    }

    #[test]
    fn generic_min_max() {
        assert_eq!(min_max(&[3, 1, 2]), Some((1, 3)));
        assert_eq!(min_max(&[1.5f64]), Some((1.5, 1.5)));
        assert_eq!(min_max::<i32>(&[]), None);
    }

    #[test]
    fn read_columns_with_headers() {
        let csv = "name,age\nalice,30\nbob,25\n";
        let (headers, columns) = read_columns(csv.as_bytes(), true).unwrap();
        assert_eq!(headers, vec!["name", "age"]);
        assert_eq!(columns[0], vec!["alice", "bob"]);
        assert_eq!(columns[1], vec!["30", "25"]);
    }

    #[test]
    fn read_columns_without_headers() {
        let csv = "1,2\n3,4\n";
        let (headers, columns) = read_columns(csv.as_bytes(), false).unwrap();
        assert_eq!(headers, vec!["column 1", "column 2"]);
        assert_eq!(columns[0], vec!["1", "3"]);
    }

    #[test]
    fn ragged_rows_pad_with_missing() {
        let csv = "a,b\n1,2\n3\n";
        let (_, columns) = read_columns(csv.as_bytes(), true).unwrap();
        assert_eq!(columns[1], vec!["2", ""]);
    }
}
