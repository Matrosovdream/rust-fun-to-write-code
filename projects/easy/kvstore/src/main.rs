//! kvstore — an in-memory key-value store REPL with disk persistence.
//!
//!   kvstore [data.json]
//!   > SET name rust
//!   > GET name
//!   > DEL name
//!   > KEYS
//!   > SAVE
//!   > QUIT          (saves too)
//!
//! Library-style errors via `thiserror` (typed, matchable) — contrast with
//! `anyhow` used in app-style projects like catr.

use std::collections::HashMap;
use std::fs;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::str::FromStr;

use thiserror::Error;

#[derive(Debug, Error)]
enum StoreError {
    #[error("key '{0}' not found")]
    KeyNotFound(String),
    #[error("storage file is corrupted: {0}")]
    Corrupted(#[from] serde_json::Error),
    #[error("io error: {0}")]
    Io(#[from] io::Error),
}

struct Store {
    path: PathBuf,
    data: HashMap<String, String>,
    dirty: bool,
}

impl Store {
    fn open(path: &Path) -> Result<Store, StoreError> {
        let data = match fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text)?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => HashMap::new(),
            Err(e) => return Err(e.into()),
        };
        Ok(Store { path: path.to_path_buf(), data, dirty: false })
    }

    fn set(&mut self, key: String, value: String) -> Option<String> {
        self.dirty = true;
        self.data.insert(key, value)
    }

    fn get(&self, key: &str) -> Result<&str, StoreError> {
        self.data
            .get(key)
            .map(String::as_str)
            .ok_or_else(|| StoreError::KeyNotFound(key.to_string()))
    }

    fn del(&mut self, key: &str) -> Result<String, StoreError> {
        self.dirty = true;
        self.data
            .remove(key)
            .ok_or_else(|| StoreError::KeyNotFound(key.to_string()))
    }

    fn keys(&self) -> Vec<&str> {
        let mut keys: Vec<&str> = self.data.keys().map(String::as_str).collect();
        keys.sort_unstable();
        keys
    }

    fn save(&mut self) -> Result<(), StoreError> {
        let json = serde_json::to_string_pretty(&self.data)?;
        let tmp = self.path.with_extension("tmp");
        fs::write(&tmp, json)?;
        fs::rename(&tmp, &self.path)?;
        self.dirty = false;
        Ok(())
    }
}

#[derive(Debug, PartialEq)]
enum Command {
    Set(String, String),
    Get(String),
    Del(String),
    Keys,
    Save,
    Quit,
}

#[derive(Debug, Error, PartialEq)]
enum ParseError {
    #[error("empty command")]
    Empty,
    #[error("unknown command '{0}' (try SET/GET/DEL/KEYS/SAVE/QUIT)")]
    Unknown(String),
    #[error("{0} expects {1}")]
    WrongArgs(&'static str, &'static str),
}

impl FromStr for Command {
    type Err = ParseError;

    /// SET takes the rest of the line as the value — values may have spaces.
    fn from_str(line: &str) -> Result<Command, ParseError> {
        let line = line.trim();
        if line.is_empty() {
            return Err(ParseError::Empty);
        }
        let (cmd, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
        let rest = rest.trim();
        let one_word = !rest.is_empty() && !rest.contains(char::is_whitespace);

        match cmd.to_ascii_uppercase().as_str() {
            "SET" => match rest.split_once(char::is_whitespace) {
                Some((key, value)) => {
                    Ok(Command::Set(key.to_string(), value.trim_start().to_string()))
                }
                None => Err(ParseError::WrongArgs("SET", "a key and a value")),
            },
            "GET" if one_word => Ok(Command::Get(rest.to_string())),
            "GET" => Err(ParseError::WrongArgs("GET", "exactly one key")),
            "DEL" if one_word => Ok(Command::Del(rest.to_string())),
            "DEL" => Err(ParseError::WrongArgs("DEL", "exactly one key")),
            "KEYS" if rest.is_empty() => Ok(Command::Keys),
            "KEYS" => Err(ParseError::WrongArgs("KEYS", "no arguments")),
            "SAVE" if rest.is_empty() => Ok(Command::Save),
            "QUIT" | "Q" | "EXIT" if rest.is_empty() => Ok(Command::Quit),
            other => Err(ParseError::Unknown(other.to_string())),
        }
    }
}

/// Executes one command; returns the reply, or None when it's time to quit.
fn execute(store: &mut Store, command: Command) -> Option<String> {
    let reply = match command {
        Command::Set(key, value) => match store.set(key, value) {
            Some(old) => format!("ok (was: {old})"),
            None => "ok".to_string(),
        },
        Command::Get(key) => match store.get(&key) {
            Ok(value) => value.to_string(),
            Err(e) => format!("error: {e}"),
        },
        Command::Del(key) => match store.del(&key) {
            Ok(old) => format!("deleted (was: {old})"),
            Err(e) => format!("error: {e}"),
        },
        Command::Keys => {
            let keys = store.keys();
            if keys.is_empty() { "(empty)".to_string() } else { keys.join("\n") }
        }
        Command::Save => match store.save() {
            Ok(()) => format!("saved to {}", store.path.display()),
            Err(e) => format!("error: {e}"),
        },
        Command::Quit => return None,
    };
    Some(reply)
}

fn main() -> ExitCode {
    let path = std::env::args().nth(1).unwrap_or_else(|| "kvstore.json".to_string());
    let mut store = match Store::open(Path::new(&path)) {
        Ok(store) => store,
        Err(e) => {
            eprintln!("kvstore: {e}");
            return ExitCode::FAILURE;
        }
    };
    println!("kvstore — {} keys loaded from {path}", store.data.len());

    let stdin = io::stdin();
    print!("> ");
    io::stdout().flush().unwrap();
    for line in stdin.lock().lines() {
        let line = line.expect("read stdin");
        match line.parse::<Command>() {
            Ok(command) => match execute(&mut store, command) {
                Some(reply) => println!("{reply}"),
                None => break,
            },
            Err(ParseError::Empty) => {}
            Err(e) => println!("error: {e}"),
        }
        print!("> ");
        io::stdout().flush().unwrap();
    }

    if store.dirty
        && let Err(e) = store.save()
    {
        eprintln!("kvstore: failed to save on exit: {e}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_parsing() {
        assert_eq!("SET a 1".parse(), Ok(Command::Set("a".into(), "1".into())));
        assert_eq!(
            "set greeting hello world".parse(),
            Ok(Command::Set("greeting".into(), "hello world".into()))
        );
        assert_eq!("GET a".parse(), Ok(Command::Get("a".into())));
        assert_eq!("keys".parse(), Ok(Command::Keys));
        assert_eq!("q".parse(), Ok(Command::Quit));
        assert_eq!("".parse::<Command>(), Err(ParseError::Empty));
        assert_eq!("SET a".parse::<Command>(), Err(ParseError::WrongArgs("SET", "a key and a value")));
        assert_eq!("GET a b".parse::<Command>(), Err(ParseError::WrongArgs("GET", "exactly one key")));
        assert_eq!("ZAP".parse::<Command>(), Err(ParseError::Unknown("ZAP".into())));
    }

    #[test]
    fn set_get_del_cycle() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("db.json")).unwrap();

        assert!(store.set("k".into(), "v1".into()).is_none());
        assert_eq!(store.set("k".into(), "v2".into()), Some("v1".into()));
        assert_eq!(store.get("k").unwrap(), "v2");
        assert_eq!(store.del("k").unwrap(), "v2");
        assert!(matches!(store.get("k"), Err(StoreError::KeyNotFound(_))));
        assert!(matches!(store.del("k"), Err(StoreError::KeyNotFound(_))));
    }

    #[test]
    fn keys_are_sorted() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("db.json")).unwrap();
        store.set("zebra".into(), "1".into());
        store.set("apple".into(), "2".into());
        assert_eq!(store.keys(), vec!["apple", "zebra"]);
    }

    #[test]
    fn save_and_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db.json");

        let mut store = Store::open(&path).unwrap();
        store.set("lang".into(), "rust".into());
        store.save().unwrap();
        assert!(!store.dirty);

        let reopened = Store::open(&path).unwrap();
        assert_eq!(reopened.get("lang").unwrap(), "rust");
    }

    #[test]
    fn corrupted_file_is_a_typed_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db.json");
        fs::write(&path, "oops").unwrap();
        assert!(matches!(Store::open(&path), Err(StoreError::Corrupted(_))));
    }

    #[test]
    fn execute_replies() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("db.json")).unwrap();
        assert_eq!(execute(&mut store, "SET a 1".parse().unwrap()), Some("ok".into()));
        assert_eq!(execute(&mut store, "GET a".parse().unwrap()), Some("1".into()));
        assert_eq!(execute(&mut store, Command::Quit), None);
    }
}
