# 03 — chores and config

Two ideas in one small Cookfile.

## Config: declared knobs that key the cache

```
config
    env.MODE = os.getenv("MODE") or "debug"

config release
    env.MODE = "release"
```

Config blocks are Lua. Only `env.*` declarations are visible to `$<...>`
placeholders — and every env var a step consults is folded into that step's
cache key. That buys the flow that makes this example worth running:

```
$ cook                    # builds out/hello.c in debug mode
$ cook build @release     # preset overlay → rebuilds in release mode
$ cook                    # back to debug — CACHE HIT, correct debug bytes
```

Same output path, two cache entries. Switching configurations back and
forth costs nothing; you never rebuild a mode you've built before.
Overrides, most specific wins: `--set MODE=tiny` > `@release` preset >
`MODE=big cook` environment > the config default.

## Chores: uncached verbs

```
chore greet who="world"
    echo "hello, $<who>"
```

`cook greet`, `cook greet ada`. Chores run every time (no caching), take
positional parameters with defaults, and are the home for `clean`,
`deploy`, `release` — anything that's an action, not an artifact.
Forget a required parameter and cook tells you exactly what's missing.
