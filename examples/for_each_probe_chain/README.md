# `for_each` over a DAG-dependent probe chain (COOK-64 / CS-0091)

A realistic, runnable example of Cook's **data-driven fan-out** where the
`for_each` source is itself the tip of a **probe dependency chain**. Render one
config file per microservice from a JSON manifest, with a derived `url` field
computed by an intermediate probe.

Where [`for_each_benchmarks/`](../for_each_benchmarks/) covers the full matrix
of `for_each` *source forms* (probe, `probe:field`, `$(cmd)`, `as lines`) across
`cook` / `plate` / `test`, this example is narrower and deeper: it exercises the
§22.5.9 **register pre-pass resolving a probe that `requires` another probe**
before the recipe fans out.

## The chain

```
data/services.json
        │  (file input)
        ▼
  probe "services_raw"          reads the manifest → array of records
        │  (inputs.requires)
        ▼
  probe "services"              enriches each record with a derived `url`
        │  (for_each source)
        ▼
  recipe "render"               one cook unit per service → build/<name>.conf
```

`services` never touches the filesystem; it reads its upstream purely through
`cook.cache.get("services_raw")`. The register pre-pass (§22.5.9) must therefore
evaluate `services_raw` **first**, stash its value, then evaluate `services`,
then size the fan-out from the array `services` returns — all before any recipe
body runs.

## Run it

```bash
cook render            # fans out one config per service
./verify.sh            # asserts codegen shape + execution + per-member cache
```

## What it demonstrates

- **Data-driven fan-out.** Three records in `data/services.json` → three
  `cook` units → three `build/*.conf` files. Add a service to the manifest and
  a fourth unit appears on the next run; the DAG is sized at register time, not
  grown dynamically (§22.5.9).

- **DAG-dependent probe pre-pass.** `services` is evaluated only after its
  `requires` upstream `services_raw`. The upstream's fingerprint folds into the
  dependent probe's fingerprint (§22.5.3), so editing the manifest invalidates
  the whole chain coherently.

- **Per-member cache (§17.1 observable #5).** Edit one service's field in the
  manifest and re-run: only that service's config rebuilds (`2/3 cached`). The
  probe chain re-evaluates the full member set, but each member's unit carries
  its own materialised-element fingerprint, so the unchanged services stay cache
  hits.

## Files

| Path | Role |
|---|---|
| `Cookfile` | the two-layer probe chain + the `for_each` recipe |
| `data/services.json` | the manifest that drives the fan-out |
| `verify.sh` | codegen-shape + execution + per-member-cache assertions |
| `build/` | generated configs (git-ignored; produced by `cook render`) |
