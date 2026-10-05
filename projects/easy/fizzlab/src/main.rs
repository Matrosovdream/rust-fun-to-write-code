//! fizzlab — FizzBuzz with rules supplied on the command line.
//!
//!   fizzlab                    # classic: up to 20, 3=Fizz 5=Buzz
//!   fizzlab 30                 # classic rules up to 30
//!   fizzlab 30 3=Fizz 5=Buzz 7=Boom

use std::env;
use std::process::ExitCode;

type Rule = (u32, String);

fn default_rules() -> Vec<Rule> {
    vec![(3, "Fizz".to_string()), (5, "Buzz".to_string())]
}

/// Parses "7=Boom" into (7, "Boom").
fn parse_rule(s: &str) -> Result<Rule, String> {
    let (divisor, word) = s
        .split_once('=')
        .ok_or_else(|| format!("bad rule '{s}', expected <divisor>=<word>"))?;
    let divisor: u32 = divisor
        .parse()
        .map_err(|_| format!("bad divisor in '{s}'"))?;
    if divisor == 0 {
        return Err(format!("divisor can't be zero in '{s}'"));
    }
    if word.is_empty() {
        return Err(format!("empty word in '{s}'"));
    }
    Ok((divisor, word.to_string()))
}

/// One line of output: concatenation of all matching words, or the number.
fn line(n: u32, rules: &[Rule]) -> String {
    let words: String = rules
        .iter()
        .filter(|(divisor, _)| n.is_multiple_of(*divisor))
        .map(|(_, word)| word.as_str())
        .collect();
    if words.is_empty() { n.to_string() } else { words }
}

fn play(upto: u32, rules: &[Rule]) -> Vec<String> {
    (1..=upto).map(|n| line(n, rules)).collect()
}

fn run(args: &[String]) -> Result<Vec<String>, String> {
    let mut upto = 20;
    let mut rules = Vec::new();

    for arg in args {
        if arg.contains('=') {
            rules.push(parse_rule(arg)?);
        } else {
            upto = arg.parse().map_err(|_| format!("bad count '{arg}'"))?;
        }
    }

    if rules.is_empty() {
        rules = default_rules();
    }
    Ok(play(upto, &rules))
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    match run(&args) {
        Ok(lines) => {
            println!("{}", lines.join("\n"));
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("fizzlab: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classic_fizzbuzz() {
        let rules = default_rules();
        assert_eq!(line(1, &rules), "1");
        assert_eq!(line(3, &rules), "Fizz");
        assert_eq!(line(5, &rules), "Buzz");
        assert_eq!(line(15, &rules), "FizzBuzz");
    }

    #[test]
    fn custom_rules_apply_in_order() {
        let rules = vec![(5, "B".to_string()), (3, "A".to_string())];
        assert_eq!(line(15, &rules), "BA");
    }

    #[test]
    fn play_produces_full_sequence() {
        let got = play(5, &default_rules());
        assert_eq!(got, vec!["1", "2", "Fizz", "4", "Buzz"]);
    }

    #[test]
    fn rule_parsing() {
        assert_eq!(parse_rule("7=Boom"), Ok((7, "Boom".to_string())));
        assert!(parse_rule("7").is_err());
        assert!(parse_rule("x=Boom").is_err());
        assert!(parse_rule("0=Boom").is_err());
        assert!(parse_rule("7=").is_err());
    }

    #[test]
    fn run_mixes_count_and_rules() {
        let args: Vec<String> = ["10", "2=Even"].iter().map(|s| s.to_string()).collect();
        let got = run(&args).unwrap();
        assert_eq!(got.len(), 10);
        assert_eq!(got[1], "Even");
        assert_eq!(got[2], "3");
    }
}
