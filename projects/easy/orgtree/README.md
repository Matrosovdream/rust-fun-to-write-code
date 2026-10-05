# orgtree — tree structures with Box

Builds an org chart from an indented text file, renders it with box-drawing
characters, and computes headcount and depth per node.

```sh
cargo run -- team.txt
cat team.txt | cargo run
cargo test
```

## Covers

Recursive types and why `Box` exists (infinite size without indirection),
owned trees (`Vec<Box<Node>>`), recursive traversal for count/depth/find,
building strings in recursive rendering, `find_map`.

## Rewrite exercises

1. Rewrite from scratch; get `insert_at` right — it's the whole parser.
2. Swap `Vec<Box<Node>>` for `Vec<Node>` — what changes? Then make a
   `cons`-style linked list where `Box` is *mandatory*.
3. Add `prune <name>`: remove a subtree (ownership makes this pleasant).
4. Render to Markdown nested lists and to JSON (no serde — by hand).
5. Support tabs and 4-space indents by auto-detecting the indent unit.
