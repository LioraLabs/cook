# cc-find-conflicting-opts

A second `cc.find` call for the same library name with conflicting opts
MUST raise per Standard §28.3.14. This fixture documents the register-phase
diagnostic shape produced by `cook_cc/finder.lua`.

The parser-only conformance harness in `cli/crates/cook-lang/tests/conformance.rs`
SKIPS fixtures carrying only a `register_error.txt`; the companion
`cook-register/tests/conformance.rs` harness consumes the baseline.

The full diagnostic emitted by the module is:

```
[cc.find] duplicate cc.find for 'raylib' with conflicting opts:
  first call opts=<canonical-opts>
  this call opts=<canonical-opts>
```

We baseline only the stable prefix so the assertion remains robust to
internal changes in `canonical_opts` formatting.
