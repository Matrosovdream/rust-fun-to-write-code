# pomodoro — terminal timer

Work/break cycles with a live countdown redrawn in place. Ctrl-C pauses and
asks to resume or quit; quitting prints session stats.

```sh
cargo run
cargo run -- -w 50 -b 10 -c 2
cargo test        # note: the tests never sleep
```

## Covers

`Duration`, `thread::sleep`, the first `mpsc` channel (Ctrl-C handler →
timer loop), `try_recv` polling, `\r` redraw, and the big one: separating
the pure schedule/summary state machine from wall-clock code so tests
don't sleep.

## Rewrite exercises

1. Rewrite from scratch; keep every test sleep-free.
2. Add a long break (15 min) every 4th cycle.
3. Replace polling (`try_recv` + sleep 1s) with `recv_timeout` — what gets
   simpler?
4. Use `Instant` to correct drift: sleeping "1 second" 1500 times loses
   seconds — measure, then fix by computing remaining from elapsed time.
5. Ring the terminal bell (`\x07`) on transitions and add `--quiet`.
