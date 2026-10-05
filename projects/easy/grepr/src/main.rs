//! grepr — the Rust-book mini-grep, extended.
//!
//!   grepr pattern file1 [file2 ...]
//!   flags: -i case-insensitive, -n line numbers, -v invert match
//!   env:   GREPR_IGNORE_CASE=1 is the same as -i

use std::env;
use std::fs;
use std::process::ExitCode;

#[derive(Debug, Default)]
struct Config {
    pattern: String,
    files: Vec<String>,
    ignore_case: bool,
    line_numbers: bool,
    invert: bool,
}

impl Config {
    fn from_args(args: &[String]) -> Result<Config, String> {
        let mut config = Config {
            ignore_case: env::var("GREPR_IGNORE_CASE").is_ok_and(|v| v != "0"),
            ..Config::default()
        };
        let mut positional = Vec::new();

        for arg in args {
            match arg.as_str() {
                "-i" => config.ignore_case = true,
                "-n" => config.line_numbers = true,
                "-v" => config.invert = true,
                flag if flag.starts_with('-') && flag.len() > 1 => {
                    return Err(format!("unknown flag '{flag}'"));
                }
                _ => positional.push(arg.clone()),
            }
        }

        let (pattern, files) = positional
            .split_first()
            .ok_or("usage: grepr [-i] [-n] [-v] <pattern> <file>...")?;
        config.pattern = pattern.clone();
        config.files = files.to_vec();
        if config.files.is_empty() {
            return Err("no files given".to_string());
        }
        Ok(config)
    }
}

/// The point of this project: the returned slices *borrow* from `contents`.
/// No copying — the lifetime says the results live as long as the input.
fn search<'a>(pattern: &str, contents: &'a str) -> Vec<(usize, &'a str)> {
    contents
        .lines()
        .enumerate()
        .filter(|(_, line)| line.contains(pattern))
        .map(|(i, line)| (i + 1, line))
        .collect()
}

fn search_case_insensitive<'a>(pattern: &str, contents: &'a str) -> Vec<(usize, &'a str)> {
    let pattern = pattern.to_lowercase();
    contents
        .lines()
        .enumerate()
        .filter(|(_, line)| line.to_lowercase().contains(&pattern))
        .map(|(i, line)| (i + 1, line))
        .collect()
}

fn invert<'a>(matches: &[(usize, &'a str)], contents: &'a str) -> Vec<(usize, &'a str)> {
    let matched: Vec<usize> = matches.iter().map(|(n, _)| *n).collect();
    contents
        .lines()
        .enumerate()
        .map(|(i, line)| (i + 1, line))
        .filter(|(n, _)| !matched.contains(n))
        .collect()
}

fn run(config: &Config) -> Result<bool, String> {
    let many_files = config.files.len() > 1;
    let mut found_any = false;

    for path in &config.files {
        let contents =
            fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;

        let matches = if config.ignore_case {
            search_case_insensitive(&config.pattern, &contents)
        } else {
            search(&config.pattern, &contents)
        };
        let matches = if config.invert {
            invert(&matches, &contents)
        } else {
            matches
        };

        for (number, line) in &matches {
            found_any = true;
            let mut prefix = String::new();
            if many_files {
                prefix.push_str(&format!("{path}:"));
            }
            if config.line_numbers {
                prefix.push_str(&format!("{number}:"));
            }
            println!("{prefix}{line}");
        }
    }
    Ok(found_any)
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let config = match Config::from_args(&args) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("grepr: {e}");
            return ExitCode::FAILURE;
        }
    };
    match run(&config) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1), // like grep: no matches -> exit 1
        Err(e) => {
            eprintln!("grepr: {e}");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const POEM: &str = "\
Rust:
safe, fast, productive.
Pick three.
Trust me.";

    #[test]
    fn finds_one_line() {
        assert_eq!(search("duct", POEM), vec![(2, "safe, fast, productive.")]);
    }

    #[test]
    fn search_is_case_sensitive_by_default() {
        assert_eq!(search("rust", POEM), vec![(4, "Trust me.")]);
    }

    #[test]
    fn case_insensitive_finds_both() {
        assert_eq!(
            search_case_insensitive("rUsT", POEM),
            vec![(1, "Rust:"), (4, "Trust me.")]
        );
    }

    #[test]
    fn invert_returns_the_complement() {
        let matches = search("st", POEM); // lines 1, 2, 4
        let inverted = invert(&matches, POEM);
        assert_eq!(inverted, vec![(3, "Pick three.")]);
    }

    #[test]
    fn config_parses_flags_anywhere() {
        let args: Vec<String> = ["-n", "pat", "a.txt", "-i", "b.txt"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let c = Config::from_args(&args).unwrap();
        assert_eq!(c.pattern, "pat");
        assert_eq!(c.files, vec!["a.txt", "b.txt"]);
        assert!(c.line_numbers && c.ignore_case && !c.invert);
    }

    #[test]
    fn config_requires_pattern_and_file() {
        assert!(Config::from_args(&[]).is_err());
        let args = vec!["onlypattern".to_string()];
        assert!(Config::from_args(&args).is_err());
    }
}
