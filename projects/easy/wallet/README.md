# wallet — in-memory account ledger

REPL: create accounts, deposit, withdraw, transfer, balances, history.
Money is `i64` cents; overdrafts and half-finished transfers are
unrepresentable.

```sh
cargo run
# > new alice
# > dep alice 100.50
# > xfer alice bob 25
cargo test
```

## Covers

The borrow checker in anger (you can't take two `&mut` into one `HashMap` —
validate first, mutate one at a time), `checked_sub`, integer money, a
transaction log as `Vec<enum>`, invariant-style tests (total is conserved).

## Rewrite exercises

1. Rewrite from scratch; hit the two-`&mut` wall yourself, then solve it.
2. Solve the transfer again with `HashMap::get_disjoint_mut` — compare.
3. Add `undo`: reverse the last transaction (what can't be undone, and why?).
4. Persist the *history* (not balances) to a file and rebuild state by
   replaying it on start — congratulations, event sourcing.
5. Add interest: a `tick` command applying 1% to every account, without
   cloning the map.
