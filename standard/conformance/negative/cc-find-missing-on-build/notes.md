# cc-find-missing-on-build

Demand-driven failure semantics per Standard §28.3.13 and §28.3.14: a
missing library MUST NOT fail at register time. The probe failure
manifests when the build is actually invoked.

Parse and register both succeed; only an execute-phase build attempt
surfaces the diagnostic. The expected execute-phase error is recorded
in `execute_error.txt`.

The parser-only conformance harness in `cli/crates/cook-lang/tests/conformance.rs`
SKIPS fixtures carrying only an `execute_error.txt` (the symmetry with
`codegen_error.txt` / `register_error.txt`). A future execute-phase
runner consumes the baseline.

The full diagnostic emitted by the module follows the §28.5 table for
`cc.find_or_error`:

```
could not locate '<name>'<version-suffix>:
  - <strategy>: <outcome> (<reason>)
  ...
<hints>
```

We baseline only the stable prefix so the assertion remains robust to
attempt-chain ordering and hint formatting.
