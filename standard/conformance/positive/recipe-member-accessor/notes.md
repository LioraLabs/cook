COOK-96: `$<recipe[in]>` per-member output accessor inside a `gather <probe>` fan-out body (§8.9, CS-0098; respelled from `$<recipe[]>` in v1.0, CS-0137).

Three recipes (`render`, `tts`, `mux`) all iterate the same `scenes` probe.  `mux` joins the other two recipes per member via `$<render[in]>` and `$<tts[in]>`.  The parser preserves all placeholder tokens verbatim; the per-member join semantics, the recipe-level DAG edges, and the per-member fingerprint fold are codegen/runtime concerns documented in §8.9.

The `MemberSource source=GatherKey("scenes")` desugar node is identical to the node produced by `gather scenes` in `positive/gather-probe` — confirming that `$<recipe[in]>` placeholders do not alter the parse-level AST; the accessor is resolved entirely at the codegen layer.
