# fizzlab — fizzbuzz with configurable rules

FizzBuzz where the rules come from the command line, so the naive if-chain
doesn't survive.

```sh
cargo run                         # classic, 1..20
cargo run -- 30 3=Fizz 5=Buzz 7=Boom
cargo test
```

## Covers

Tuples, `Vec`, `String` building, `split_once`, iterator chains
(`filter`/`map`/`collect`), error messages as `Result<_, String>`.

## Rewrite exercises

1. Rewrite from scratch using only iterator chains in `line` (no `for`).
2. Add `--start` so the range doesn't have to begin at 1.
3. Read rules from a file (one `divisor=word` per line) when `-f file` given.
4. Make rules match on "contains digit" too: `d3=Lucky` fires on 13, 23, 31…
5. Print in N columns (`--cols 5`) with aligned cells.
