# probe-basic

Minimal demo of `cook.probe`: a probe resolves the system C compiler once
per build, and a consumer unit uses its value via `{{demo:compiler.path}}`
template substitution.

> **Note (CS-0074):** This example requires the probe-execution wiring to be
> complete. Currently, `cook.probe()` does not emit a `WorkPayload::Probe`
> work node, so the probe never executes and the consumer sees an empty value.
> The example is correct as written; it will work end-to-end once the DAG
> wiring gap is closed (see `cli/crates/cook-cli/tests/probe_integration.rs`).

Run from this directory:

    cargo run --bin cook --manifest-path ../../cli/Cargo.toml -- build

First run executes the probe; second hits the cache. Result: `./build/demo`
binary that prints `hello from probe-basic`.
