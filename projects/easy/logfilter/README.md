# logfilter — log file analyzer

Parses an nginx/Apache "combined" access log and reports totals, status
distribution, top URLs, and a requests-per-hour histogram. Malformed lines
are counted and skipped. A sample `access.log` is included.

```sh
cargo run -- access.log
cargo run -- access.log --status 404
cargo run -- access.log --since 2026-09-21 --until 2026-09-22
cargo test
```

## Covers

A regex with capture groups behind `OnceLock`, `chrono` timestamp parsing
(`%d/%b/%Y:%H:%M:%S %z`) and date comparison, `filter_map` pipelines,
a generic `tally` helper over anything hashable, `NaiveDate` as a clap arg.

## Rewrite exercises

1. Rewrite from scratch; write `parse_line` + its tests before any reporting.
2. Add `--ip` filtering and a "top IPs" section.
3. Detect scanners: IPs whose 404 share is above 50% with ≥3 requests.
4. Replace the regex with a hand-written parser (split carefully around
   brackets and quotes) — compare speed on a million-line file.
5. Stream two logs merged by timestamp (like `sort -m`) using two
   `Peekable` iterators.
