# wordcount — wc clone

Counts lines, words, chars, and bytes for files or stdin; multiple files get
a totals row, like real `wc`.

```sh
cargo run -- src/main.rs README.md
echo 'hello world' | cargo run
cargo run -- -l -c src/main.rs
cargo test
```

## Covers

`BufReader` and `read_line`, operator overloading (`impl Add`), chars vs
bytes, `Box<dyn BufRead>` to unify stdin and files, testing with
`io::Cursor`.

## Rewrite exercises

1. Rewrite from scratch; keep counting single-pass.
2. Add `-L`: length of the longest line.
3. Count per-paragraph (blank-line separated) with `--para`.
4. Accept globs: `wordcount 'src/**/*.rs'` (use the `glob` crate).
5. Benchmark `read_line` vs reading the whole file vs `read` chunks — which
   wins and why?
