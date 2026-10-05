//! findr — a `find` clone: walk directories and filter entries.
//!
//!   findr . --name '\.rs$'
//!   findr src --type f --min-size 1024
//!   findr . --type d
//!   findr . --newer-than 7      # modified within the last 7 days

use std::process::ExitCode;
use std::time::{Duration, SystemTime};

use anyhow::Result;
use clap::{Parser, ValueEnum};
use regex::Regex;
use walkdir::{DirEntry, WalkDir};

/// Find files and directories
#[derive(Debug, Parser)]
#[command(version, about)]
struct Args {
    /// Where to start walking
    #[arg(default_value = ".")]
    paths: Vec<String>,

    /// Filter names by regex (matched against the file name, not the path)
    #[arg(short, long)]
    name: Option<Regex>,

    /// Entry type: f (file), d (dir), l (symlink)
    #[arg(short = 't', long = "type", value_enum)]
    entry_type: Option<EntryType>,

    /// Minimum size in bytes (files only)
    #[arg(long)]
    min_size: Option<u64>,

    /// Maximum size in bytes (files only)
    #[arg(long)]
    max_size: Option<u64>,

    /// Modified within the last N days
    #[arg(long)]
    newer_than: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, ValueEnum)]
enum EntryType {
    #[value(name = "f")]
    File,
    #[value(name = "d")]
    Dir,
    #[value(name = "l")]
    Link,
}

/// A filter is just a boxed predicate. The walk applies all of them;
/// adding a new flag means pushing one more closure.
type Filter = Box<dyn Fn(&DirEntry) -> bool>;

fn build_filters(args: &Args, now: SystemTime) -> Vec<Filter> {
    let mut filters: Vec<Filter> = Vec::new();

    if let Some(regex) = args.name.clone() {
        filters.push(Box::new(move |entry| {
            entry.file_name().to_str().is_some_and(|name| regex.is_match(name))
        }));
    }

    if let Some(entry_type) = args.entry_type {
        filters.push(Box::new(move |entry| {
            let ft = entry.file_type();
            match entry_type {
                EntryType::File => ft.is_file(),
                EntryType::Dir => ft.is_dir(),
                EntryType::Link => ft.is_symlink(),
            }
        }));
    }

    if args.min_size.is_some() || args.max_size.is_some() {
        let (min, max) = (args.min_size.unwrap_or(0), args.max_size.unwrap_or(u64::MAX));
        filters.push(Box::new(move |entry| {
            entry.file_type().is_file()
                && entry
                    .metadata()
                    .map(|m| (min..=max).contains(&m.len()))
                    .unwrap_or(false)
        }));
    }

    if let Some(days) = args.newer_than {
        let cutoff = now - Duration::from_secs(days * 24 * 60 * 60);
        filters.push(Box::new(move |entry| {
            entry
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .is_some_and(|mtime| mtime >= cutoff)
        }));
    }

    filters
}

fn matches_all(entry: &DirEntry, filters: &[Filter]) -> bool {
    filters.iter().all(|f| f(entry))
}

fn run(args: &Args) -> Result<bool> {
    let filters = build_filters(args, SystemTime::now());
    let mut failed = false;

    for start in &args.paths {
        for entry in WalkDir::new(start) {
            match entry {
                Ok(entry) => {
                    if matches_all(&entry, &filters) {
                        println!("{}", entry.path().display());
                    }
                }
                Err(e) => {
                    // Unreadable directories shouldn't kill the whole walk.
                    eprintln!("findr: {e}");
                    failed = true;
                }
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
            eprintln!("findr: {e:#}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn args_default() -> Args {
        Args {
            paths: vec![".".into()],
            name: None,
            entry_type: None,
            min_size: None,
            max_size: None,
            newer_than: None,
        }
    }

    /// Builds a little tree and returns matching paths relative to it.
    fn find_in_tree(mutate: impl FnOnce(&mut Args)) -> Vec<String> {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir(root.join("sub")).unwrap();
        fs::write(root.join("small.txt"), "hi").unwrap();
        fs::write(root.join("big.log"), vec![b'x'; 5000]).unwrap();
        fs::write(root.join("sub/nested.txt"), "nested").unwrap();

        let mut args = args_default();
        mutate(&mut args);
        let filters = build_filters(&args, SystemTime::now());

        let mut found: Vec<String> = WalkDir::new(root)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| matches_all(e, &filters))
            .map(|e| {
                e.path()
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .filter(|p| !p.is_empty())
            .collect();
        found.sort();
        found
    }

    #[test]
    fn no_filters_matches_everything() {
        let found = find_in_tree(|_| {});
        assert_eq!(found, vec!["big.log", "small.txt", "sub", "sub/nested.txt"]);
    }

    #[test]
    fn name_regex_filters() {
        let found = find_in_tree(|a| a.name = Some(Regex::new(r"\.txt$").unwrap()));
        assert_eq!(found, vec!["small.txt", "sub/nested.txt"]);
    }

    #[test]
    fn type_filter() {
        let found = find_in_tree(|a| a.entry_type = Some(EntryType::Dir));
        assert_eq!(found, vec!["sub"]);
    }

    #[test]
    fn size_filters() {
        let found = find_in_tree(|a| a.min_size = Some(1000));
        assert_eq!(found, vec!["big.log"]);
        let found = find_in_tree(|a| a.max_size = Some(10));
        assert_eq!(found, vec!["small.txt", "sub/nested.txt"]);
    }

    #[test]
    fn newer_than_matches_fresh_files() {
        // Everything was just created, so 1 day covers it all...
        let found = find_in_tree(|a| a.newer_than = Some(1));
        assert_eq!(found.len(), 4);
    }

    #[test]
    fn filters_combine_with_and() {
        let found = find_in_tree(|a| {
            a.name = Some(Regex::new(r"\.txt$").unwrap());
            a.min_size = Some(3);
        });
        assert_eq!(found, vec!["sub/nested.txt"]);
    }
}
