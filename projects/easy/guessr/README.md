# guessr — number guessing game

The program picks a number between 1 and 100; you guess, it answers
higher/lower, counts attempts, and rates your result.

## Run

```sh
cargo run
cargo test
```

## Covers

`let`/`mut`, `loop`/`continue`/`break`, `match`, `Ordering`, parsing with
`Result`, ranges, the `rand` crate.

## Rewrite exercises

1. Rewrite it from scratch without peeking.
2. Add difficulty levels: easy 1–50, hard 1–1000, chosen at start.
3. Limit the attempts (e.g. 7) and reveal the number on defeat.
4. Track a best-score table across rounds within one run.
5. Invert the roles: you pick the number, the program binary-searches it —
   you answer `h`/`l`/`=`.
