# catr — cat clone

Prints files (or stdin, `-`) with `-n` line numbers or `-b`
number-nonblank. First project with a real argument parser and end-to-end
binary tests.

```sh
cargo run -- -n src/main.rs
echo hi | cargo run
cargo test          # unit + assert_cmd integration tests
```

## Covers

`clap` derive (flags, defaults, `conflicts_with`), `anyhow::Context` for
error chains, `Box<dyn BufRead>`, exit codes, `assert_cmd` + `predicates`
for testing the compiled binary.

## Rewrite exercises

1. Rewrite from scratch; keep `print_file` testable (generic `impl Write`).
2. Add `-E` (show `$` at line ends) and `-s` (squeeze repeated blank lines).
3. Make numbering per-file instead of shared — which did GNU cat choose?
4. Add `--max-lines N` that stops cleanly mid-file.
5. Benchmark `lines()` (allocates per line) vs `read_line` into a reused
   buffer on a big file.
