//! rockpaper — rock-paper-scissors against a random opponent, best of N.

use std::fmt;
use std::io::{self, Write};

use rand::Rng;
use rand::seq::IndexedRandom;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Move {
    Rock,
    Paper,
    Scissors,
}

const MOVES: [Move; 3] = [Move::Rock, Move::Paper, Move::Scissors];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Win,
    Lose,
    Draw,
}

impl Move {
    fn parse(s: &str) -> Option<Move> {
        match s.trim().to_ascii_lowercase().as_str() {
            "r" | "rock" => Some(Move::Rock),
            "p" | "paper" => Some(Move::Paper),
            "s" | "scissors" => Some(Move::Scissors),
            _ => None,
        }
    }

    /// Outcome for `self` against `other`, from self's point of view.
    fn against(self, other: Move) -> Outcome {
        use Move::*;
        match (self, other) {
            (a, b) if a == b => Outcome::Draw,
            (Rock, Scissors) | (Paper, Rock) | (Scissors, Paper) => Outcome::Win,
            _ => Outcome::Lose,
        }
    }
}

impl fmt::Display for Move {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Move::Rock => "rock",
            Move::Paper => "paper",
            Move::Scissors => "scissors",
        };
        f.write_str(name)
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq)]
struct Score {
    you: u32,
    bot: u32,
    draws: u32,
}

impl Score {
    fn record(&mut self, outcome: Outcome) {
        match outcome {
            Outcome::Win => self.you += 1,
            Outcome::Lose => self.bot += 1,
            Outcome::Draw => self.draws += 1,
        }
    }

    /// Best-of-n: the match ends when someone has the majority of wins.
    fn winner(&self, best_of: u32) -> Option<&'static str> {
        let needed = best_of / 2 + 1;
        if self.you >= needed {
            Some("you")
        } else if self.bot >= needed {
            Some("bot")
        } else {
            None
        }
    }
}

fn read_line(prompt: &str) -> String {
    print!("{prompt}");
    io::stdout().flush().expect("flush stdout");
    let mut line = String::new();
    io::stdin().read_line(&mut line).expect("read stdin");
    line.trim().to_string()
}

fn main() {
    let best_of: u32 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .filter(|n| n % 2 == 1)
        .unwrap_or(3);

    println!("Best of {best_of}. Moves: [r]ock, [p]aper, [s]cissors, [q]uit.");
    let mut rng = rand::rng();
    let mut score = Score::default();

    loop {
        let input = read_line("> ");
        if input.eq_ignore_ascii_case("q") {
            println!("Forfeit. Final score: you {} — {} bot.", score.you, score.bot);
            return;
        }
        let Some(you) = Move::parse(&input) else {
            println!("Unknown move '{input}'.");
            continue;
        };
        let bot = *MOVES.choose(&mut rng).expect("MOVES is not empty");
        let outcome = you.against(bot);
        score.record(outcome);

        let verdict = match outcome {
            Outcome::Win => "you win the round",
            Outcome::Lose => "bot wins the round",
            Outcome::Draw => "draw",
        };
        println!(
            "You: {you}, bot: {bot} — {verdict}. Score {}:{} ({} draws)",
            score.you, score.bot, score.draws
        );

        if let Some(winner) = score.winner(best_of) {
            println!("Match over — {winner} won!");
            // A tiny flourish: the bot comments on the result.
            let quip = if winner == "you" { "Well played." } else { "Better luck next time." };
            println!("{quip} (bot rolled {} rounds)", rng.random_range(1..=6));
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Move::*;
    use Outcome::*;

    #[test]
    fn every_pair_has_the_right_outcome() {
        let cases = [
            (Rock, Rock, Draw),
            (Rock, Paper, Lose),
            (Rock, Scissors, Win),
            (Paper, Rock, Win),
            (Paper, Paper, Draw),
            (Paper, Scissors, Lose),
            (Scissors, Rock, Lose),
            (Scissors, Paper, Win),
            (Scissors, Scissors, Draw),
        ];
        for (a, b, want) in cases {
            assert_eq!(a.against(b), want, "{a:?} vs {b:?}");
        }
    }

    #[test]
    fn parsing_accepts_short_and_long_forms() {
        assert_eq!(Move::parse("r"), Some(Rock));
        assert_eq!(Move::parse(" PAPER "), Some(Paper));
        assert_eq!(Move::parse("lizard"), None);
    }

    #[test]
    fn best_of_three_ends_at_two_wins() {
        let mut score = Score::default();
        score.record(Win);
        assert_eq!(score.winner(3), None);
        score.record(Draw);
        assert_eq!(score.winner(3), None);
        score.record(Win);
        assert_eq!(score.winner(3), Some("you"));
    }

    #[test]
    fn draws_do_not_count_toward_winning() {
        let mut score = Score::default();
        for _ in 0..10 {
            score.record(Draw);
        }
        assert_eq!(score.winner(3), None);
        assert_eq!(score.draws, 10);
    }
}
