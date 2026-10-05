# grepr — mini grep

The Rust-book classic, extended: search a pattern in files with `-i`
(case-insensitive), `-n` (line numbers), `-v` (invert). Exit codes match
real grep (0 found, 1 not found, 2 error).

```sh
cargo run -- -n fn src/main.rs
GREPR_IGNORE_CASE=1 cargo run -- RUST src/main.rs
cargo test
```

## Covers

Explicit lifetimes (`fn search<'a>(... contents: &'a str) -> Vec<(usize, &'a str)>`),
borrowing instead of cloning, `env::var` configuration, argument parsing by
hand, exit codes.

## Rewrite exercises

1. Rewrite from scratch; write `search`'s signature (with the lifetime)
   before the body.
2. Add `-c`: print only the count of matching lines per file.
3. Add context lines `-A n` / `-B n` like real grep.
4. Read stdin when no files are given — what changes about the lifetimes?
5. Replace `contains` with your own substring search (two nested loops),
   then with the `regex` crate — compare all three.
