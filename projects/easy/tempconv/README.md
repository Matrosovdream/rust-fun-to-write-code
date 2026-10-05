# tempconv — unit converter CLI

Converts temperature, length, and weight between units:

```sh
cargo run -- 100 c f        # 100°C = 212.0000°F
cargo run -- 5 km mi        # 5km = 3.1069mi
cargo run -- 1 kg lb        # 1kg = 2.2046lb
cargo test
```

## Covers

Enums as data, `match` exhaustiveness, implementing `FromStr`, custom error
enum with `Display`, `std::env::args`, returning an exit code from `main`.

## Rewrite exercises

1. Rewrite it from scratch; keep temperature non-linear (via Celsius as a hub).
2. Add a new kind (volume: l, gal, ml) — the compiler should walk you through
   every `match` that needs updating.
3. Make precision a flag: `tempconv 1 kg lb -p 2`.
4. Accept `100c` as a single token (split number and unit while parsing).
5. Add an interactive mode when no args are given.
