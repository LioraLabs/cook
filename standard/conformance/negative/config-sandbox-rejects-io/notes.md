# config-sandbox-rejects-io

Companion to `config-sandbox-rejects-os`: a `config` body may not reach `io`
either (Standard §5.3.2, CS-0163). The pre-sandbox idiom for reading a file into
a config value — `io.open(path):read("*a")` — is a register-phase error that
points the author at `host.read(path)`.

Same harness split as the `os` case: the parser-only harness skips this
`register_error.txt`-only fixture; `cook-register/tests/conformance.rs`
baselines the diagnostic. We baseline only the stable prefix.
