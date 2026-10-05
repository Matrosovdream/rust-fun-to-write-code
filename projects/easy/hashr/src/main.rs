//! hashr — hash every file in a directory with a pool of worker threads.
//!
//!   hashr hash <dir> [-j 8]     # path  sha256 for every file
//!   hashr dup <dir>             # groups of identical files
//!   hashr diff <dir_a> <dir_b>  # compare two trees by content
//!
//! The shape to remember:
//!   paths channel -> N workers -> results channel -> collector
//! The receiver is shared behind Arc<Mutex<_>> so workers can pull jobs.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use sha2::{Digest, Sha256};
use walkdir::WalkDir;

/// Parallel file hasher
#[derive(Debug, Parser)]
#[command(version, about)]
struct Args {
    #[command(subcommand)]
    command: Cmd,

    /// Worker threads
    #[arg(short = 'j', long, default_value_t = 4, global = true)]
    jobs: usize,
}

#[derive(Debug, Subcommand)]
enum Cmd {
    /// Print sha256 of every file under a directory
    Hash { dir: PathBuf },
    /// Find files with identical content
    Dup { dir: PathBuf },
    /// Compare two directory trees by content
    Diff { a: PathBuf, b: PathBuf },
}

/// Streaming hash: fixed 64 KiB buffer, any file size.
fn hash_file(path: &Path) -> Result<String> {
    let file = File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    let mut reader = BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn files_under(dir: &Path) -> Vec<PathBuf> {
    WalkDir::new(dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .map(|e| e.into_path())
        .collect()
}

/// Hashes all files using `jobs` worker threads.
/// Returns path -> hash; unreadable files are reported and skipped.
fn hash_all(paths: Vec<PathBuf>, jobs: usize) -> HashMap<PathBuf, String> {
    let (path_tx, path_rx) = mpsc::channel::<PathBuf>();
    let (result_tx, result_rx) = mpsc::channel::<(PathBuf, Result<String>)>();
    // mpsc receivers aren't shareable — the Mutex turns one receiver into
    // a job queue that N workers pull from.
    let path_rx = Arc::new(Mutex::new(path_rx));

    let mut workers = Vec::new();
    for _ in 0..jobs.max(1) {
        let rx = Arc::clone(&path_rx);
        let tx = result_tx.clone();
        workers.push(thread::spawn(move || {
            loop {
                // Hold the lock only to take a job, never while hashing.
                let job = rx.lock().expect("queue lock").recv();
                match job {
                    Ok(path) => {
                        let hash = hash_file(&path);
                        if tx.send((path, hash)).is_err() {
                            break; // collector is gone
                        }
                    }
                    Err(_) => break, // channel closed: no more jobs
                }
            }
        }));
    }
    // Drop our copies so the channels close when everyone is done.
    drop(result_tx);

    let total = paths.len();
    for path in paths {
        path_tx.send(path).expect("workers alive");
    }
    drop(path_tx);

    let mut hashes = HashMap::with_capacity(total);
    for (path, result) in result_rx {
        match result {
            Ok(hash) => {
                hashes.insert(path, hash);
            }
            Err(e) => eprintln!("hashr: {e:#}"),
        }
    }
    for worker in workers {
        worker.join().expect("worker panicked");
    }
    hashes
}

/// hash -> all paths sharing it, keeping only groups of 2+.
fn duplicates(hashes: &HashMap<PathBuf, String>) -> Vec<(String, Vec<PathBuf>)> {
    let mut by_hash: HashMap<&str, Vec<&PathBuf>> = HashMap::new();
    for (path, hash) in hashes {
        by_hash.entry(hash).or_default().push(path);
    }
    let mut groups: Vec<(String, Vec<PathBuf>)> = by_hash
        .into_iter()
        .filter(|(_, paths)| paths.len() > 1)
        .map(|(hash, mut paths)| {
            paths.sort();
            (hash.to_string(), paths.into_iter().cloned().collect())
        })
        .collect();
    groups.sort();
    groups
}

#[derive(Debug, Default, PartialEq)]
struct Diff {
    only_a: Vec<PathBuf>,
    only_b: Vec<PathBuf>,
    changed: Vec<PathBuf>,
    same: usize,
}

/// Compares by path relative to each root, then by content hash.
fn diff_trees(a_root: &Path, a: &HashMap<PathBuf, String>, b_root: &Path, b: &HashMap<PathBuf, String>) -> Diff {
    let relative = |root: &Path, map: &HashMap<PathBuf, String>| -> HashMap<PathBuf, String> {
        map.iter()
            .map(|(p, h)| (p.strip_prefix(root).unwrap_or(p).to_path_buf(), h.clone()))
            .collect()
    };
    let a = relative(a_root, a);
    let b = relative(b_root, b);

    let mut diff = Diff::default();
    for (path, hash) in &a {
        match b.get(path) {
            None => diff.only_a.push(path.clone()),
            Some(other) if other != hash => diff.changed.push(path.clone()),
            Some(_) => diff.same += 1,
        }
    }
    for path in b.keys() {
        if !a.contains_key(path) {
            diff.only_b.push(path.clone());
        }
    }
    diff.only_a.sort();
    diff.only_b.sort();
    diff.changed.sort();
    diff
}

fn main() -> Result<()> {
    let args = Args::parse();
    match args.command {
        Cmd::Hash { dir } => {
            let hashes = hash_all(files_under(&dir), args.jobs);
            let mut paths: Vec<&PathBuf> = hashes.keys().collect();
            paths.sort();
            for path in paths {
                println!("{}  {}", hashes[path], path.display());
            }
        }
        Cmd::Dup { dir } => {
            let hashes = hash_all(files_under(&dir), args.jobs);
            let groups = duplicates(&hashes);
            if groups.is_empty() {
                println!("no duplicates");
            }
            for (hash, paths) in groups {
                println!("{}:", &hash[..12]);
                for path in paths {
                    println!("  {}", path.display());
                }
            }
        }
        Cmd::Diff { a, b } => {
            let hashes_a = hash_all(files_under(&a), args.jobs);
            let hashes_b = hash_all(files_under(&b), args.jobs);
            let diff = diff_trees(&a, &hashes_a, &b, &hashes_b);
            for path in &diff.only_a {
                println!("only in {}: {}", a.display(), path.display());
            }
            for path in &diff.only_b {
                println!("only in {}: {}", b.display(), path.display());
            }
            for path in &diff.changed {
                println!("changed: {}", path.display());
            }
            println!("{} identical file(s)", diff.same);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    const HELLO_SHA256: &str = "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9";

    #[test]
    fn known_hash_vector() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hello.txt");
        fs::write(&path, "hello world").unwrap();
        assert_eq!(hash_file(&path).unwrap(), HELLO_SHA256);
    }

    #[test]
    fn same_result_with_any_worker_count() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..20 {
            fs::write(dir.path().join(format!("f{i}.txt")), format!("content {i}")).unwrap();
        }
        let serial = hash_all(files_under(dir.path()), 1);
        let parallel = hash_all(files_under(dir.path()), 8);
        assert_eq!(serial.len(), 20);
        assert_eq!(serial, parallel);
    }

    #[test]
    fn finds_duplicate_groups() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.txt"), "same").unwrap();
        fs::write(dir.path().join("b.txt"), "same").unwrap();
        fs::write(dir.path().join("c.txt"), "different").unwrap();

        let hashes = hash_all(files_under(dir.path()), 2);
        let groups = duplicates(&hashes);
        assert_eq!(groups.len(), 1);
        let names: Vec<String> = groups[0]
            .1
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["a.txt", "b.txt"]);
    }

    #[test]
    fn diff_catches_all_three_cases() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a"), dir.path().join("b"));
        fs::create_dir_all(&a).unwrap();
        fs::create_dir_all(&b).unwrap();
        fs::write(a.join("same.txt"), "x").unwrap();
        fs::write(b.join("same.txt"), "x").unwrap();
        fs::write(a.join("changed.txt"), "old").unwrap();
        fs::write(b.join("changed.txt"), "new").unwrap();
        fs::write(a.join("gone.txt"), "a only").unwrap();
        fs::write(b.join("added.txt"), "b only").unwrap();

        let diff = diff_trees(&a, &hash_all(files_under(&a), 2), &b, &hash_all(files_under(&b), 2));
        assert_eq!(diff.only_a, vec![PathBuf::from("gone.txt")]);
        assert_eq!(diff.only_b, vec![PathBuf::from("added.txt")]);
        assert_eq!(diff.changed, vec![PathBuf::from("changed.txt")]);
        assert_eq!(diff.same, 1);
    }

    #[test]
    fn empty_directory_is_fine() {
        let dir = tempfile::tempdir().unwrap();
        assert!(hash_all(files_under(dir.path()), 4).is_empty());
    }
}
