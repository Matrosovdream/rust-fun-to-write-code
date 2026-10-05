# rpn — reverse polish notation calculator

Evaluates postfix expressions from args or a REPL; `x = 3 4 +` assigns
variables usable in later expressions.

```sh
cargo run -- '3 4 + 2 *'    # 14
cargo run                    # REPL
cargo test
```

## Covers

`Vec` as a stack, `pop()` returning `Option`, converting `Option` to
`Result` with `ok_or`, `HashMap` for variables, matching on
`stack.as_slice()`, a line-based REPL.

## Rewrite exercises

1. Rewrite from scratch; the whole evaluator is one loop over tokens.
2. Add stack-manipulation words: `dup`, `swap`, `drop` (hello, Forth).
3. Add unary functions: `9 sqrt`, `0.5 sin`.
4. Convert infix to RPN with the shunting-yard algorithm (`--infix` flag) —
   reuse the evaluator untouched.
5. Keep the REPL history and add `vars` command listing all bindings sorted.
