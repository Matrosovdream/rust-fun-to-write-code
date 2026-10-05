# kvstore — key-value store REPL

`SET`/`GET`/`DEL`/`KEYS`/`SAVE`/`QUIT` over a `HashMap`, persisted to a JSON
file on `SAVE` and on exit.

```sh
cargo run -- mydata.json
# > SET name rust
# > GET name
cargo test
```

## Covers

`thiserror` for typed library errors (`#[from]` conversions) vs `anyhow`,
parsing commands into an enum via `FromStr`, matching on tuples of
`(cmd, arg1, arg2)`, a dirty flag + save-on-exit, atomic file replace.

## Rewrite exercises

1. Rewrite from scratch; the command parser is the heart — write its tests
   first.
2. Add `INCR key` (error if the value isn't an integer) and `TTL key secs`
   with lazy expiry on read.
3. Replace JSON with an append-only log (`SET a 1` per line); compact on
   save. Congratulations, you've built a tiny Bitcask.
4. Add `--readonly` mode where mutations are refused at the type level
   (two store types, one trait).
5. Quote-aware parsing: `SET msg "hello world"` — write a small tokenizer.
