# tictactoe — terminal tic-tac-toe

Two players on one keyboard; cells are numbered 1-9 like a keypad; win/draw
detection and replay.

```sh
cargo run
cargo test
```

## Covers

Fixed-size arrays (`[[Option<Player>; 3]; 3]`), `Option` as "empty cell",
enums for state, `Display` for rendering, keeping game logic free of I/O so
it can be tested.

## Rewrite exercises

1. Rewrite from scratch; the win check should be a table of lines, not
   hand-rolled ifs.
2. Add an unbeatable bot with minimax (the board is tiny — brute force is fine).
3. Generalize to N×N with K in a row; arrays become `Vec`s — feel the difference.
4. Add undo: keep a move history and `u` to revert.
5. Highlight the winning line in the final render.
