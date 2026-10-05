# headr — head clone

First N lines (`-n`, default 10) or first N bytes (`-c`); multiple files get
`==> name <==` headers like real `head`.

```sh
cargo run -- -n 5 src/main.rs
cargo run -- -c 20 src/main.rs README.md
cargo test
```

## Covers

Bytes vs lines for real, `Read::take`, `read_line` preserving line endings,
`String::from_utf8_lossy` when a cut splits a UTF-8 char, clap value ranges
and conflicting flags.

## Rewrite exercises

1. Rewrite from scratch; make the no-trailing-newline case correct.
2. Add negative counts like GNU head: `-n -3` = all but the last 3 lines.
3. Implement `tail -n` next to it — why is it fundamentally harder?
4. Accept `-c 1K` / `-c 2M` suffixes.
5. In bytes mode, trim to the last complete UTF-8 boundary instead of
   emitting U+FFFD — `str::floor_char_boundary` exists; do it by hand first.
