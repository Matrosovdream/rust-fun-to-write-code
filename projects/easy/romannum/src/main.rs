//! romannum — converts between roman numerals and arabic numbers, both ways.
//!
//!   romannum XIV      # 14
//!   romannum 2024     # MMXXIV

use std::env;
use std::process::ExitCode;

/// Values and symbols in descending order, subtractive pairs included.
const TABLE: &[(u32, &str)] = &[
    (1000, "M"),
    (900, "CM"),
    (500, "D"),
    (400, "CD"),
    (100, "C"),
    (90, "XC"),
    (50, "L"),
    (40, "XL"),
    (10, "X"),
    (9, "IX"),
    (5, "V"),
    (4, "IV"),
    (1, "I"),
];

const MAX: u32 = 3999;

fn to_roman(mut n: u32) -> Result<String, String> {
    if n == 0 || n > MAX {
        return Err(format!("number must be 1..={MAX}, got {n}"));
    }
    let mut out = String::new();
    // Greedy: take the largest value that still fits, as many times as it fits.
    for &(value, symbol) in TABLE {
        while n >= value {
            out.push_str(symbol);
            n -= value;
        }
    }
    Ok(out)
}

fn digit_value(c: char) -> Option<u32> {
    match c {
        'I' => Some(1),
        'V' => Some(5),
        'X' => Some(10),
        'L' => Some(50),
        'C' => Some(100),
        'D' => Some(500),
        'M' => Some(1000),
        _ => None,
    }
}

fn from_roman(s: &str) -> Result<u32, String> {
    if s.is_empty() {
        return Err("empty numeral".to_string());
    }
    let digits: Vec<u32> = s
        .chars()
        .map(|c| digit_value(c.to_ascii_uppercase()).ok_or(format!("invalid character '{c}'")))
        .collect::<Result<_, _>>()?;

    // A digit smaller than its right neighbour is subtracted (IV = 4).
    let mut total = 0;
    for pair in digits.windows(2) {
        let [current, next] = pair else { unreachable!() };
        if current < next {
            total -= *current as i64;
        } else {
            total += *current as i64;
        }
    }
    total += *digits.last().unwrap() as i64;

    // Strictness check: "IIII" and "IC" sum fine but aren't canonical.
    // Re-encoding the value and comparing catches every malformed numeral.
    let value = u32::try_from(total).map_err(|_| format!("'{s}' is not a valid numeral"))?;
    let canonical = to_roman(value)?;
    if canonical != s.to_ascii_uppercase() {
        return Err(format!("'{s}' is not canonical (did you mean {canonical}?)"));
    }
    Ok(value)
}

fn convert(input: &str) -> Result<String, String> {
    match input.parse::<u32>() {
        Ok(n) => to_roman(n),
        Err(_) => from_roman(input).map(|n| n.to_string()),
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("usage: romannum <number|numeral>...   e.g. romannum 2024 XIV");
        return ExitCode::FAILURE;
    }
    let mut failed = false;
    for arg in &args {
        match convert(arg) {
            Ok(out) => println!("{arg} = {out}"),
            Err(e) => {
                eprintln!("romannum: {e}");
                failed = true;
            }
        }
    }
    if failed { ExitCode::FAILURE } else { ExitCode::SUCCESS }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_numerals() {
        let cases = [
            (1, "I"),
            (4, "IV"),
            (9, "IX"),
            (14, "XIV"),
            (40, "XL"),
            (90, "XC"),
            (400, "CD"),
            (1990, "MCMXC"),
            (2024, "MMXXIV"),
            (3999, "MMMCMXCIX"),
        ];
        for (n, roman) in cases {
            assert_eq!(to_roman(n).unwrap(), roman, "to_roman({n})");
            assert_eq!(from_roman(roman).unwrap(), n, "from_roman({roman})");
        }
    }

    #[test]
    fn round_trip_over_full_range() {
        for n in 1..=MAX {
            let roman = to_roman(n).unwrap();
            assert_eq!(from_roman(&roman).unwrap(), n, "round trip failed for {n}");
        }
    }

    #[test]
    fn lowercase_is_accepted() {
        assert_eq!(from_roman("xiv").unwrap(), 14);
    }

    #[test]
    fn out_of_range_numbers() {
        assert!(to_roman(0).is_err());
        assert!(to_roman(4000).is_err());
    }

    #[test]
    fn malformed_numerals_rejected() {
        for bad in ["IIII", "VV", "IC", "XM", "IXIX", "ABC", ""] {
            assert!(from_roman(bad).is_err(), "'{bad}' should be rejected");
        }
    }
}
