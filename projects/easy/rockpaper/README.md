# rockpaper — rock-paper-scissors vs computer

Best-of-N (default 3) match against a random bot, with score tracking.

```sh
cargo run          # best of 3
cargo run -- 5     # best of 5
cargo test
```

## Covers

`impl` blocks on enums, `match` on tuples, deriving
`Debug`/`Clone`/`Copy`/`PartialEq`, `Default` for state structs, picking a
random element with `rand`.

## Rewrite exercises

1. Rewrite from scratch; make `against` a tuple-match, not nested ifs.
2. Add Lizard and Spock — watch the compiler find every incomplete `match`.
3. Make the bot exploit you: it tracks your move frequencies and counters
   the most common one.
4. Extract the game into `lib.rs` so tests don't live in `main.rs`.
5. Implement `Display` for `Score` instead of formatting inline.
