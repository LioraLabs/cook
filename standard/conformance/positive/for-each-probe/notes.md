CS-0091 — `for_each` data-member iteration source (§8.3).

Pins the canonical §8.3 probe-key surface: a recipe whose iteration is
driven by a `for_each <probe-key>` step rather than by `ingredients`. The
member binds as `item`; the `cook` step interpolates `$<item.id>` into the
output path and `$<item.name>` into the shell body (COOK-63).

**`parse.txt` shape (informative).** The `for_each` step renders as

```
ForEach source=ProbeKey("cards") as_lines=false
```

via the `Step::ForEach` arm of `format_step` in
`cli/crates/cook-lang/tests/conformance.rs`. Codegen lowers this recipe to a
register-phase fan-out — one `cook.add_unit` per member — over the probe's
array value (§22.5.9); the demand-driven pre-pass and per-member fingerprint
fold are the COOK-64 runtime slice.
