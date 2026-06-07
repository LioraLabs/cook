COOK-96: `$<recipe[]>` per-member output accessor inside an `ingredients <probe>` fan-out body (§8.10, CS-0098).

Three recipes (`render`, `tts`, `mux`) all iterate the same `scenes` probe.  `mux` joins the other two recipes per member via `$<render[]>` and `$<tts[]>`.  The parser preserves all placeholder tokens verbatim; the per-member join semantics, the recipe-level DAG edges, and the per-member fingerprint fold are codegen/runtime concerns documented in §8.10.

The `ForEach source=ProbeKey("scenes")` desugar node is identical to the node produced by `ingredients scenes` in `positive/ingredients-probe` — confirming that `$<recipe[]>` placeholders do not alter the parse-level AST; the accessor is resolved entirely at the codegen layer.
