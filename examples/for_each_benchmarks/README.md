# §8.3 `for_each` Benchmarks (COOK-63 / CS-0091)

Concrete coverage of Cook's **data-driven fan-out**: the `for_each` step, the
data-member counterpart to `ingredients`. Where `ingredients` drives one work
unit per filesystem path, `for_each` drives one unit per **data member** — a
record or scalar — with the current member bound as `item`.

## Surface forms

A `for_each` step names exactly one **source** and an optional `as lines`
modifier (Cook Standard §8.3):

| Source | Meaning | Member typing |
|---|---|---|
| `for_each <probe>` | An array-shaped probe value (§22.5.9) | each array element is a record/scalar |
| `for_each <probe>:<field>` | The array at the probe value's named field | each element of that array |
| `for_each $(cmd)` | Register-time shell capture; stdout split on newlines | each line JSON-decoded into a record/scalar |
| `for_each $(cmd) as lines` | Same capture, JSON parsing disabled | each line is a raw string |
| `for_each (lua-expr)` | **Reserved** — parses, then rejected | — |

The current member is available as:

- the Lua name `item` in block bodies and Lua-expression outputs;
- `$<item>` — the whole member (canonical key-sorted JSON for a record, the
  scalar's string form otherwise);
- `$<item.FIELD>` — the value of record field `FIELD`.

`cook`, `plate`, and `test` steps each produce **one unit per member**.

## The recipes

| Recipe | Source form | Consumer | Units (this data) |
|---|---|---|---|
| `cards_cook` | probe `cards` | `cook` | 2 (one per card) |
| `catalog_cook` | probe `catalog:items` (key:field) | `cook` | 2 |
| `deploy` | `$(cat data/hosts.ndjson)` (JSON lines) | `plate` | 2 (one per host) |
| `render` | `$(ls posts) as lines` (raw lines) | `cook` | 2 (one per post) |
| `eval` | probe `cases` | `test` | 2 (one per case) |

Sources live in `data/` (probe-backed `*.json`, the `hosts.ndjson` capture) and
`posts/` (the `as lines` capture). Commands are POSIX-portable on purpose.

## Verifying

**COOK-63 lands the parser + codegen.** The register-time runtime these recipes
need — the §22.5.9 probe pre-pass that materialises an array probe value
*before* registration, the register-time `$(cmd)` capture, and the whole-member
`cook.member_to_string` rendering — is the **COOK-64** slice. Until then, verify
at the level COOK-63 implements, with the transpiler:

```sh
cook emit-lua     # print the generated register-phase fan-out Lua
./verify.sh       # assert the codegen shape of every recipe (uses emit-lua)
```

`verify.sh` confirms each recipe lowers to the expected `for _, item in
ipairs(_items)` fan-out: the right member source (`cook.cache.get` / `:field`
index / `cook.sh` split with-or-without `json_decode`), `$<item.FIELD>` →
`tostring(item["FIELD"])`, bare `$<item>` → `cook.member_to_string(item)`, and
one `cook.add_unit` / `cook.add_test` per member.

## Once COOK-64 lands

The recipes run end-to-end as written:

```sh
cook cards_cook      # writes build/cards/{ace,king}.txt
cook deploy          # echoes one deploy line per host
cook eval            # runs one test per case
cook clean           # wipe build/
```

At that point `verify.sh` gains an execution tier (assert the produced outputs
and the per-member cache behaviour: editing one member's record re-runs only its
unit). The codegen assertions stay as the parser/codegen regression guard.
