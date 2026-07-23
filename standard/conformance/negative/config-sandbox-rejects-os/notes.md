# config-sandbox-rejects-os

A `config` body runs in a sandboxed Lua VM (Standard §5.3.2, CS-0163): `os`
(along with `io`, clocks, randomness, process spawning, and filesystem writes)
MUST NOT be reachable. Reaching for `os.getenv` — the pre-sandbox idiom for
reading an ambient environment variable — is a register-phase error that points
the author at `host.env(name, default)`.

The parser-only harness (`cook-lang/tests/conformance.rs`) SKIPS fixtures that
carry only a `register_error.txt`; the `cook-register/tests/conformance.rs`
harness drives the config dispatch and baselines the diagnostic. The base config
block always executes, so the rejection fires before any recipe body runs.

We baseline only the stable prefix of the diagnostic.
