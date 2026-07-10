# §8.3 `ingredients <probe>` Benchmarks (COOK-63 / CS-0091)

Concrete coverage of Cook's **data-driven fan-out**: the `ingredients <probe>`
form, the data-member counterpart to `ingredients "glob"`. Where `ingredients
"glob"` drives one work unit per filesystem path, `ingredients <probe>` drives
one unit per **data member** — a record or scalar — with the current member
bound as `item`.

## Surface forms

An `ingredients <probe>` line names exactly one **probe source** (Cook Standard
§8.3):

| Source | Meaning | Member typing |
|---|---|---|
| `ingredients <probe>` | An array-shaped probe value (§22.5.9) | each array element is a record/scalar |
| `ingredients <probe>:<field>` | The array at the probe value's named field | each element of that array |

The current member is available as:

- the Lua name `item` in block bodies and Lua-expression outputs;
- `$<in>` — the whole member (canonical key-sorted JSON for a record, the
  scalar's string form otherwise);
- `$<in.FIELD>` — the value of record field `FIELD`.

`cook` and `test` steps each produce **one unit per member**.

## The recipes

| Recipe | Source form | Consumer | Units (this data) |
|---|---|---|---|
| `cards_cook` | probe `cards` | `cook` | 2 (one per card) |
| `catalog_cook` | probe `catalog:items` (key:field) | `cook` | 2 |
| `eval` | probe `cases` | `test` | 2 (one per case) |

Sources live in `data/` (probe-backed `*.json`). Commands are POSIX-portable on
purpose.

## Verifying

**COOK-63 lands the parser + codegen.** The register-time runtime these recipes
need — the §22.5.9 probe pre-pass that materialises an array probe value
*before* registration and the whole-member `cook.member_to_string` rendering —
is the **COOK-64** slice. Until then, verify at the level COOK-63 implements,
with the transpiler:

```sh
cook emit-lua     # print the generated register-phase fan-out Lua
./verify.sh       # assert the codegen shape of every recipe (uses emit-lua)
```

`verify.sh` confirms each recipe lowers to the expected `for _, item in
ipairs(_items)` fan-out: the right member source (`cook.cache.get` / `:field`
index), `$<in.FIELD>` → `tostring(item["FIELD"])`, bare `$<in>` →
`cook.member_to_string(item)`, and one `cook.add_unit` / `cook.add_test` per
member.

## Once COOK-64 lands

The recipes run end-to-end as written:

```sh
cook cards_cook      # writes build/cards/{ace,king}.txt
cook eval            # runs one test per case
cook clean           # wipe build/
```

At that point `verify.sh` gains an execution tier (assert the produced outputs
and the per-member cache behaviour: editing one member's record re-runs only its
unit). The codegen assertions stay as the parser/codegen regression guard.
