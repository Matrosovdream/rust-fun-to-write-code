# fibgen — sequence generators as iterators

Fibonacci, primes, and Collatz implemented as types with `impl Iterator`,
composed with standard adapters.

```sh
cargo run -- fib 10
cargo run -- primes 10
cargo run -- collatz 27
cargo run -- fibsum 10
cargo test
```

## Covers

Implementing `Iterator`, associated types (`type Item`), lazy infinite
sequences, `take`/`filter`/`take_while`/`sum` on your own types,
`checked_add` to end instead of overflow.

## Rewrite exercises

1. Rewrite from scratch; start with `Fib` and get `take(10)` working.
2. Add a `Squares` iterator and solve: sum of squares below 1000 not
   divisible by 3 — in one iterator chain.
3. Replace `Fib`'s struct with `std::iter::successors` — compare.
4. Make `Primes` a proper sieve that grows on demand.
5. Implement `DoubleEndedIterator` for a bounded range of Fibonacci — what
   has to change?
