# jsonfmt — JSON formatter and inspector

Pretty-print or minify JSON; `--path user.pets[1].name` extracts a value
(bare strings print unquoted, like `jq -r`); invalid JSON reports
line and column.

```sh
cargo run -- data.json
echo '{"a":[1,2,3]}' | cargo run -- --path a[2]
echo '{"a":1}' | cargo run -- --minify
cargo test
```

## Covers

Untyped JSON (`serde_json::Value`) vs typed structs, matching on `Value`
variants, a tiny path parser, precise error messages that name where the
walk failed, `serde_json` error positions.

## Rewrite exercises

1. Rewrite from scratch; the path parser is the fiddly part — tests first.
2. Write your own pretty-printer over `Value` (recursive, with indent) —
   don't call `to_string_pretty`.
3. Add `--keys`: list all paths in the document, one per line.
4. Support `--path 'users[*].name'` returning an array of matches.
5. Add `--sort-keys` and `--compact-arrays` (arrays of scalars on one line) —
   now your hand-rolled printer earns its keep.
