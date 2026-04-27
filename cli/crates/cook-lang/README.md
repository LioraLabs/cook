# cook-lang

The Cookfile parser: text in, AST out. The current reference implementation of the [Cook Standard](../../../standard/).

## Cook Standard claim

This crate claims **Cook Standard v0.2**.

The claim lives in `src/lib.rs`:

```rust
pub const COOK_STANDARD_VERSION: &str = "0.2";
```

To verify the claim, run the conformance harness:

```bash
cargo test -p cook-lang --test conformance
```

To verify backwards conformance against a previously-cut version:

```bash
standard/scripts/check-conformance-against-tag.sh v0.1
```

See `CONFORMANCE.md` for details and pending CSes.
