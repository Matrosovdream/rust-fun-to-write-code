# Plan — 30 easy Rust projects

The goal: cover the core of Rust through small, complete programs that are fun to
rewrite by hand. Projects live in `projects/easy/<name>` and are ordered as a
progression — each one leans on what the previous ones taught. Groups A–D go from
pure syntax to threads and networking.

## Coverage map

| Area | Where it's covered |
| --- | --- |
| Variables, loops, `match` | guessr, tempconv, fizzlab |
| Enums & pattern matching | tempconv, calcli, rockpaper, tictactoe |
| Ownership & borrowing | wallet, grepr, rpn |
| Structs, methods, `impl` | rockpaper, wallet, tictactoe, shapes |
| Traits & trait objects | shapes, fibgen, orgtree |
| Generics | shapes, csvstat |
| Lifetimes (basics) | grepr, anagram |
| `Option` / `Result` / `?` | calcli, romannum, catr, and everything after |
| Custom error types | calcli, todocli, kvstore |
| Iterators & closures | wordcount, fibgen, rpn, logfilter |
| Collections (`Vec`, `HashMap`, `HashSet`) | hangman, anagram, kvstore, logfilter |
| Strings, `char` vs bytes | caesar, romannum, headr |
| `Box` & recursion | orgtree |
| File I/O, `BufReader` | wordcount, catr, uniqr, logfilter |
| Paths & filesystem walking | findr |
| CLI args (`std::env`, `clap`) | tempconv (std), catr/headr/findr (clap) |
| serde & data formats | todocli, kvstore, jsonfmt, csvstat, weather |
| Time & `Duration` | pomodoro, logfilter |
| Threads, channels, `Arc<Mutex>` | hashr, echoserver, pomodoro |
| Networking (TCP, HTTP client) | echoserver, weather |
| External crates (`rand`, `regex`, `chrono`, `reqwest`…) | throughout group C–D |
| Testing (`cargo test`, `assert_cmd`) | every project |

---

## Group A — syntax, enums, match (`projects/easy/`)

### 1. `guessr` — number guessing game

The classic warm-up: the program picks a number, you guess, it says higher/lower,
counts attempts, offers a rematch.

*   **Patterns:** game loop, early `continue` on bad input.
*   **Stdlib:** `std::io` (stdin), `std::cmp::Ordering`.
*   **Crates:** `rand`.
*   **Testing:** unit tests for the guess-checking function.
*   **Teaches:** `let`/`mut`, shadowing, `loop`/`break`, `match`, `parse::<u32>()`,
    handling `Result` for the first time.

### 2. `tempconv` — unit converter CLI

Converts temperature, length, and weight: `tempconv 100 c f` → `212°F`. Units are
an enum parsed from the arguments.

*   **Patterns:** enum + `FromStr` for parsing, exhaustive `match` for conversion.
*   **Stdlib:** `std::env::args`, `std::str::FromStr`.
*   **Crates:** none (std only).
*   **Testing:** table of conversion cases.
*   **Teaches:** enums as data, `match` exhaustiveness, implementing a std trait,
    returning `Result` from `main`.

### 3. `calcli` — command-line calculator

Evaluates `calcli 2 + 3 '*' 4` with operator precedence; division by zero and bad
tokens produce proper errors, not panics.

*   **Patterns:** token enum, two-pass evaluation (or tiny recursive descent),
    custom error enum with `Display`.
*   **Stdlib:** `std::fmt`, `std::error::Error`.
*   **Crates:** none.
*   **Testing:** happy-path expressions + every error variant.
*   **Teaches:** custom error types, the `?` operator, `impl Display`, why Rust
    pushes you to make failure explicit.

### 4. `fizzlab` — fizzbuzz with configurable rules

FizzBuzz where rules (`3 → Fizz`, `5 → Buzz`, …) come from the command line, so
the naive if-chain doesn't work.

*   **Patterns:** rules as `Vec<(u32, String)>`, building output by iteration.
*   **Stdlib:** `std::env`, iterators over ranges.
*   **Crates:** none.
*   **Testing:** default rules + custom rule sets.
*   **Teaches:** tuples, `Vec`, `String` building, first taste of iterator
    chains (`map`, `collect`).

### 5. `romannum` — roman numerals converter

Converts both ways: `XIV → 14`, `2024 → MMXXIV`, with validation of malformed
numerals.

*   **Patterns:** lookup table as `const` slice, greedy subtraction algorithm.
*   **Stdlib:** `char` methods, slices, `windows(2)`.
*   **Crates:** none.
*   **Testing:** round-trip property (`to_roman(from_roman(x)) == x`) over a range.
*   **Teaches:** slices, `const`, string iteration with `chars()`, writing a
    round-trip test.

### 6. `caesar` — cipher tool

Caesar and Vigenère ciphers: encrypt/decrypt stdin or a file, plus a brute-force
mode that prints all 26 shifts.

*   **Patterns:** transformation as a `char → char` function passed around.
*   **Stdlib:** `u8`/`char` conversions, `std::io::Read`.
*   **Crates:** none.
*   **Testing:** encrypt→decrypt round-trips, non-ASCII passthrough.
*   **Teaches:** bytes vs chars, closures as parameters (`impl Fn`), wrapping
    arithmetic.

### 7. `rockpaper` — rock-paper-scissors vs computer

Best-of-N match against a random opponent, with score tracking and a
`Move`/`Outcome` model.

*   **Patterns:** enums with methods (`Move::beats`), `match` on tuples.
*   **Stdlib:** `std::io`.
*   **Crates:** `rand`.
*   **Testing:** every `(Move, Move)` pair → expected `Outcome`.
*   **Teaches:** `impl` blocks on enums, matching on tuples, deriving
    `PartialEq`/`Debug`/`Clone`.

### 8. `tictactoe` — terminal tic-tac-toe

Two players on one keyboard, 3×3 board, win/draw detection, input validation,
replay.

*   **Patterns:** board as `[[Option<Player>; 3]; 3]`, game state enum.
*   **Stdlib:** arrays, `std::fmt::Display` for board rendering.
*   **Crates:** none.
*   **Testing:** win detection on rows/columns/diagonals, draw detection.
*   **Teaches:** fixed-size arrays, `Option` as "empty cell", separating game
    logic from I/O so it's testable.

---

## Group B — ownership, traits, iterators (`projects/easy/`)

### 9. `wordcount` — wc clone

Counts lines, words, chars, and bytes for one or more files or stdin; prints a
totals row like real `wc`.

*   **Patterns:** a `Counts` struct with `Add` implemented for totals.
*   **Stdlib:** `BufReader`, `std::ops::Add`, `split_whitespace`.
*   **Crates:** none.
*   **Testing:** fixture files, empty file, file without trailing newline.
*   **Teaches:** buffered I/O, operator overloading, chars vs bytes counting.

### 10. `grepr` — mini grep

The Rust-book classic, extended: search a pattern in files, flags for
case-insensitive, line numbers, and invert-match.

*   **Patterns:** `Config` struct built from args, search functions returning
    `Vec<&str>` borrowed from the file contents.
*   **Stdlib:** `std::env`, `std::fs`, string slicing.
*   **Crates:** none.
*   **Testing:** unit tests for search functions, case sensitivity, invert.
*   **Teaches:** explicit lifetimes in signatures, borrowing instead of cloning,
    `env::var` for configuration.

### 11. `fibgen` — sequence generators as iterators

A small library + CLI: Fibonacci, primes, and Collatz as types implementing
`Iterator`, composed with standard adapters (`take`, `filter`, `sum`).

*   **Patterns:** custom `Iterator` implementations, lazy evaluation.
*   **Stdlib:** `Iterator` trait, `std::iter`.
*   **Crates:** none.
*   **Testing:** known prefixes of each sequence, adapter combinations.
*   **Teaches:** implementing `Iterator`, associated types, why laziness lets you
    `take(10)` from an infinite sequence.

### 12. `shapes` — geometry calculator

Circle, rectangle, triangle; compute area/perimeter, read shapes from arguments,
print a sorted report. One trait, many types.

*   **Patterns:** `trait Shape`, both `Vec<Box<dyn Shape>>` and a generic
    function, to compare the two.
*   **Stdlib:** `std::fmt`, sorting with `sort_by`.
*   **Crates:** none.
*   **Testing:** area/perimeter per shape, sorting order.
*   **Teaches:** traits, trait objects vs generics with bounds, dynamic dispatch,
    when to use which.

### 13. `rpn` — reverse polish notation calculator

Evaluates `3 4 + 2 *` from args or interactively; supports variables (`x = 5`).

*   **Patterns:** `Vec` as a stack, token enum, fold over tokens.
*   **Stdlib:** `Vec` push/pop, `HashMap` for variables.
*   **Crates:** none.
*   **Testing:** expressions, stack underflow, unknown variable.
*   **Teaches:** stack discipline with ownership, `pop()` returning `Option`,
    combining `Option` and `Result` cleanly.

### 14. `anagram` — word-play toolkit

Given a dictionary file: find anagrams of a word, check palindromes, find the
longest word spellable from given letters.

*   **Patterns:** normalized key (sorted chars) → `HashMap<String, Vec<String>>`.
*   **Stdlib:** `HashMap`, `HashSet`, sorting, `entry()` API.
*   **Crates:** none.
*   **Testing:** small fixture dictionary with known groups.
*   **Teaches:** the `entry` API, grouping, borrowing strings vs owning them,
    `&str` vs `String` decisions.

### 15. `wallet` — in-memory account ledger

REPL: create accounts, deposit, withdraw, transfer, print history. Transfers must
fail cleanly on insufficient funds — money is stored in cents.

*   **Patterns:** `Ledger` owning accounts, methods taking `&mut self`, operations
    log as `Vec<Transaction>`.
*   **Stdlib:** `HashMap`, integer money arithmetic.
*   **Crates:** none.
*   **Testing:** transfer success/failure, balance invariants after a sequence.
*   **Teaches:** the borrow checker in anger (two accounts from one map),
    `checked_sub`, designing APIs that can't represent invalid states.

### 16. `orgtree` — tree structures with Box

Build an org chart / file-tree from an indented text file, print it with
box-drawing characters, compute depth and headcount per node.

*   **Patterns:** `struct Node { children: Vec<Box<Node>> }` (or `Vec<Node>` — and
    understand why both work), recursive traversal.
*   **Stdlib:** recursion, `std::fmt::Display`.
*   **Crates:** none.
*   **Testing:** parse→render round-trip, depth/count on a fixture tree.
*   **Teaches:** recursive types, why `Box` exists, ownership in tree
    manipulation.

---

## Group C — files, data formats, real CLIs (`projects/easy/`)

### 17. `catr` — cat clone

Prints files with optional line numbers (`-n`) and number-nonblank (`-b`);
`-` means stdin. First project with a real argument parser.

*   **Patterns:** `clap` derive API, one `run(config)` function, errors to stderr
    with proper exit codes.
*   **Stdlib:** `BufReader`, `std::process::exit`.
*   **Crates:** `clap`, `anyhow`.
*   **Testing:** `assert_cmd` integration tests against fixture files.
*   **Teaches:** `clap` derive, `anyhow` for application errors, testing a binary
    end to end.

### 18. `headr` — head clone

First N lines (`-n`) or first N bytes (`-c`) of each file; multiple files get
`==> name <==` headers like real `head`.

*   **Patterns:** mutually exclusive flags, reading exactly N bytes.
*   **Stdlib:** `Read::take`, `BufRead::read_line`.
*   **Crates:** `clap`, `anyhow`.
*   **Testing:** `assert_cmd`: lines mode, bytes mode splitting a UTF-8 char.
*   **Teaches:** bytes vs lines for real, lossy UTF-8 output, flag validation.

### 19. `uniqr` — uniq clone

Collapses adjacent duplicate lines, `-c` prefixes counts, reads file or stdin,
writes file or stdout.

*   **Patterns:** streaming with one line of lookbehind, generic `Write` output.
*   **Stdlib:** `BufRead`, `Box<dyn Write>`.
*   **Crates:** `clap`, `anyhow`.
*   **Testing:** `assert_cmd` with fixture pairs (input → expected).
*   **Teaches:** writing to an abstract `Write`, streaming without loading the
    file, trait objects in practice.

### 20. `todocli` — todo manager with JSON storage

Add, list, done, remove; tasks persist in `~/.todocli.json`; `list` supports
filtering by status and sorting by date.

*   **Patterns:** repository struct wrapping the storage file, serde
    round-tripping, atomic save (write temp + rename).
*   **Stdlib:** `std::fs`, `std::path::PathBuf`.
*   **Crates:** `serde`, `serde_json`, `clap`, `chrono`.
*   **Testing:** repository tests against a temp dir (`tempfile`).
*   **Teaches:** `#[derive(Serialize, Deserialize)]`, designing structs for
    persistence, subcommands in `clap`.

### 21. `kvstore` — key-value store REPL

`SET`/`GET`/`DEL`/`KEYS`/`SAVE` commands in a REPL; data persists to disk on exit
and loads on start.

*   **Patterns:** command enum + `FromStr` parser, store struct with
    load/save, custom error type with `thiserror`.
*   **Stdlib:** `HashMap`, `std::io` REPL loop.
*   **Crates:** `serde_json`, `thiserror`.
*   **Testing:** command parsing table, store round-trip in a temp dir.
*   **Teaches:** `thiserror` vs `anyhow` (library vs app errors), parsing commands
    into types, REPL structure.

### 22. `csvstat` — CSV statistics

For each numeric column of a CSV: count, min, max, mean, median; non-numeric
columns get distinct-value counts.

*   **Patterns:** typed records vs dynamic `StringRecord`, a generic
    `Stats<T>` accumulator.
*   **Stdlib:** sorting for median, `Option` folding for min/max.
*   **Crates:** `csv`, `clap`.
*   **Testing:** fixture CSVs: clean, with gaps, with mixed types.
*   **Teaches:** the `csv` crate, generics with numeric bounds, handling missing
    data without panicking.

### 23. `jsonfmt` — JSON formatter and inspector

Pretty-print or minify JSON from stdin/file; `--path user.addresses[0].city`
extracts a value; invalid JSON reports line/column.

*   **Patterns:** working with untyped `serde_json::Value`, path parsing.
*   **Stdlib:** `std::io`.
*   **Crates:** `serde_json`, `clap`, `anyhow`.
*   **Testing:** format round-trips, path extraction, error positions.
*   **Teaches:** dynamic JSON (`Value`) vs typed structs, `match` on `Value`
    variants, recursive printing.

### 24. `findr` — find clone

Walk directories; filter by name (regex), type (file/dir/link), size, and
modification age; print matches.

*   **Patterns:** composing filters as a `Vec` of predicates (closures).
*   **Stdlib:** `std::fs::Metadata`, `SystemTime`.
*   **Crates:** `walkdir`, `regex`, `clap`.
*   **Testing:** `assert_cmd` against a fixture directory tree.
*   **Teaches:** `walkdir`, `regex`, boxed closures as filters, `PathBuf`/`Path`
    manipulation.

### 25. `logfilter` — log file analyzer

Parse an nginx-style access log: filter by status/date range, then report top
URLs, status distribution, and requests per hour.

*   **Patterns:** parse line → `Option<Entry>` (skip garbage), iterator pipeline
    from file to report, grouping with `HashMap`.
*   **Stdlib:** `BufReader`, iterator adapters (`filter_map`, `fold`).
*   **Crates:** `chrono`, `regex`.
*   **Testing:** fixture log with known totals, malformed-line handling.
*   **Teaches:** `filter_map`, `chrono` parsing/comparison, turning a shell
    one-liner habit into typed Rust.

---

## Group D — time, threads, network (`projects/easy/`)

### 26. `pomodoro` — terminal timer

Work/break pomodoro cycles with a live countdown redrawn in place; Ctrl-C pauses
and asks to resume or quit; session stats at the end.

*   **Patterns:** timer loop with `sleep`, drawing with `\r`, a channel to deliver
    the Ctrl-C signal.
*   **Stdlib:** `std::time::{Duration, Instant}`, `std::thread::sleep`, `mpsc`.
*   **Crates:** `ctrlc`.
*   **Testing:** unit tests for the schedule/state machine (not the sleeping).
*   **Teaches:** `Duration`/`Instant`, first channel, separating a pure state
    machine from wall-clock code so it's testable.

### 27. `hashr` — parallel file hasher

Compute SHA-256 of every file in a directory using N worker threads; compare two
directories by hash to find duplicates and diffs.

*   **Patterns:** worker pool over an `mpsc` channel of paths, results collected
    via a second channel.
*   **Stdlib:** `std::thread`, `std::sync::mpsc`, `Arc`.
*   **Crates:** `sha2`, `walkdir`, `clap`.
*   **Testing:** known-hash fixtures; same result with 1 and 8 workers.
*   **Teaches:** spawning threads, channels, `Arc`, `Send` in practice, why the
    compiler stops your data races.

### 28. `echoserver` — TCP echo/chat server

TCP server: echoes lines back, with a `/time` and `/quit` command; thread per
connection, a shared `Arc<Mutex<_>>` counter of connections; comes with a tiny
client binary.

*   **Patterns:** accept loop + thread per connection, shared state behind
    `Arc<Mutex>`, two binaries in one crate.
*   **Stdlib:** `std::net::{TcpListener, TcpStream}`, `std::sync::{Arc, Mutex}`.
*   **Crates:** none.
*   **Testing:** integration test connecting over localhost.
*   **Teaches:** TCP basics, `Mutex` guards, graceful connection shutdown,
    multiple `[[bin]]` targets.

### 29. `weather` — HTTP API client

Fetch current weather for a city from a public API (Open-Meteo, no key needed):
geocode the city, fetch the forecast, print a formatted report; `--json` for raw
output.

*   **Patterns:** typed response structs mirroring the API, two-step API flow,
    errors for network vs "city not found".
*   **Stdlib:** —
*   **Crates:** `reqwest` (blocking), `serde`, `clap`, `anyhow`.
*   **Testing:** deserialization tests from canned JSON responses.
*   **Teaches:** HTTP client, deserializing real-world JSON (optional fields,
    renames), wrapping third-party errors with context.

### 30. `lifegame` — Conway's Game of Life (capstone)

Terminal Game of Life: random or file-loaded start, generations animated in
place, pause/step/speed controls, detects when the board stabilizes.

*   **Patterns:** double-buffered grid, `Display` rendering, main loop with input
    polling; pulls together structs, iterators, I/O, and timing.
*   **Stdlib:** `std::time`, terminal output.
*   **Crates:** `crossterm` (or ANSI codes by hand), `rand`.
*   **Testing:** step function on known patterns (blinker, glider), stabilization
    detection.
*   **Teaches:** nothing new — that's the point. A rewrite you can do start to
    finish with everything from projects 1–29.
