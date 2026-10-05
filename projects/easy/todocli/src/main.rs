//! todocli — a todo manager that persists to a JSON file.
//!
//!   todocli add "buy milk"
//!   todocli list            # pending tasks
//!   todocli list --all
//!   todocli done 2
//!   todocli rm 2
//!
//! Storage: $TODOCLI_FILE, or ~/.todocli.json.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
struct Task {
    id: u64,
    title: String,
    created_at: DateTime<Utc>,
    done: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    completed_at: Option<DateTime<Utc>>,
}

/// All storage concerns live here: load, mutate, save. The commands below
/// never touch the filesystem themselves.
struct Repo {
    path: PathBuf,
    tasks: Vec<Task>,
}

impl Repo {
    fn load(path: &Path) -> Result<Repo> {
        let tasks = match fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text)
                .with_context(|| format!("{} is corrupted", path.display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(e).with_context(|| format!("failed to read {}", path.display())),
        };
        Ok(Repo { path: path.to_path_buf(), tasks })
    }

    /// Atomic save: write a temp file, then rename over the target.
    /// A crash mid-write can't eat the existing data.
    fn save(&self) -> Result<()> {
        let json = serde_json::to_string_pretty(&self.tasks)?;
        let tmp = self.path.with_extension("json.tmp");
        fs::write(&tmp, json).with_context(|| format!("failed to write {}", tmp.display()))?;
        fs::rename(&tmp, &self.path)
            .with_context(|| format!("failed to replace {}", self.path.display()))?;
        Ok(())
    }

    fn add(&mut self, title: &str) -> &Task {
        let id = self.tasks.iter().map(|t| t.id).max().unwrap_or(0) + 1;
        self.tasks.push(Task {
            id,
            title: title.to_string(),
            created_at: Utc::now(),
            done: false,
            completed_at: None,
        });
        self.tasks.last().expect("just pushed")
    }

    fn get_mut(&mut self, id: u64) -> Result<&mut Task> {
        self.tasks
            .iter_mut()
            .find(|t| t.id == id)
            .with_context(|| format!("no task with id {id}"))
    }

    fn complete(&mut self, id: u64) -> Result<&Task> {
        let task = self.get_mut(id)?;
        if task.done {
            bail!("task {id} is already done");
        }
        task.done = true;
        task.completed_at = Some(Utc::now());
        Ok(task)
    }

    fn remove(&mut self, id: u64) -> Result<Task> {
        let pos = self
            .tasks
            .iter()
            .position(|t| t.id == id)
            .with_context(|| format!("no task with id {id}"))?;
        Ok(self.tasks.remove(pos))
    }

    /// Oldest first; optionally filtered by done-ness.
    fn list(&self, filter: Option<bool>) -> Vec<&Task> {
        let mut tasks: Vec<&Task> = self
            .tasks
            .iter()
            .filter(|t| filter.is_none_or(|want_done| t.done == want_done))
            .collect();
        tasks.sort_by_key(|t| t.created_at);
        tasks
    }
}

/// Manage todos in a JSON file
#[derive(Debug, Parser)]
#[command(version, about)]
struct Args {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Debug, Subcommand)]
enum Cmd {
    /// Add a new task
    Add { title: Vec<String> },
    /// List tasks (pending by default)
    List {
        /// Include completed tasks
        #[arg(long)]
        all: bool,
        /// Only completed tasks
        #[arg(long, conflicts_with = "all")]
        done: bool,
    },
    /// Mark a task as done
    Done { id: u64 },
    /// Remove a task
    Rm { id: u64 },
}

fn storage_path() -> PathBuf {
    if let Ok(path) = std::env::var("TODOCLI_FILE") {
        return PathBuf::from(path);
    }
    std::env::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".todocli.json")
}

fn render(task: &Task) -> String {
    let mark = if task.done { "x" } else { " " };
    format!("[{mark}] #{:<3} {}  ({})", task.id, task.title, task.created_at.format("%Y-%m-%d"))
}

fn main() -> Result<()> {
    let args = Args::parse();
    let mut repo = Repo::load(&storage_path())?;

    match args.command {
        Cmd::Add { title } => {
            let title = title.join(" ");
            if title.trim().is_empty() {
                bail!("task title can't be empty");
            }
            let task = repo.add(&title);
            println!("added {}", render(task));
            repo.save()?;
        }
        Cmd::List { all, done } => {
            let filter = if all { None } else { Some(done) };
            let tasks = repo.list(filter);
            if tasks.is_empty() {
                println!("nothing here");
            }
            for task in tasks {
                println!("{}", render(task));
            }
        }
        Cmd::Done { id } => {
            let task = repo.complete(id)?;
            println!("done: {}", render(task));
            repo.save()?;
        }
        Cmd::Rm { id } => {
            let task = repo.remove(id)?;
            println!("removed: {}", render(&task));
            repo.save()?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_repo() -> (tempfile::TempDir, Repo) {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repo::load(&dir.path().join("todo.json")).unwrap();
        (dir, repo)
    }

    #[test]
    fn missing_file_means_empty_repo() {
        let (_dir, repo) = temp_repo();
        assert!(repo.tasks.is_empty());
    }

    #[test]
    fn add_assigns_sequential_ids() {
        let (_dir, mut repo) = temp_repo();
        assert_eq!(repo.add("one").id, 1);
        assert_eq!(repo.add("two").id, 2);
        repo.remove(2).unwrap();
        // max+1 strategy: a removed trailing id gets reused — a deliberate
        // simplification (see rewrite exercise 3)
        assert_eq!(repo.add("three").id, 2);
    }

    #[test]
    fn save_and_reload_round_trips() {
        let (dir, mut repo) = temp_repo();
        repo.add("persisted");
        repo.complete(1).unwrap();
        repo.save().unwrap();

        let reloaded = Repo::load(&dir.path().join("todo.json")).unwrap();
        assert_eq!(reloaded.tasks, repo.tasks);
        assert!(reloaded.tasks[0].done);
        assert!(reloaded.tasks[0].completed_at.is_some());
    }

    #[test]
    fn complete_twice_fails() {
        let (_dir, mut repo) = temp_repo();
        repo.add("task");
        repo.complete(1).unwrap();
        assert!(repo.complete(1).is_err());
        assert!(repo.complete(99).is_err());
    }

    #[test]
    fn list_filters() {
        let (_dir, mut repo) = temp_repo();
        repo.add("a");
        repo.add("b");
        repo.complete(1).unwrap();

        let pending: Vec<&str> = repo.list(Some(false)).iter().map(|t| t.title.as_str()).collect();
        assert_eq!(pending, vec!["b"]);
        let done: Vec<&str> = repo.list(Some(true)).iter().map(|t| t.title.as_str()).collect();
        assert_eq!(done, vec!["a"]);
        assert_eq!(repo.list(None).len(), 2);
    }

    #[test]
    fn corrupted_file_is_an_error_not_a_wipe() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("todo.json");
        fs::write(&path, "{not json").unwrap();
        assert!(Repo::load(&path).is_err());
    }
}
