# `cook (LUA_EXPR)` — Lua-expression output paths (CS-0089 / §8.4.2)

A runnable example of the **Lua-expression output form**: the `cook` step's
output slot holds a parenthesised Lua expression instead of a quoted pattern,
evaluated **once per ingredient at register time** with `input` bound to that
ingredient's path.

## Why a template isn't enough

The quoted pattern is a *template*: `cook "build/$<in.stem>.o"` can re-prefix
and re-suffix, but `$<in.stem>` strips the directory — every output lands flat
in one directory. When the output is a *rewrite* of the input path (keep the
subtree, swap a prefix, swap an extension), the template vocabulary can't say
it. The expression form can:

```cook
recipe translate
    ingredients "docs/en/**/*.md"
    cook (input:gsub("docs/en", "build/fr")) {
        sed 's/Hello/Bonjour/g' $<in> > $<out>
    }
```

`docs/en/guide/intro.md` → `build/fr/guide/intro.md` — the `guide/` subtree
survives because the whole path is rewritten, not templated from the stem.

## The two recipes

| Recipe | Body | Demonstrates |
|---|---|---|
| `translate` | `{ … }` shell block | prefix rewrite preserving the nested subtree; `$<in>` / `$<out>` resolve to the iteration's ingredient path and the expression's evaluated string (§8.4.2 rule 6) |
| `index` | `>{ … }` Lua block | chained `gsub` rewrites on one line (prefix swap + `.md` → `.json`); the body sees the §23.1 `input` / `output` bindings, where `output` is exactly the string the expression produced |

## Run it

```bash
cook translate         # one rewritten file per source, subtree preserved
cook index             # one .json sidecar per source
./verify.sh            # asserts codegen shape + execution + 1:1 cache
```

## What it demonstrates

- **Per-ingredient register-time evaluation.** The emitted Lua evaluates the
  expression inside the per-ingredient loop and guards the result (non-string
  or empty → the Note 8.4.2.3 register-phase diagnostic).

- **One-to-one mode only (§8.4.2 rule 4).** The expression form requires an
  element-by-element iteration source; here that's the `ingredients` glob.
  Each markdown file produces one unit whose output is the evaluated string.

- **Ordinary 1:1 caching.** The expression changes how the output *path* is
  computed, not how the unit is fingerprinted: edit one source file and only
  its unit rebuilds (`2/3 cached`), exactly as with a quoted pattern.

## Files

| Path | Role |
|---|---|
| `Cookfile` | the two expression-output recipes |
| `docs/en/**` | nested source tree that drives the fan-out |
| `verify.sh` | codegen-shape + execution + cache assertions |
| `build/` | generated outputs (git-ignored; produced by the recipes) |
