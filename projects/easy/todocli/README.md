# todocli — todo manager with JSON storage

Add, list, complete, and remove tasks; everything persists to
`~/.todocli.json` (override with `TODOCLI_FILE`).

```sh
cargo run -- add buy milk
cargo run -- list
cargo run -- done 1
cargo run -- list --all
TODOCLI_FILE=/tmp/t.json cargo run -- add sandboxed
cargo test
```

## Covers

serde derive round-tripping, `chrono` timestamps in JSON, clap subcommands,
a repository struct that owns all filesystem concerns, atomic save
(temp file + rename), `tempfile` in tests.

## Rewrite exercises

1. Rewrite from scratch; keep commands free of `fs` calls.
2. Add `edit <id> <new title>` and `clear` (drop all completed).
3. Make ids never reuse: persist a `next_id` counter in the file.
4. Add due dates (`add "pay rent" --due 2026-11-01`) and sort/flag overdue
   tasks in `list`.
5. Support two storage backends (JSON and CSV) behind a `Storage` trait —
   the repository pattern for real.
