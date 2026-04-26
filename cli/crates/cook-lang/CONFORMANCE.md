# Conformance

This crate is the current reference implementation of the [Cook Standard](../../../standard/).

## Claim

`cook-lang` claims **Cook Standard v0.2**.

The claim is the constant `COOK_STANDARD_VERSION` in `src/lib.rs`. The constant is the single source of truth; the README and `cook --version` mirror it.

## Verifying the claim

The conformance harness walks `standard/conformance/` (relative to the workspace root) and asserts that every positive case parses into the expected AST shape and every negative case is rejected with the expected error substring.

```bash
cd cli && cargo test -p cook-lang --test conformance
```

## Backwards conformance

To verify that this parser still satisfies a previously-cut Standard version:

```bash
standard/scripts/check-conformance-against-tag.sh v0.1
```

The script materializes the corpus from the `cs-standard/v0.1` git tag into a temp directory and runs the harness against it. The corpus path is overridable via the `COOK_CONFORMANCE_CORPUS` environment variable.

## Pending CSes

CSes that this crate is in the middle of implementing — included here when the parser is mid-catch-up between cuts. The conformance harness output is authoritative; this list is a human summary.

None at this version.

## Bumping the claim

When `cook-lang` finishes catching up to a new cut, bump `COOK_STANDARD_VERSION` in the same commit that closes the last gap, mirror the new value in `cli/crates/cook-lang/README.md` and the project root `README.md`, and clear the corresponding entry from the **Pending CSes** list above.
