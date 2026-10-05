# uniqr — uniq clone

Collapses adjacent duplicate lines; `-c` prefixes counts, `-d` keeps only
duplicates, `-u` only uniques. Reads a file or stdin, writes a file or
stdout.

```sh
sort words.txt | cargo run -- -c
cargo run -- input.txt output.txt
cargo test
```

## Covers

Streaming with one line of lookbehind (no whole-file buffer), writing to an
abstract `Box<dyn Write>`, closures capturing flags, `tempfile` in tests.

## Rewrite exercises

1. Rewrite from scratch; get the "flush the last group" edge case right.
2. Add `-i` (case-insensitive comparison) — what happens to the printed line?
3. Add `-f N`: skip the first N fields when comparing (real uniq has it).
4. Make it a true pipeline citizen: handle `SIGPIPE`/broken pipe without a
   panic message when piped into `head`.
5. Compare performance with `String` reuse (`read_line` + `mem::swap`)
   against allocating `lines()`.
