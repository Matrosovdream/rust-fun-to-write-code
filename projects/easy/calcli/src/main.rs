//! calcli — evaluates arithmetic expressions with precedence and parentheses.
//!
//! Grammar (classic recursive descent):
//!   expr   = term   { ("+" | "-") term }
//!   term   = factor { ("*" | "/" | "%") factor }
//!   factor = NUMBER | "-" factor | "(" expr ")"

use std::env;
use std::error::Error;
use std::fmt;
use std::process::ExitCode;

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Number(f64),
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    LParen,
    RParen,
}

#[derive(Debug, PartialEq)]
enum CalcError {
    BadToken(String),
    UnexpectedEnd,
    TrailingInput(String),
    DivisionByZero,
    Usage,
}

impl fmt::Display for CalcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CalcError::BadToken(s) => write!(f, "unexpected token '{s}'"),
            CalcError::UnexpectedEnd => write!(f, "expression ended unexpectedly"),
            CalcError::TrailingInput(s) => write!(f, "unexpected input after expression: '{s}'"),
            CalcError::DivisionByZero => write!(f, "division by zero"),
            CalcError::Usage => write!(f, "usage: calcli <expression>   e.g. calcli '2 + 3 * (4 - 1)'"),
        }
    }
}

impl Error for CalcError {}

fn tokenize(input: &str) -> Result<Vec<Token>, CalcError> {
    let mut tokens = Vec::new();
    let mut chars = input.chars().peekable();

    while let Some(&c) = chars.peek() {
        match c {
            ' ' | '\t' => {
                chars.next();
            }
            '+' => {
                chars.next();
                tokens.push(Token::Plus);
            }
            '-' => {
                chars.next();
                tokens.push(Token::Minus);
            }
            '*' | 'x' => {
                chars.next();
                tokens.push(Token::Star);
            }
            '/' => {
                chars.next();
                tokens.push(Token::Slash);
            }
            '%' => {
                chars.next();
                tokens.push(Token::Percent);
            }
            '(' => {
                chars.next();
                tokens.push(Token::LParen);
            }
            ')' => {
                chars.next();
                tokens.push(Token::RParen);
            }
            c if c.is_ascii_digit() || c == '.' => {
                let mut num = String::new();
                while let Some(&c) = chars.peek() {
                    if c.is_ascii_digit() || c == '.' {
                        num.push(c);
                        chars.next();
                    } else {
                        break;
                    }
                }
                let value: f64 = num.parse().map_err(|_| CalcError::BadToken(num.clone()))?;
                tokens.push(Token::Number(value));
            }
            other => return Err(CalcError::BadToken(other.to_string())),
        }
    }
    Ok(tokens)
}

struct Parser<'a> {
    tokens: &'a [Token],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn new(tokens: &'a [Token]) -> Self {
        Self { tokens, pos: 0 }
    }

    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn next(&mut self) -> Option<&Token> {
        let t = self.tokens.get(self.pos);
        self.pos += 1;
        t
    }

    fn expr(&mut self) -> Result<f64, CalcError> {
        let mut value = self.term()?;
        while let Some(op) = self.peek().cloned() {
            match op {
                Token::Plus => {
                    self.next();
                    value += self.term()?;
                }
                Token::Minus => {
                    self.next();
                    value -= self.term()?;
                }
                _ => break,
            }
        }
        Ok(value)
    }

    fn term(&mut self) -> Result<f64, CalcError> {
        let mut value = self.factor()?;
        while let Some(op) = self.peek().cloned() {
            match op {
                Token::Star => {
                    self.next();
                    value *= self.factor()?;
                }
                Token::Slash => {
                    self.next();
                    let rhs = self.factor()?;
                    if rhs == 0.0 {
                        return Err(CalcError::DivisionByZero);
                    }
                    value /= rhs;
                }
                Token::Percent => {
                    self.next();
                    let rhs = self.factor()?;
                    if rhs == 0.0 {
                        return Err(CalcError::DivisionByZero);
                    }
                    value %= rhs;
                }
                _ => break,
            }
        }
        Ok(value)
    }

    fn factor(&mut self) -> Result<f64, CalcError> {
        match self.next().cloned() {
            Some(Token::Number(n)) => Ok(n),
            Some(Token::Minus) => Ok(-self.factor()?),
            Some(Token::LParen) => {
                let value = self.expr()?;
                match self.next() {
                    Some(Token::RParen) => Ok(value),
                    Some(t) => Err(CalcError::BadToken(format!("{t:?}"))),
                    None => Err(CalcError::UnexpectedEnd),
                }
            }
            Some(t) => Err(CalcError::BadToken(format!("{t:?}"))),
            None => Err(CalcError::UnexpectedEnd),
        }
    }
}

fn eval(input: &str) -> Result<f64, CalcError> {
    let tokens = tokenize(input)?;
    if tokens.is_empty() {
        return Err(CalcError::Usage);
    }
    let mut parser = Parser::new(&tokens);
    let value = parser.expr()?;
    if parser.pos < tokens.len() {
        return Err(CalcError::TrailingInput(format!("{:?}", tokens[parser.pos])));
    }
    Ok(value)
}

/// Prints floats like a calculator: no trailing `.0` for whole numbers.
fn format_result(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1e15 {
        format!("{value:.0}")
    } else {
        format!("{value}")
    }
}

fn main() -> ExitCode {
    let expr = env::args().skip(1).collect::<Vec<_>>().join(" ");
    match eval(&expr) {
        Ok(value) => {
            println!("{}", format_result(value));
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("calcli: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn precedence_and_parens() {
        assert_eq!(eval("2 + 3 * 4").unwrap(), 14.0);
        assert_eq!(eval("(2 + 3) * 4").unwrap(), 20.0);
        assert_eq!(eval("10 - 2 - 3").unwrap(), 5.0);
        assert_eq!(eval("20 / 4 / 5").unwrap(), 1.0);
        assert_eq!(eval("10 % 3").unwrap(), 1.0);
    }

    #[test]
    fn unary_minus_and_floats() {
        assert_eq!(eval("-5 + 3").unwrap(), -2.0);
        assert_eq!(eval("2 * -3").unwrap(), -6.0);
        assert_eq!(eval("0.5 * 4").unwrap(), 2.0);
    }

    #[test]
    fn division_by_zero() {
        assert_eq!(eval("1 / 0"), Err(CalcError::DivisionByZero));
        assert_eq!(eval("1 % (2 - 2)"), Err(CalcError::DivisionByZero));
    }

    #[test]
    fn bad_input() {
        assert_eq!(eval("2 + $"), Err(CalcError::BadToken("$".into())));
        assert_eq!(eval("2 +"), Err(CalcError::UnexpectedEnd));
        assert_eq!(eval("(2 + 3"), Err(CalcError::UnexpectedEnd));
        assert!(matches!(eval("2 3"), Err(CalcError::TrailingInput(_))));
        assert_eq!(eval(""), Err(CalcError::Usage));
    }

    #[test]
    fn result_formatting() {
        assert_eq!(format_result(14.0), "14");
        assert_eq!(format_result(2.5), "2.5");
    }
}
