//! wallet — in-memory account ledger with a tiny REPL.
//!
//! Money is i64 cents: floats and money don't mix.
//!
//!   > new alice
//!   > dep alice 100.50
//!   > xfer alice bob 25
//!   > bal
//!   > hist alice

use std::collections::HashMap;
use std::fmt;
use std::io::{self, BufRead, Write};

type Cents = i64;

#[derive(Debug, Clone, PartialEq)]
enum Transaction {
    Deposit { to: String, amount: Cents },
    Withdrawal { from: String, amount: Cents },
    Transfer { from: String, to: String, amount: Cents },
}

impl Transaction {
    fn involves(&self, name: &str) -> bool {
        match self {
            Transaction::Deposit { to, .. } => to == name,
            Transaction::Withdrawal { from, .. } => from == name,
            Transaction::Transfer { from, to, .. } => from == name || to == name,
        }
    }
}

impl fmt::Display for Transaction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Transaction::Deposit { to, amount } => {
                write!(f, "deposit  {:>10} -> {to}", fmt_cents(*amount))
            }
            Transaction::Withdrawal { from, amount } => {
                write!(f, "withdraw {:>10} <- {from}", fmt_cents(*amount))
            }
            Transaction::Transfer { from, to, amount } => {
                write!(f, "transfer {:>10} {from} -> {to}", fmt_cents(*amount))
            }
        }
    }
}

#[derive(Debug, PartialEq)]
enum LedgerError {
    NoSuchAccount(String),
    AccountExists(String),
    InsufficientFunds { name: String, balance: Cents, needed: Cents },
    BadAmount(String),
    SelfTransfer,
}

impl fmt::Display for LedgerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LedgerError::NoSuchAccount(name) => write!(f, "no account '{name}'"),
            LedgerError::AccountExists(name) => write!(f, "account '{name}' already exists"),
            LedgerError::InsufficientFunds { name, balance, needed } => write!(
                f,
                "'{name}' has {} but needs {}",
                fmt_cents(*balance),
                fmt_cents(*needed)
            ),
            LedgerError::BadAmount(s) => write!(f, "bad amount '{s}'"),
            LedgerError::SelfTransfer => write!(f, "can't transfer to the same account"),
        }
    }
}

#[derive(Debug, Default)]
struct Ledger {
    accounts: HashMap<String, Cents>,
    history: Vec<Transaction>,
}

impl Ledger {
    fn create(&mut self, name: &str) -> Result<(), LedgerError> {
        if self.accounts.contains_key(name) {
            return Err(LedgerError::AccountExists(name.to_string()));
        }
        self.accounts.insert(name.to_string(), 0);
        Ok(())
    }

    fn balance(&self, name: &str) -> Result<Cents, LedgerError> {
        self.accounts
            .get(name)
            .copied()
            .ok_or_else(|| LedgerError::NoSuchAccount(name.to_string()))
    }

    fn deposit(&mut self, name: &str, amount: Cents) -> Result<(), LedgerError> {
        let balance = self
            .accounts
            .get_mut(name)
            .ok_or_else(|| LedgerError::NoSuchAccount(name.to_string()))?;
        *balance += amount;
        self.history.push(Transaction::Deposit { to: name.to_string(), amount });
        Ok(())
    }

    fn withdraw(&mut self, name: &str, amount: Cents) -> Result<(), LedgerError> {
        let balance = self
            .accounts
            .get_mut(name)
            .ok_or_else(|| LedgerError::NoSuchAccount(name.to_string()))?;
        // checked_sub + the guard below: an overdraft is unrepresentable.
        *balance = balance
            .checked_sub(amount)
            .filter(|b| *b >= 0)
            .ok_or(LedgerError::InsufficientFunds {
                name: name.to_string(),
                balance: *balance,
                needed: amount,
            })?;
        self.history.push(Transaction::Withdrawal { from: name.to_string(), amount });
        Ok(())
    }

    /// The borrow checker won't give out two `&mut` into one map at once —
    /// so: validate everything first, then mutate one account at a time.
    fn transfer(&mut self, from: &str, to: &str, amount: Cents) -> Result<(), LedgerError> {
        if from == to {
            return Err(LedgerError::SelfTransfer);
        }
        let from_balance = self.balance(from)?;
        self.balance(to)?; // both must exist before anything changes
        if from_balance < amount {
            return Err(LedgerError::InsufficientFunds {
                name: from.to_string(),
                balance: from_balance,
                needed: amount,
            });
        }
        *self.accounts.get_mut(from).expect("checked above") -= amount;
        *self.accounts.get_mut(to).expect("checked above") += amount;
        self.history.push(Transaction::Transfer {
            from: from.to_string(),
            to: to.to_string(),
            amount,
        });
        Ok(())
    }

    fn history_of<'a>(&'a self, name: &str) -> Vec<&'a Transaction> {
        self.history.iter().filter(|t| t.involves(name)).collect()
    }

    fn total(&self) -> Cents {
        self.accounts.values().sum()
    }
}

fn fmt_cents(cents: Cents) -> String {
    format!("{}.{:02}", cents / 100, (cents % 100).abs())
}

/// Parses "100", "100.5", "100.50" into cents. Rejects negatives and
/// sub-cent precision.
fn parse_amount(s: &str) -> Result<Cents, LedgerError> {
    let bad = || LedgerError::BadAmount(s.to_string());
    let (whole, frac) = match s.split_once('.') {
        None => (s, "0"),
        Some((w, f)) if f.len() <= 2 && !f.is_empty() => (w, f),
        Some(_) => return Err(bad()),
    };
    let whole: Cents = whole.parse().map_err(|_| bad())?;
    let frac: Cents = frac.parse().map_err(|_| bad())?;
    if whole < 0 || frac < 0 {
        return Err(bad());
    }
    let frac = if s.split_once('.').is_some_and(|(_, f)| f.len() == 1) {
        frac * 10
    } else {
        frac
    };
    Ok(whole * 100 + frac)
}

const HELP: &str = "commands:
  new <name>                 create account
  dep <name> <amount>        deposit
  wd  <name> <amount>        withdraw
  xfer <from> <to> <amount>  transfer
  bal                        balances + total
  hist <name>                account history
  q                          quit";

fn execute(ledger: &mut Ledger, line: &str) -> Result<String, LedgerError> {
    let words: Vec<&str> = line.split_whitespace().collect();
    match words.as_slice() {
        ["new", name] => {
            ledger.create(name)?;
            Ok(format!("created '{name}'"))
        }
        ["dep", name, amount] => {
            ledger.deposit(name, parse_amount(amount)?)?;
            Ok(format!("{name}: {}", fmt_cents(ledger.balance(name)?)))
        }
        ["wd", name, amount] => {
            ledger.withdraw(name, parse_amount(amount)?)?;
            Ok(format!("{name}: {}", fmt_cents(ledger.balance(name)?)))
        }
        ["xfer", from, to, amount] => {
            ledger.transfer(from, to, parse_amount(amount)?)?;
            Ok("ok".to_string())
        }
        ["bal"] => {
            let mut names: Vec<&String> = ledger.accounts.keys().collect();
            names.sort();
            let mut out: Vec<String> = names
                .iter()
                .map(|n| format!("{n}: {}", fmt_cents(ledger.accounts[*n])))
                .collect();
            out.push(format!("total: {}", fmt_cents(ledger.total())));
            Ok(out.join("\n"))
        }
        ["hist", name] => {
            ledger.balance(name)?; // validate the account exists
            let lines: Vec<String> =
                ledger.history_of(name).iter().map(|t| t.to_string()).collect();
            Ok(if lines.is_empty() { "no transactions".to_string() } else { lines.join("\n") })
        }
        _ => Ok(HELP.to_string()),
    }
}

fn main() {
    let mut ledger = Ledger::default();
    println!("wallet — type 'help' for commands");
    let stdin = io::stdin();
    print!("> ");
    io::stdout().flush().unwrap();
    for line in stdin.lock().lines() {
        let line = line.expect("read stdin");
        if line.trim() == "q" {
            break;
        }
        if !line.trim().is_empty() {
            match execute(&mut ledger, &line) {
                Ok(out) => println!("{out}"),
                Err(e) => println!("error: {e}"),
            }
        }
        print!("> ");
        io::stdout().flush().unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ledger_with(name: &str, cents: Cents) -> Ledger {
        let mut ledger = Ledger::default();
        ledger.create(name).unwrap();
        ledger.deposit(name, cents).unwrap();
        ledger
    }

    #[test]
    fn deposit_and_withdraw() {
        let mut ledger = ledger_with("alice", 10_000);
        ledger.withdraw("alice", 2_500).unwrap();
        assert_eq!(ledger.balance("alice").unwrap(), 7_500);
    }

    #[test]
    fn overdraft_is_rejected_and_changes_nothing() {
        let mut ledger = ledger_with("alice", 100);
        let err = ledger.withdraw("alice", 200).unwrap_err();
        assert!(matches!(err, LedgerError::InsufficientFunds { balance: 100, needed: 200, .. }));
        assert_eq!(ledger.balance("alice").unwrap(), 100);
        assert_eq!(ledger.history.len(), 1); // only the deposit
    }

    #[test]
    fn transfer_moves_money_and_conserves_total() {
        let mut ledger = ledger_with("alice", 10_000);
        ledger.create("bob").unwrap();
        ledger.transfer("alice", "bob", 2_500).unwrap();
        assert_eq!(ledger.balance("alice").unwrap(), 7_500);
        assert_eq!(ledger.balance("bob").unwrap(), 2_500);
        assert_eq!(ledger.total(), 10_000);
    }

    #[test]
    fn failed_transfer_changes_neither_account() {
        let mut ledger = ledger_with("alice", 100);
        ledger.create("bob").unwrap();
        assert!(ledger.transfer("alice", "bob", 500).is_err());
        assert!(ledger.transfer("alice", "ghost", 50).is_err());
        assert!(ledger.transfer("alice", "alice", 50).is_err());
        assert_eq!(ledger.balance("alice").unwrap(), 100);
        assert_eq!(ledger.balance("bob").unwrap(), 0);
    }

    #[test]
    fn history_filters_by_account() {
        let mut ledger = ledger_with("alice", 1_000);
        ledger.create("bob").unwrap();
        ledger.deposit("bob", 500).unwrap();
        ledger.transfer("alice", "bob", 100).unwrap();
        assert_eq!(ledger.history_of("alice").len(), 2); // deposit + transfer
        assert_eq!(ledger.history_of("bob").len(), 2); // deposit + transfer
    }

    #[test]
    fn amount_parsing() {
        assert_eq!(parse_amount("100").unwrap(), 10_000);
        assert_eq!(parse_amount("100.5").unwrap(), 10_050);
        assert_eq!(parse_amount("100.50").unwrap(), 10_050);
        assert_eq!(parse_amount("0.07").unwrap(), 7);
        assert!(parse_amount("-5").is_err());
        assert!(parse_amount("1.234").is_err());
        assert!(parse_amount("abc").is_err());
        assert!(parse_amount("1.").is_err());
    }

    #[test]
    fn cents_formatting() {
        assert_eq!(fmt_cents(10_050), "100.50");
        assert_eq!(fmt_cents(7), "0.07");
    }

    #[test]
    fn execute_full_session() {
        let mut ledger = Ledger::default();
        execute(&mut ledger, "new alice").unwrap();
        execute(&mut ledger, "dep alice 50").unwrap();
        let out = execute(&mut ledger, "bal").unwrap();
        assert!(out.contains("alice: 50.00"));
        assert!(out.contains("total: 50.00"));
        let err = execute(&mut ledger, "wd alice 60").unwrap_err();
        assert!(matches!(err, LedgerError::InsufficientFunds { .. }));
    }
}
