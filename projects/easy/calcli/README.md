# calcli — command-line calculator

Evaluates arithmetic expressions with precedence, parentheses, unary minus,
and `%`. Errors (bad tokens, division by zero) are values, not panics.

```sh
cargo run -- '2 + 3 * (4 - 1)'   # 11
cargo run -- 10 % 3              # 1
cargo test
```

## Covers

Tokenizer with `Peekable<Chars>`, recursive descent parsing, custom error enum
implementing `Display` + `Error`, the `?` operator.

## Rewrite exercises

1. Rewrite from scratch — tokenizer first, parser second.
2. Add `^` (power, right-associative) — one new precedence level.
3. Add functions: `sqrt(2)`, `abs(-3)`.
4. Print the parse as a tree (`2 + 3 * 4` → an AST drawing) with `--ast`.
5. Replace recursive descent with the shunting-yard algorithm and compare.
