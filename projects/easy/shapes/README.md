# shapes — geometry calculator

Circle, rectangle, triangle behind one `Shape` trait; parses specs from the
command line and prints a report sorted by area.

```sh
cargo run -- circle:2 rect:3x4 tri:3,4,5
cargo test
```

## Covers

Traits with default methods, trait objects (`Vec<Box<dyn Shape>>`) vs
generics (`fn describe_generic<T: Shape>`), Heron's formula, `total_cmp`
for sorting floats, validating input at the boundary.

## Rewrite exercises

1. Rewrite from scratch; decide consciously where you need `dyn` and where
   generics do.
2. Add a `Square` that is *not* a special-cased `Rect` — then add
   `impl From<Square> for Rect` and discuss which design you prefer.
3. Add `scale(factor)` to the trait. Which shapes can share a default impl?
4. Implement `Display` for a `ShapeReport` struct instead of building strings.
5. Replace `Box<dyn Shape>` with an enum `AnyShape` and match — compare the
   trade-offs (open vs closed set of types).
