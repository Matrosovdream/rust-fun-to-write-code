use std::cmp::Ordering;
use std::io::{self, Write};

use rand::Rng;

const MIN: u32 = 1;
const MAX: u32 = 100;

#[derive(Debug, PartialEq)]
enum Hint {
    TooLow,
    TooHigh,
    Correct,
}

fn check(guess: u32, secret: u32) -> Hint {
    match guess.cmp(&secret) {
        Ordering::Less => Hint::TooLow,
        Ordering::Greater => Hint::TooHigh,
        Ordering::Equal => Hint::Correct,
    }
}

/// Rates a finished game by attempt count.
fn rating(attempts: u32) -> &'static str {
    match attempts {
        1 => "Pure luck!",
        2..=6 => "Great — that's binary-search territory.",
        7..=10 => "Not bad.",
        _ => "Were you guessing in order?",
    }
}

fn read_line(prompt: &str) -> String {
    print!("{prompt}");
    io::stdout().flush().expect("flush stdout");
    let mut line = String::new();
    io::stdin().read_line(&mut line).expect("read stdin");
    line.trim().to_string()
}

fn play() {
    let secret = rand::rng().random_range(MIN..=MAX);
    let mut attempts: u32 = 0;

    println!("I picked a number between {MIN} and {MAX}. Guess it!");

    loop {
        let input = read_line("> ");

        // `parse` returns a Result: bad input is a normal case, not a crash.
        let guess: u32 = match input.parse() {
            Ok(n) => n,
            Err(_) => {
                println!("'{input}' is not a number, try again.");
                continue;
            }
        };

        if !(MIN..=MAX).contains(&guess) {
            println!("Out of range — the number is between {MIN} and {MAX}.");
            continue;
        }

        attempts += 1;

        match check(guess, secret) {
            Hint::TooLow => println!("Too low."),
            Hint::TooHigh => println!("Too high."),
            Hint::Correct => {
                println!("Correct! You got it in {attempts} attempt(s). {}", rating(attempts));
                break;
            }
        }
    }
}

fn main() {
    loop {
        play();
        let again = read_line("Play again? [y/N] ");
        if !again.eq_ignore_ascii_case("y") {
            println!("Bye!");
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_reports_too_low() {
        assert_eq!(check(10, 50), Hint::TooLow);
    }

    #[test]
    fn check_reports_too_high() {
        assert_eq!(check(90, 50), Hint::TooHigh);
    }

    #[test]
    fn check_reports_correct() {
        assert_eq!(check(50, 50), Hint::Correct);
    }

    #[test]
    fn rating_covers_all_attempt_counts() {
        assert_eq!(rating(1), "Pure luck!");
        assert!(rating(4).contains("binary-search"));
        assert_eq!(rating(8), "Not bad.");
        assert!(rating(42).contains("in order"));
    }
}
