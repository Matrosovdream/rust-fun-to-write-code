# findr — find clone

Walks directories and filters by name regex, type, size, and modification
age. Filters compose: every flag adds one predicate.

```sh
cargo run -- . --name '\.rs$'
cargo run -- src --type f --min-size 1024
cargo run -- . --newer-than 7
cargo test
```

## Covers

`walkdir`, `regex` as a clap value type, boxed closures as composable
filters (`Vec<Box<dyn Fn(&DirEntry) -> bool>>`), `fs::Metadata` and
`SystemTime`, continuing past unreadable directories.

## Rewrite exercises

1. Rewrite from scratch; start with the walk, add filters one at a time.
2. Add `--max-depth` and `--hidden` (skip dotfiles by default, like `fd`).
3. Add `--exec 'echo {}'` running a command per match.
4. Print sizes in a `--long` format (size, mtime, path) aligned in columns.
5. Parallelize with `jwalk` or a worker pool — measure on a big tree.
