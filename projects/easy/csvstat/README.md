# csvstat — CSV statistics

Per-column stats: numeric columns get count/min/max/mean/median, text
columns get distinct counts and the most common value. Empty cells are
missing data, not errors.

```sh
cargo run -- sample.csv
cat sample.csv | cargo run
cargo run -- data.csv --no-header
cargo test
```

## Covers

The `csv` crate (`ReaderBuilder`, flexible/ragged rows), deciding types by
`collect::<Option<Vec<_>>>`, a generic `min_max<T: PartialOrd + Copy>`,
median via `sort_by(f64::total_cmp)`, handling missing data without panics.

## Rewrite exercises

1. Rewrite from scratch; decide early how a column "becomes" numeric.
2. Stream instead of loading all columns: compute count/min/max/mean in one
   pass (median needs a different idea — go read about t-digest, implement
   reservoir sampling as the easy version).
3. Add `--col age` to analyze one column, and `--json` output via serde.
4. Detect dates (`2024-01-15`) as a third column kind.
5. Add standard deviation — then make `Stats` generic over `f64`/`i64`
   using the `num-traits` crate.
