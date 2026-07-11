# 05 — data-driven fan-out

The build's shape comes from **data**: a JSON manifest lists services, a
probe chain turns records into an iteration source, and the recipe fans
out one cached unit per record.

```
probe services_raw
    ingredients "data/services.json"     # file input → fingerprints the probe
    json { cat data/services.json }

probe services: services_raw             # enrich records in Lua
    >{ ... cook.probes.get("services_raw") ... }

recipe render
    ingredients services                 # fan out: one unit per record
    cook "build/$<in.name>.conf" { ... $<in.url> ... }
```

## The demo

```
$ cook render        # auth, billing, search
$ cook render        # all cached
$ $EDITOR data/services.json      # add a fourth service
$ cook render        # ONLY the new service builds
```

Each record gets its own cache entry, keyed on the member — the DAG is
sized at register time from the probe's value, so growing the data grows
the build without invalidating what's done. This is the shape for eval
suites over test cases, renders over scenes, LLM calls over prompts —
any "N inputs, same treatment" where N lives in data.

## Per-member joins

A second recipe iterating the same source references the first one
per-member with `$<render[in]>` — "render's output for *this* member":

```
recipe summary: render
    ingredients services
    cook "build/summary/$<in.name>.txt" { cat $<render[in]> >> $<out> }
```

`cook why render` shows each member's key with its determinants —
manifest content, the probe chain, the command — attributed one by one.
