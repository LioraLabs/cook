# 04 — probes

A **probe** is a named, memoized value producer that runs before the build.
Its value is a *determinant*: steps built from it re-key when it changes and
stay cached when it doesn't. Probes are lazy — an undemanded probe never runs.

Producer kinds:

```
probe greeting
    { echo "hello from cook" }          # one string, from shell

probe target_list
    ingredients "data/targets.txt"      # file input → fingerprints the probe
    lines { cat data/targets.txt }      # a list, one member per line

probe target_count: target_list         # probe depending on a probe
    >{ return tostring(#cook.probes.get("target_list")) }
```

(there is also `json { ... }` for structured data — example 05 fans out
over JSON records — plus `tools { ... }` / `envs { ... }` for fingerprinting
tool paths and environment variables, which example 10 uses as a host key.)

## The demo

```
$ cook targets            # one unit per member: dev, staging, prod
$ cook targets            # cached
$ echo "qa" >> data/targets.txt
$ cook targets            # ONE new unit runs; dev/staging/prod stay cached
```

`ingredients target_list` makes the probe an iteration source: the member
set is resolved at register time, the DAG is sized from it, and each member
gets its own cache entry. Data grows → only new work runs.

`cook emit-lua` shows what the probe DSL lowers to if you want to see the
machinery.
