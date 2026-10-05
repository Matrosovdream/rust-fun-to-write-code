# romannum — roman numerals converter

Converts both ways and rejects malformed numerals (`IIII`, `IC`) by
re-encoding and comparing — a neat validation trick worth remembering.

```sh
cargo run -- 2024 XIV mcmxc
cargo test
```

## Covers

`const` tables of tuples, slices, `chars()`, `windows(2)`, greedy algorithms,
`collect::<Result<_, _>>()`, a full-range round-trip test.

## Rewrite exercises

1. Rewrite from scratch; start from the table, not from if-chains.
2. Support vinculum notation for numbers > 3999 (an overline = ×1000).
3. Add `--lenient` that accepts non-canonical numerals like `IIII`.
4. Implement `FromStr` and `Display` for a `Roman(u32)` newtype instead of
   free functions.
5. Property test: for all n, `from_roman(to_roman(n)) == n` — now write the
   same with proptest.
