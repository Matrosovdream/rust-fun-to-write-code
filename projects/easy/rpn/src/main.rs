//! rpn — reverse polish notation calculator with variables.
//!
//!   rpn '3 4 + 2 *'       # 14
//!   rpn                   # REPL:  > x = 3 4 +
//!                         #        > x 2 *

use std::collections::HashMap;
use std::env;
use std::fmt;
use std::io::{self, BufRead, Write};
use std::process::ExitCode;

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Number(f64),
    Op(char),
    Var(String),
}

#[derive(Debug, PartialEq)]
enum RpnError {
    BadToken(String),
    StackUnderflow(char),
    UnknownVar(String),
    DivisionByZero,
    LeftoverValues(usize),
    EmptyExpression,
}

impl fmt::Display for RpnError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RpnError::BadToken(s) => write!(f, "bad token '{s}'"),
            RpnError::StackUnderflow(op) => write!(f, "operator '{op}' needs two values"),
            RpnError::UnknownVar(name) => write!(f, "unknown variable '{name}'"),
            RpnError::DivisionByZero => write!(f, "division by zero"),
            RpnError::LeftoverValues(n) => {
                write!(f, "{n} values left on the stack — missing an operator?")
            }
            RpnError::EmptyExpression => write!(f, "empty expression"),
        }
    }
}

fn tokenize(input: &str) -> Result<Vec<Token>, RpnError> {
    input
        .split_whitespace()
        .map(|word| match word {
            "+" | "-" | "*" | "/" | "^" => Ok(Token::Op(word.chars().next().unwrap())),
            _ if word.parse::<f64>().is_ok() => Ok(Token::Number(word.parse().unwrap())),
            _ if word.chars().all(|c| c.is_ascii_alphabetic() || c == '_') => {
                Ok(Token::Var(word.to_string()))
            }
            other => Err(RpnError::BadToken(other.to_string())),
        })
        .collect()
}

fn apply(op: char, a: f64, b: f64) -> Result<f64, RpnError> {
    match op {
        '+' => Ok(a + b),
        '-' => Ok(a - b),
        '*' => Ok(a * b),
        '/' => {
            if b == 0.0 {
                Err(RpnError::DivisionByZero)
            } else {
                Ok(a / b)
            }
        }
        '^' => Ok(a.powf(b)),
        _ => unreachable!("tokenizer only produces + - * / ^"),
    }
}

fn eval(input: &str, vars: &HashMap<String, f64>) -> Result<f64, RpnError> {
    let tokens = tokenize(input)?;
    if tokens.is_empty() {
        return Err(RpnError::EmptyExpression);
    }

    // The whole algorithm is a Vec used as a stack.
    let mut stack: Vec<f64> = Vec::new();
    for token in tokens {
        match token {
            Token::Number(n) => stack.push(n),
            Token::Var(name) => {
                let value = *vars.get(&name).ok_or(RpnError::UnknownVar(name))?;
                stack.push(value);
            }
            Token::Op(op) => {
                // pop() returns Option — underflow is a value, not a crash.
                let b = stack.pop().ok_or(RpnError::StackUnderflow(op))?;
                let a = stack.pop().ok_or(RpnError::StackUnderflow(op))?;
                stack.push(apply(op, a, b)?);
            }
        }
    }

    match stack.as_slice() {
        [result] => Ok(*result),
        rest => Err(RpnError::LeftoverValues(rest.len())),
    }
}

/// A line is either `name = expression` or a bare expression.
fn eval_line(line: &str, vars: &mut HashMap<String, f64>) -> Result<(String, f64), RpnError> {
    if let Some((name, expr)) = line.split_once('=') {
        let name = name.trim();
        if !name.is_empty() && name.chars().all(|c| c.is_ascii_alphabetic() || c == '_') {
            let value = eval(expr, vars)?;
            vars.insert(name.to_string(), value);
            return Ok((name.to_string(), value));
        }
        return Err(RpnError::BadToken(name.to_string()));
    }
    let value = eval(line, vars)?;
    Ok(("".to_string(), value))
}

fn repl() {
    let mut vars = HashMap::new();
    let stdin = io::stdin();
    print!("> ");
    io::stdout().flush().unwrap();
    for line in stdin.lock().lines() {
        let line = line.expect("read stdin");
        let trimmed = line.trim();
        if trimmed == "q" || trimmed == "quit" {
            break;
        }
        if !trimmed.is_empty() {
            match eval_line(trimmed, &mut vars) {
                Ok((name, value)) if name.is_empty() => println!("{value}"),
                Ok((name, value)) => println!("{name} = {value}"),
                Err(e) => println!("error: {e}"),
            }
        }
        print!("> ");
        io::stdout().flush().unwrap();
    }
}

fn main() -> ExitCode {
    let expr = env::args().skip(1).collect::<Vec<_>>().join(" ");
    if expr.is_empty() {
        println!("RPN calculator. 'x = 3 4 +' assigns, 'q' quits.");
        repl();
        return ExitCode::SUCCESS;
    }
    match eval(&expr, &HashMap::new()) {
        Ok(value) => {
            println!("{value}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("rpn: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eval0(input: &str) -> Result<f64, RpnError> {
        eval(input, &HashMap::new())
    }

    #[test]
    fn basic_arithmetic() {
        assert_eq!(eval0("3 4 +").unwrap(), 7.0);
        assert_eq!(eval0("3 4 + 2 *").unwrap(), 14.0);
        assert_eq!(eval0("10 2 -").unwrap(), 8.0);
        assert_eq!(eval0("2 10 ^").unwrap(), 1024.0);
        assert_eq!(eval0("7 2 /").unwrap(), 3.5);
    }

    #[test]
    fn operand_order_matters() {
        assert_eq!(eval0("1 2 -").unwrap(), -1.0);
        assert_eq!(eval0("8 2 /").unwrap(), 4.0);
    }

    #[test]
    fn errors() {
        assert_eq!(eval0("3 +"), Err(RpnError::StackUnderflow('+')));
        assert_eq!(eval0("1 0 /"), Err(RpnError::DivisionByZero));
        assert_eq!(eval0("1 2 3 +"), Err(RpnError::LeftoverValues(2)));
        assert_eq!(eval0(""), Err(RpnError::EmptyExpression));
        assert_eq!(eval0("3 4 $"), Err(RpnError::BadToken("$".into())));
        assert_eq!(eval0("x 1 +"), Err(RpnError::UnknownVar("x".into())));
    }

    #[test]
    fn variables() {
        let mut vars = HashMap::new();
        let (name, value) = eval_line("x = 3 4 +", &mut vars).unwrap();
        assert_eq!((name.as_str(), value), ("x", 7.0));
        assert_eq!(eval("x 2 *", &vars).unwrap(), 14.0);

        eval_line("y = x x *", &mut vars).unwrap();
        assert_eq!(eval("y", &vars).unwrap(), 49.0);
    }

    #[test]
    fn bad_assignment_target() {
        let mut vars = HashMap::new();
        assert!(eval_line("3x = 1", &mut vars).is_err());
    }
}
