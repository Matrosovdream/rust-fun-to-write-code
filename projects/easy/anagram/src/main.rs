//! anagram — word-play toolkit over a dictionary file.
//!
//!   anagram find listen             # words that are anagrams of "listen"
//!   anagram pal "step on no pets"   # palindrome check (ignores spaces/case)
//!   anagram spell pplsae            # longest words spellable from letters
//!
//! Dictionary: ./dict.txt by default, override with -d <file>.

use std::collections::HashMap;
use std::env;
use std::fs;
use std::process::ExitCode;

/// Anagram key: lowercase letters, sorted. "Listen" -> "eilnst".
fn normalize(word: &str) -> String {
    let mut letters: Vec<char> = word
        .chars()
        .filter(|c| c.is_alphabetic())
        .flat_map(char::to_lowercase)
        .collect();
    letters.sort_unstable();
    letters.into_iter().collect()
}

/// Groups dictionary words by anagram key. The values *borrow* from the
/// dictionary string — one allocation for the whole file.
fn build_index(dict: &str) -> HashMap<String, Vec<&str>> {
    let mut index: HashMap<String, Vec<&str>> = HashMap::new();
    for word in dict.split_whitespace() {
        // The entry API: insert-or-get in one lookup.
        index.entry(normalize(word)).or_default().push(word);
    }
    index
}

fn find_anagrams<'a>(index: &HashMap<String, Vec<&'a str>>, word: &str) -> Vec<&'a str> {
    index
        .get(&normalize(word))
        .map(|group| {
            group
                .iter()
                .filter(|&&w| !w.eq_ignore_ascii_case(word))
                .copied()
                .collect()
        })
        .unwrap_or_default()
}

fn is_palindrome(text: &str) -> bool {
    let letters: Vec<char> = text
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect();
    !letters.is_empty() && letters.iter().eq(letters.iter().rev())
}

fn letter_counts(word: &str) -> HashMap<char, u32> {
    let mut counts = HashMap::new();
    for c in word.chars().filter(|c| c.is_alphabetic()) {
        for lower in c.to_lowercase() {
            *counts.entry(lower).or_insert(0) += 1;
        }
    }
    counts
}

/// Can `word` be spelled from the multiset of `letters`?
fn can_spell(word: &str, letters: &HashMap<char, u32>) -> bool {
    letter_counts(word)
        .iter()
        .all(|(c, needed)| letters.get(c).copied().unwrap_or(0) >= *needed)
}

/// Longest words first, ties alphabetical, top 10.
fn best_spellable<'a>(dict: &'a str, letters: &str) -> Vec<&'a str> {
    let available = letter_counts(letters);
    let mut words: Vec<&str> = dict
        .split_whitespace()
        .filter(|w| can_spell(w, &available))
        .collect();
    words.sort_by_key(|w| (std::cmp::Reverse(w.chars().count()), w.to_lowercase()));
    words.truncate(10);
    words
}

fn run(args: &[String]) -> Result<String, String> {
    let mut dict_path = "dict.txt".to_string();
    let mut positional = Vec::new();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == "-d" {
            dict_path = iter.next().ok_or("-d needs a file")?.clone();
        } else {
            positional.push(arg.clone());
        }
    }
    let [cmd, word] = positional.as_slice() else {
        return Err("usage: anagram <find|pal|spell> <word> [-d dict.txt]".to_string());
    };

    match cmd.as_str() {
        "pal" => Ok(if is_palindrome(word) {
            format!("'{word}' is a palindrome")
        } else {
            format!("'{word}' is not a palindrome")
        }),
        "find" | "spell" => {
            let dict = fs::read_to_string(&dict_path)
                .map_err(|e| format!("can't read dictionary {dict_path}: {e}"))?;
            let found = match cmd.as_str() {
                "find" => find_anagrams(&build_index(&dict), word),
                _ => best_spellable(&dict, word),
            };
            if found.is_empty() {
                Ok("no matches".to_string())
            } else {
                Ok(found.join("\n"))
            }
        }
        other => Err(format!("unknown command '{other}'")),
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    match run(&args) {
        Ok(out) => {
            println!("{out}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("anagram: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DICT: &str = "listen silent enlist tinsel stop pots tops spot \
                        apple apples race care acre racecar level pea sap plea";

    #[test]
    fn normalize_sorts_and_lowercases() {
        assert_eq!(normalize("Listen"), "eilnst");
        assert_eq!(normalize("silent"), "eilnst");
        assert_eq!(normalize("don't!"), "dnot");
    }

    #[test]
    fn index_groups_anagrams() {
        let index = build_index(DICT);
        let group = index.get("eilnst").unwrap();
        assert_eq!(group, &vec!["listen", "silent", "enlist", "tinsel"]);
    }

    #[test]
    fn find_excludes_the_word_itself() {
        let index = build_index(DICT);
        let found = find_anagrams(&index, "Listen");
        assert_eq!(found, vec!["silent", "enlist", "tinsel"]);
        assert!(find_anagrams(&index, "zebra").is_empty());
    }

    #[test]
    fn palindromes() {
        assert!(is_palindrome("racecar"));
        assert!(is_palindrome("Step on no pets!"));
        assert!(is_palindrome("A man, a plan, a canal: Panama"));
        assert!(!is_palindrome("rust"));
        assert!(!is_palindrome("!!"));
    }

    #[test]
    fn spelling_respects_letter_counts() {
        assert!(can_spell("apple", &letter_counts("aplpeX")));
        // only one 'p' available — "apple" needs two
        assert!(!can_spell("apple", &letter_counts("alpe")));
        assert!(can_spell("plea", &letter_counts("alpe")));
    }

    #[test]
    fn best_spellable_prefers_longer_words() {
        let found = best_spellable(DICT, "ppalse");
        assert_eq!(found.first(), Some(&"apples"));
        assert!(found.contains(&"sap"));
        assert!(!found.contains(&"race"));
    }
}
