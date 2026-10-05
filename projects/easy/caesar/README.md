# caesar — cipher tool

Caesar and Vigenère ciphers over stdin or a file, plus a brute-force mode
printing all 26 shifts.

```sh
echo 'Attack at dawn' | cargo run -- enc 3
cargo run -- dec 3 secret.txt
echo 'Dwwdfn dw gdzq' | cargo run -- brute
echo 'ATTACKATDAWN' | cargo run -- venc LEMON
cargo test
```

## Covers

Bytes vs chars, `u8`/`char` conversions, closures as parameters
(`impl FnMut(char) -> char`), modulo arithmetic, reading stdin or a file
behind one function.

## Rewrite exercises

1. Rewrite from scratch — get `shift_char` right first, the rest follows.
2. Add ROT13 as a dedicated subcommand (it's its own inverse — test that).
3. Score brute-force candidates by English letter frequency and print the
   best guess first.
4. Add an Atbash cipher (A↔Z, B↔Y) — same `transform`, different closure.
5. Stream input chunk-by-chunk with `Read::read` instead of reading the whole
   input into a `String`.
