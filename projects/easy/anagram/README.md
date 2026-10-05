# anagram — word-play toolkit

Find anagrams in a dictionary, check palindromes, and find the longest words
spellable from a set of letters. A 25k-word `dict.txt` is bundled.

```sh
cargo run -- find listen
cargo run -- pal 'Step on no pets'
cargo run -- spell pplsae
cargo run -- find stop -d /usr/share/dict/words
cargo test
```

## Covers

`HashMap` grouping with the `entry` API, `HashSet`-style membership via keys,
borrowing `&str` slices out of one big `String`, multiset comparison with
char counts, sorting with `Reverse`.

## Rewrite exercises

1. Rewrite from scratch; design the anagram key first.
2. Load the index once and answer many queries in a REPL — measure the
   speedup vs re-reading the file.
3. Find the *largest anagram group* in the dictionary in one pass.
4. `spell` currently ignores word quality — rank by Scrabble letter scores
   instead of length.
5. Make `find` accept a phrase and find two-word anagrams (hard mode —
   recursion over the letter multiset).
