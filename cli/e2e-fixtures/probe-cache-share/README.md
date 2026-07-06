# probe-cache-share

Demonstrates cross-invocation cache hit. Probe sleeps 1s then records a
timestamp. First `cook build` ~1100ms (probe runs). Second ~200ms (cache
hit). Both record the SAME timestamp = cache hit confirmed.

> **Note (CS-0074):** This example requires the probe-execution wiring to be
> complete. Currently, `cook.probe()` does not emit `WorkPayload::Probe` work
> nodes, so the probe never executes. The example is correct as written; it
> will work end-to-end once the DAG wiring gap is closed.

Run: `bash verify.sh`.
