# lifegame — Conway's Game of Life (capstone)

Terminal Game of Life: random soup or a `.cells` pattern file, animated in
an alternate screen, with pause/step/speed controls and stabilization
detection (still lifes and oscillators are recognized by period).

```sh
cargo run
cargo run -- glider.cells --width 40 --height 20
cargo run -- --density 0.15 --delay 50
cargo test
```

Controls: `space` pause/resume · `n` step · `+`/`-` speed · `r` randomize ·
`q` quit.

## Covers

Nothing new — that's the point. Double-buffered grid, toroidal neighbor
math, `Display` rendering, `Instant`-based timing, non-blocking key polling
(`crossterm`), an RAII guard restoring the terminal, hashing for cycle
detection. A full rewrite with everything from projects 1–29.

## Rewrite exercises

1. Rewrite from scratch — aim for one sitting.
2. Store the grid as `Vec<u8>` with bit-packing — measure memory and speed.
3. Only redraw cells that changed between generations.
4. Add RLE pattern format support (`.rle` files from conwaylife.com).
5. Make the board infinite: a `HashSet<(i64, i64)>` of live cells — which
   parts of the code survive the change untouched?
