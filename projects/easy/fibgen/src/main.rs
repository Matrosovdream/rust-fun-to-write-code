//! fibgen — number sequences as lazy iterators.
//!
//!   fibgen fib 10         # first 10 Fibonacci numbers
//!   fibgen primes 10      # first 10 primes
//!   fibgen collatz 27     # Collatz trajectory of 27
//!   fibgen fibsum 20      # sum of the first 20 even Fibonacci numbers

use std::env;
use std::process::ExitCode;

/// Infinite Fibonacci sequence: 0, 1, 1, 2, 3, ...
/// `Iterator` has one required method; everything else (take, filter, sum)
/// comes for free.
struct Fib {
    pair: Option<(u64, u64)>,
}

impl Fib {
    fn new() -> Self {
        Fib { pair: Some((0, 1)) }
    }
}

impl Iterator for Fib {
    type Item = u64;

    fn next(&mut self) -> Option<u64> {
        let (current, next) = self.pair?;
        // checked_add: the sequence overflows u64 past index 92 —
        // ending the iterator beats panicking.
        self.pair = current.checked_add(next).map(|sum| (next, sum));
        Some(current)
    }
}

/// Infinite primes, trial division — plenty for an easy project.
struct Primes {
    found: Vec<u64>,
    candidate: u64,
}

impl Primes {
    fn new() -> Self {
        Primes { found: Vec::new(), candidate: 2 }
    }
}

impl Iterator for Primes {
    type Item = u64;

    fn next(&mut self) -> Option<u64> {
        loop {
            let c = self.candidate;
            self.candidate += 1;
            let is_prime = self
                .found
                .iter()
                .take_while(|&&p| p * p <= c)
                .all(|&p| !c.is_multiple_of(p));
            if is_prime {
                self.found.push(c);
                return Some(c);
            }
        }
    }
}

/// Finite: the Collatz trajectory of n down to 1 (inclusive).
struct Collatz {
    current: Option<u64>,
}

impl Collatz {
    fn new(start: u64) -> Self {
        Collatz { current: (start > 0).then_some(start) }
    }
}

impl Iterator for Collatz {
    type Item = u64;

    fn next(&mut self) -> Option<u64> {
        let n = self.current?;
        self.current = match n {
            1 => None,
            n if n % 2 == 0 => Some(n / 2),
            n => Some(3 * n + 1),
        };
        Some(n)
    }
}

fn run(cmd: &str, n: u64) -> Result<String, String> {
    let join = |v: Vec<u64>| {
        v.iter().map(u64::to_string).collect::<Vec<_>>().join(" ")
    };
    match cmd {
        "fib" => Ok(join(Fib::new().take(n as usize).collect())),
        "primes" => Ok(join(Primes::new().take(n as usize).collect())),
        "collatz" => {
            if n == 0 {
                return Err("collatz needs a positive start".to_string());
            }
            let path: Vec<u64> = Collatz::new(n).collect();
            Ok(format!("{} ({} steps)", join(path.clone()), path.len() - 1))
        }
        // Composing adapters on a custom iterator — the payoff.
        "fibsum" => Ok(Fib::new()
            .filter(|x| x % 2 == 0)
            .take(n as usize)
            .sum::<u64>()
            .to_string()),
        _ => Err(format!("unknown command '{cmd}'")),
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let (Some(cmd), Some(n)) = (args.first(), args.get(1).and_then(|s| s.parse().ok())) else {
        eprintln!("usage: fibgen <fib|primes|collatz|fibsum> <n>");
        return ExitCode::FAILURE;
    };
    match run(cmd, n) {
        Ok(out) => {
            println!("{out}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("fibgen: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fib_prefix() {
        let first10: Vec<u64> = Fib::new().take(10).collect();
        assert_eq!(first10, vec![0, 1, 1, 2, 3, 5, 8, 13, 21, 34]);
    }

    #[test]
    fn fib_ends_instead_of_overflowing() {
        // Finite because of checked_add: collect() terminates.
        let all: Vec<u64> = Fib::new().collect();
        assert_eq!(all.len(), 93);
    }

    #[test]
    fn primes_prefix() {
        let first10: Vec<u64> = Primes::new().take(10).collect();
        assert_eq!(first10, vec![2, 3, 5, 7, 11, 13, 17, 19, 23, 29]);
    }

    #[test]
    fn collatz_of_6() {
        let path: Vec<u64> = Collatz::new(6).collect();
        assert_eq!(path, vec![6, 3, 10, 5, 16, 8, 4, 2, 1]);
    }

    #[test]
    fn collatz_of_1_is_just_1() {
        assert_eq!(Collatz::new(1).collect::<Vec<_>>(), vec![1]);
    }

    #[test]
    fn adapters_compose_with_custom_iterators() {
        // Sum of even Fibonacci numbers up to 4,000,000 (Project Euler #2).
        let sum: u64 = Fib::new().filter(|x| x % 2 == 0).take_while(|&x| x < 4_000_000).sum();
        assert_eq!(sum, 4_613_732);
    }

    #[test]
    fn run_commands() {
        assert_eq!(run("fib", 5).unwrap(), "0 1 1 2 3");
        assert_eq!(run("primes", 3).unwrap(), "2 3 5");
        assert!(run("collatz", 6).unwrap().ends_with("(8 steps)"));
        assert!(run("warp", 1).is_err());
    }
}
