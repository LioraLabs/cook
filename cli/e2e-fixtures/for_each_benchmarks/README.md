# §8.2 `gather <probe>` Benchmarks (COOK-63 / CS-0091)

Concrete coverage of Cook's **data-driven fan-out**: the `gather <probe>`
form supplies a recipe with data members — records or scalars. In these
recipes, accessor-bearing `cook` outputs and item-referencing `test` bodies
fan out with the current member bound as `item`.

## Surface forms

The benchmark's `gather <probe>` lines name exactly one **probe source** (Cook
Standard §8.2):

| Source | Meaning | Member typing |
|---|---|---|
| `gather <probe>` | An array-shaped probe value (§22.5.10) | each array element is a record/scalar |
| `gather <probe>:<field>` | The array at the probe value's named field | each element of that array |

The current member is available as:

- the Lua name `item` in block bodies and Lua-expression outputs;
- `$<in>` — the whole member (canonical key-sorted JSON for a record, the
  scalar's string form otherwise);
- `$<in.FIELD>` — the value of record field `FIELD`.

The gather source supplies the recipe's members. An accessor-bearing `cook`
output registers one unit per member; a later all-literal `cook` output
registers one aggregate unit over the preceding outputs; an all-literal first
`cook` step is rejected. Every consumer in this benchmark deliberately selects
the per-member form (`test` selects it by referencing the item in its body).

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
need — the §22.5.10 probe pre-pass that materialises an array probe value
*before* registration and the whole-member `cook.member_to_string` rendering —
is the **COOK-64** slice. Until then, verify at the level COOK-63 implements,
with the transpiler:

```sh
cook emit-lua     # print the generated register-phase fan-out Lua
./verify.sh       # assert the codegen shape of every recipe (uses emit-lua)
```

`verify.sh` confirms each recipe lowers to the expected `for _, item in
ipairs(_items)` fan-out: the right member source (`cook.probes.get` / `:field`
index), `$<in.FIELD>` → `cook.member_to_string(item["FIELD"])`, bare `$<in>` →
`cook.member_to_string(item)`, and one `cook.add_unit` per
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
