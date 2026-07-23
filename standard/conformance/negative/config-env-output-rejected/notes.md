# config-env-output-rejected

CS-0164. The config output namespace is `var` (`var.NAME = ...`), not `env`.
A config body that writes `env.NAME = ...` — the pre-CS-0164 form, and the shape
every make/just refugee reaches for — reads the now-absent `env` global, which
the config sandbox (§5.3.1, §5.3.2) traps with a load-time did-you-mean
diagnostic that points at `var.` rather than a cryptic nil-index traceback.

The rejection lives at register time (the sandbox `__index` fires when the
config dispatcher runs the body), not at parse or codegen time — a config body
is opaque `LUA_SOURCE`. The parser-only harness (`cook-lang/tests/conformance.rs`)
therefore SKIPS this fixture; the `cook-register/tests/conformance.rs` harness
drives the config dispatch and baselines the stable prefix of the diagnostic.
