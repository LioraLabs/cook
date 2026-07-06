# probe-chain

Demonstrates probe-depends-on-probe topology. `demo:cc-version` declares
`requires = {"demo:cc-path"}` — the engine wires a DAG edge so `cc-path`
runs first; `cc-version` reads its value via `cook.cache.get`.

> **Note (CS-0074):** This example requires the probe-execution wiring to be
> complete. Currently, `cook.probe()` does not emit `WorkPayload::Probe` work
> nodes, so neither probe executes. The example is correct as written; it will
> work end-to-end once the DAG wiring gap is closed.

Run: `cargo run --bin cook --manifest-path ../../cli/Cargo.toml -- build`.
Both probes execute on first invocation; both cached on second.
