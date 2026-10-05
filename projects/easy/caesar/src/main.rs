//! caesar — Caesar and Vigenère ciphers over stdin or a file.
//!
//!   echo 'Attack at dawn' | caesar enc 3
//!   caesar dec 3 message.txt
//!   caesar brute message.txt          # prints all 26 shifts
//!   echo 'Attack at dawn' | caesar venc LEMON
//!   caesar vdec LEMON message.txt

use std::env;
use std::fs;
use std::io::{self, Read};
use std::process::ExitCode;

/// Applies a char transformation to the whole text. Non-ASCII-alphabetic
/// characters pass through untouched inside the transformations themselves.
fn transform(text: &str, mut f: impl FnMut(char) -> char) -> String {
    text.chars().map(&mut f).collect()
}

/// Shifts one ASCII letter by `shift` (0..26), preserving case.
fn shift_char(c: char, shift: u8) -> char {
    let base = match c {
        'a'..='z' => b'a',
        'A'..='Z' => b'A',
        _ => return c,
    };
    // Work in 0..26 space; modulo keeps the wrap-around branchless.
    let offset = (c as u8 - base + shift) % 26;
    (base + offset) as char
}

fn caesar(text: &str, shift: u8) -> String {
    transform(text, |c| shift_char(c, shift % 26))
}

/// Vigenère: each letter of the key shifts one letter of the text.
/// The key advances only on alphabetic characters, like the classic cipher.
fn vigenere(text: &str, key: &str, decrypt: bool) -> String {
    let shifts: Vec<u8> = key
        .chars()
        .filter(|c| c.is_ascii_alphabetic())
        .map(|c| c.to_ascii_lowercase() as u8 - b'a')
        .collect();
    let mut i = 0;
    transform(text, move |c| {
        if !c.is_ascii_alphabetic() {
            return c;
        }
        let mut s = shifts[i % shifts.len()];
        if decrypt {
            s = (26 - s) % 26;
        }
        i += 1;
        shift_char(c, s)
    })
}

fn brute_force(text: &str) -> Vec<(u8, String)> {
    (1..26).map(|shift| (shift, caesar(text, 26 - shift))).collect()
}

fn read_input(file: Option<&str>) -> io::Result<String> {
    match file {
        Some(path) => fs::read_to_string(path),
        None => {
            let mut buf = String::new();
            io::stdin().read_to_string(&mut buf)?;
            Ok(buf)
        }
    }
}

const USAGE: &str = "usage:
  caesar enc <shift> [file]     encrypt (Caesar)
  caesar dec <shift> [file]     decrypt (Caesar)
  caesar brute [file]           print all 26 shifts
  caesar venc <key> [file]      encrypt (Vigenère)
  caesar vdec <key> [file]      decrypt (Vigenère)
Reads stdin when no file is given.";

fn run(args: &[String]) -> Result<String, String> {
    let (cmd, rest) = args.split_first().ok_or(USAGE.to_string())?;

    let parse_shift = |s: &str| -> Result<u8, String> {
        let n: u8 = s.parse().map_err(|_| format!("bad shift '{s}'"))?;
        Ok(n % 26)
    };

    match cmd.as_str() {
        "enc" | "dec" => {
            let [shift, file @ ..] = rest else {
                return Err(USAGE.to_string());
            };
            let mut shift = parse_shift(shift)?;
            if cmd == "dec" {
                shift = (26 - shift) % 26;
            }
            let text = read_input(file.first().map(String::as_str)).map_err(|e| e.to_string())?;
            Ok(caesar(&text, shift))
        }
        "brute" => {
            let text = read_input(rest.first().map(String::as_str)).map_err(|e| e.to_string())?;
            let mut out = String::new();
            for (shift, candidate) in brute_force(&text) {
                out.push_str(&format!("[{shift:2}] {}", candidate));
                if !out.ends_with('\n') {
                    out.push('\n');
                }
            }
            Ok(out.trim_end().to_string())
        }
        "venc" | "vdec" => {
            let [key, file @ ..] = rest else {
                return Err(USAGE.to_string());
            };
            if !key.chars().any(|c| c.is_ascii_alphabetic()) {
                return Err(format!("key '{key}' has no letters"));
            }
            let text = read_input(file.first().map(String::as_str)).map_err(|e| e.to_string())?;
            Ok(vigenere(&text, key, cmd == "vdec"))
        }
        _ => Err(USAGE.to_string()),
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    match run(&args) {
        Ok(out) => {
            print!("{out}");
            if !out.ends_with('\n') {
                println!();
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caesar_shifts_preserving_case() {
        assert_eq!(caesar("Attack at dawn!", 3), "Dwwdfn dw gdzq!");
        assert_eq!(caesar("xyz XYZ", 3), "abc ABC");
    }

    #[test]
    fn caesar_round_trip() {
        let msg = "The quick brown fox; jumps over 13 lazy dogs?";
        for shift in 0..26 {
            let enc = caesar(msg, shift);
            let dec = caesar(&enc, (26 - shift) % 26);
            assert_eq!(dec, msg, "shift {shift}");
        }
    }

    #[test]
    fn non_ascii_passes_through() {
        assert_eq!(caesar("über café 123", 5), "ügjw hfké 123");
    }

    #[test]
    fn vigenere_classic_example() {
        // The canonical Wikipedia example.
        assert_eq!(vigenere("ATTACKATDAWN", "LEMON", false), "LXFOPVEFRNHR");
        assert_eq!(vigenere("LXFOPVEFRNHR", "LEMON", true), "ATTACKATDAWN");
    }

    #[test]
    fn vigenere_key_skips_non_letters() {
        let enc = vigenere("a b c", "bb", false);
        assert_eq!(enc, "b c d");
    }

    #[test]
    fn brute_force_contains_plaintext() {
        let enc = caesar("hello world", 7);
        let found = brute_force(&enc)
            .into_iter()
            .any(|(shift, text)| shift == 7 && text == "hello world");
        assert!(found);
    }
}
