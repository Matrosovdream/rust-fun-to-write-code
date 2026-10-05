# hashr — parallel file hasher

SHA-256 of every file under a directory using N worker threads; find
duplicate files; diff two directory trees by content.

```sh
cargo run -- hash src -j 8
cargo run -- dup ~/Downloads
cargo run -- diff backup/v1 backup/v2
cargo test
```

## Covers

The worker-pool shape: a paths channel fanned out to N threads (receiver
shared via `Arc<Mutex<_>>`), results fanned back in over a second channel,
channel closure as the shutdown signal, `Send` in practice, streaming
hashing with a fixed buffer, `sha2`.

## Rewrite exercises

1. Rewrite from scratch; draw the two channels on paper first.
2. Why must `result_tx` and `path_tx` be dropped where they are? Remove
   each drop and explain the hang.
3. Pre-filter `dup` by file size before hashing — huge speedup, why?
4. Add a progress line (`hashed 420/1000`) updated from the collector.
5. Swap the hand-rolled pool for `rayon`'s `par_iter` — compare line count,
   then benchmark both on a large directory.
