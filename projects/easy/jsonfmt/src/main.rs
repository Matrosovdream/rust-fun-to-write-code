//! jsonfmt — pretty-print, minify, and inspect JSON.
//!
//!   jsonfmt data.json                       # pretty-print
//!   cat data.json | jsonfmt --minify
//!   jsonfmt data.json --path user.pets[1].name
//!
//! Works on *untyped* JSON (`serde_json::Value`) — the shape is unknown
//! until runtime, the opposite of the typed structs in `todocli`.

use std::fs;
use std::io::{self, Read};

use anyhow::{Context, Result, anyhow, bail};
use clap::Parser;
use serde_json::Value;

/// Pretty-print, minify, and inspect JSON
#[derive(Debug, Parser)]
#[command(version, about)]
struct Args {
    /// JSON file ("-" means stdin)
    #[arg(default_value = "-")]
    file: String,

    /// Minify instead of pretty-printing
    #[arg(short, long)]
    minify: bool,

    /// Extract a value: e.g. user.addresses[0].city
    #[arg(short, long)]
    path: Option<String>,
}

#[derive(Debug, PartialEq)]
enum Step {
    Key(String),
    Index(usize),
}

/// "user.pets[1].name" -> [Key(user), Key(pets), Index(1), Key(name)]
fn parse_path(path: &str) -> Result<Vec<Step>> {
    let mut steps = Vec::new();
    for part in path.split('.') {
        if part.is_empty() {
            bail!("empty segment in path '{path}'");
        }
        // Each dot-part may carry [n] suffixes: pets[1][0]
        let (key, rest) = match part.find('[') {
            Some(bracket) => part.split_at(bracket),
            None => (part, ""),
        };
        if !key.is_empty() {
            steps.push(Step::Key(key.to_string()));
        }
        let mut rest = rest;
        while let Some(stripped) = rest.strip_prefix('[') {
            let (index, after) = stripped
                .split_once(']')
                .ok_or_else(|| anyhow!("unclosed '[' in '{part}'"))?;
            let index: usize = index.parse().with_context(|| format!("bad index '{index}'"))?;
            steps.push(Step::Index(index));
            rest = after;
        }
        if !rest.is_empty() {
            bail!("unexpected '{rest}' in '{part}'");
        }
    }
    Ok(steps)
}

/// Walks the Value tree, reporting exactly where a path stops matching.
fn extract<'a>(root: &'a Value, steps: &[Step]) -> Result<&'a Value> {
    let mut current = root;
    for (i, step) in steps.iter().enumerate() {
        let at = || {
            steps[..=i]
                .iter()
                .map(|s| match s {
                    Step::Key(k) => format!(".{k}"),
                    Step::Index(n) => format!("[{n}]"),
                })
                .collect::<String>()
        };
        current = match (step, current) {
            (Step::Key(key), Value::Object(map)) => {
                map.get(key).ok_or_else(|| anyhow!("no key '{key}' at {}", at()))?
            }
            (Step::Key(_), other) => {
                bail!("expected an object at {}, found {}", at(), kind(other))
            }
            (Step::Index(n), Value::Array(items)) => items
                .get(*n)
                .ok_or_else(|| anyhow!("index {n} out of bounds (len {}) at {}", items.len(), at()))?,
            (Step::Index(_), other) => {
                bail!("expected an array at {}, found {}", at(), kind(other))
            }
        };
    }
    Ok(current)
}

fn kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a bool",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

fn format_value(value: &Value, minify: bool) -> Result<String> {
    let out = if minify {
        serde_json::to_string(value)?
    } else {
        serde_json::to_string_pretty(value)?
    };
    Ok(out)
}

fn run(input: &str, args: &Args) -> Result<String> {
    // serde_json's parse errors carry line and column — surface them.
    let root: Value = serde_json::from_str(input)
        .map_err(|e| anyhow!("invalid JSON at line {}, column {}: {e}", e.line(), e.column()))?;

    let value = match &args.path {
        Some(path) => extract(&root, &parse_path(path)?)?,
        None => &root,
    };
    // A bare string extract prints unquoted, like `jq -r`.
    if let Value::String(s) = value
        && args.path.is_some()
    {
        return Ok(s.clone());
    }
    format_value(value, args.minify)
}

fn main() -> Result<()> {
    let args = Args::parse();
    let input = match args.file.as_str() {
        "-" => {
            let mut buf = String::new();
            io::stdin().read_to_string(&mut buf)?;
            buf
        }
        path => fs::read_to_string(path).with_context(|| format!("failed to read {path}"))?,
    };
    println!("{}", run(&input, &args)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const DATA: &str = r#"{
        "user": {
            "name": "stan",
            "pets": [{"name": "rex"}, {"name": "whiskers"}]
        },
        "active": true
    }"#;

    fn args(minify: bool, path: Option<&str>) -> Args {
        Args { file: "-".into(), minify, path: path.map(str::to_string) }
    }

    #[test]
    fn pretty_then_minify_round_trips() {
        let pretty = run(DATA, &args(false, None)).unwrap();
        let mini = run(&pretty, &args(true, None)).unwrap();
        assert!(!mini.contains('\n'));
        let v1: Value = serde_json::from_str(DATA).unwrap();
        let v2: Value = serde_json::from_str(&mini).unwrap();
        assert_eq!(v1, v2);
    }

    #[test]
    fn path_parsing() {
        assert_eq!(
            parse_path("user.pets[1].name").unwrap(),
            vec![
                Step::Key("user".into()),
                Step::Key("pets".into()),
                Step::Index(1),
                Step::Key("name".into())
            ]
        );
        assert_eq!(parse_path("a[0][1]").unwrap().len(), 3);
        assert!(parse_path("a..b").is_err());
        assert!(parse_path("a[x]").is_err());
        assert!(parse_path("a[1").is_err());
    }

    #[test]
    fn extracts_nested_values() {
        assert_eq!(run(DATA, &args(false, Some("user.pets[1].name"))).unwrap(), "whiskers");
        assert_eq!(run(DATA, &args(false, Some("active"))).unwrap(), "true");
    }

    #[test]
    fn extract_errors_name_the_exact_spot() {
        let err = run(DATA, &args(false, Some("user.pets[5].name"))).unwrap_err();
        assert!(err.to_string().contains("index 5 out of bounds"));
        let err = run(DATA, &args(false, Some("user.name.x"))).unwrap_err();
        assert!(err.to_string().contains("expected an object at .user.name.x"));
        let err = run(DATA, &args(false, Some("ghost"))).unwrap_err();
        assert!(err.to_string().contains("no key 'ghost'"));
    }

    #[test]
    fn invalid_json_reports_position() {
        let err = run("{\n  \"a\": oops\n}", &args(false, None)).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("line 2"), "{msg}");
    }
}
