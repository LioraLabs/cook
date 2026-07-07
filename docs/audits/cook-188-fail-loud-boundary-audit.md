# COOK-188 fail-loud boundary audit

Every value-coercion boundary on the `luagen → register → engine` path, reviewed
for the "wrong-type/unknown input → default or skip → silent success" disease
(COOK-188). One row per site: file, what it used to coerce, and a verdict —
**FIXED** (now a hard error or an honest render, with the task that did it) or
**KEPT** (with the reason it is safe to leave permissive). Line numbers are
post-change (`milestone/engine-trust` base `1e4ded80` + this branch).

Grep tag: `COOK-188 fail-loud boundary audit`.

## Register API — `cook.add_unit` (`cook-register/src/unit_api.rs`)

| Site | Was | Verdict |
|---|---|---|
| `command` read → non-string | `unwrap_or_default()` → `""` (empty command "succeeds") | **FIXED** by COOK-187 (CS-0122); a non-string `command` is rejected via explicit `LuaValue` match. This audit inherits it. |
| `lua_code` (`:321`) | permissive read | **FIXED** T1 — non-string → `type_err("lua_code","a string",…)` |
| `interactive` (`:327`) | `unwrap_or_default()` → false | **FIXED** T1 — non-bool → `type_err` |
| `line` (`:333`) | coerced to 0 | **FIXED** T1 — non-int → `type_err` |
| `cache` (`:339`) | `unwrap_or_default()` | **FIXED** T1 — non-bool → `type_err` |
| `step_kind` (`:368`) | permissive | **FIXED** T1 — non-string → `type_err` |
| `inputs` / env values (`:104`, `:390`) | element coercion | **FIXED** T1 — non-string element → `type_err("… a table of strings", …)`; env values reject non-string coercion (commit 559cecee) |
| `command` Nil/absent (`:305`) → `String::new()` | — | **KEPT**: an absent `command` is legal for a `lua_code` unit; the exactly-one-of check downstream turns "neither present" into the loud error, so `""` here is a sentinel, not a silent default. |
| `lua_code` Nil (`:319`) → `None` | — | **KEPT**: same exactly-one gate; absence is meaningful, not coerced. |
| sigil-scanner index scratch (`:196`, `:205`) `String::new()` | — | **KEPT**: parser position accumulators, not user-facing value reads. |

## Register API — `cook.add_test` (`cook-register/src/test_api.rs`)

| Site | Was | Verdict |
|---|---|---|
| `command` (`:33`) | permissive; `lua_code` unsupported → codegen/`add_test` mismatch (folded COOK-21) | **FIXED** T3 — strict `type_err`; empty string treated as *absent* for the exactly-one check (commit 68510a5e) |
| `lua_code` (`:41`) | did not exist on the payload | **FIXED** T3 — added; non-string → `type_err`; `WorkPayload::Test.lua_code` added in `cook-contracts` |
| exactly-one `command` XOR `lua_code` (`:44`) | neither/both silently accepted | **FIXED** T3 — both-absent and both-present are hard register errors citing §22.4/CS-0127 |

## Codegen (`cook-luagen`)

| Site | Was | Verdict |
|---|---|---|
| `recipe.rs` step-kind dispatch | catch-all `_ => { i += 1; }` silently skipped unknown `Step` variants (`#[non_exhaustive]` — recurs on every new variant) | **FIXED** T5 — `CodegenError::UnhandledStepKind { kind, index, recipe }` (`recipe.rs:65`); codegen returns `Result`, propagated to the register boundary |
| unresolved `$<…>` sigil sentinels baked into emitted Lua as literal `[[SIGIL_ERROR:…]]` | shipped a broken string that failed opaquely at execute | **FIXED** T5 — `CodegenError::PlaceholderViolation` on the checked path (`recipe.rs:213/222/240/288`) |
| `for_each` **plate** bodies with probe refs (`plate_step.rs`) | `command = function() … end` closure → coerced to `""` (silent no-op) | **FIXED** — lowered as literal sigils preserving plate sandbox policy (commit d6bf0b5b); the `Cook`-hardcoded `try_expand_probe_templates` path was not widened, so plate's policy is unchanged |
| `for_each` **test** shell command with probe refs (`test_step.rs`) | closure wrap, no probe-rewrite machinery on the Test path | **FIXED** T4 — hard codegen error naming the probe key + line (probe values are not available to `test` shell commands; read them in a Lua test body) |
| parametric-chore sigil resolver (`recipe.rs`) | `cook.__expand_chore_sigils` resolved **only** bound params: `$<MODE>` (env) errored "no such parameter"; `$<recipe>` silently failed to lower — three behaviors for one token | **FIXED** T6 — one resolution path for all chores; params shadow, then env and recipe refs resolve exactly as in recipe bodies (commit 84eeee7c). Verified: `$<MODE>`→env, `$<who>`→param, `$<build>`→recipe output (and pulls `build` into the closure). |
| `capture.rs __quote_param` | lenient | **FIXED** T6 — strict; the shared quoting path feeds the unified resolver |

## Engine (`cook-engine`, `cook-luaotp`, `cook-progress`)

| Site | Was | Verdict |
|---|---|---|
| recipe status under upstream failure (`executor.rs` finish path; `cook-progress` `render/plain.rs:57`) | a recipe whose only node was `skipped (upstream-failed)` reported `done (1/1)` | **FIXED** T7 (instance 3) — `skipped_nodes` tracked; recipe renders `skipped (n/m ran, upstream-failed)`. Regression-locked by `plain.rs:195 recipe_skipped_writes_skipped_not_done_line`. Verified end-to-end: exit 1, `report  skipped (0/1 ran, upstream-failed)`. |
| lua-body test presatisfaction (`dag_builder.rs:537-542`) | scheduling gap for `lua_code` test units | **FIXED** T4 — a `lua_code` test is never treated as presatisfied |
| worker execution of `lua_code` tests (`pool.rs`) | no execute path for Lua-body tests | **FIXED** — `execute_lua_test` on the worker VM; `success = should_fail XOR chunk_ok`; body folded into the test fingerprint (`cook-fingerprint`) |
| `pool.rs:1035` unknown `WorkPayload` variant | — | **KEPT**: already fail-loud — returns a `WorkResult` with `error: Some("BUG: unknown WorkPayload variant …")`; surfaced, not swallowed. |
| `dag_builder.rs` `.unwrap()/.expect()` (`:238/:257/:279/:311/:476`) | — | **KEPT**: each guarded by a just-checked invariant with a "bug" message; these are assertions, not input coercions. |
| `render/plain.rs` node-attribution / display fallbacks | — | **KEPT**: display-only; a missing label degrades presentation, never correctness or exit code. |
| `Sharing::from_wire_str` catch-all (`cook-contracts`) | unknown disposition → default | **KEPT** at the wire-decode layer (forward-compat decode); the *authoring* boundary in `unit_api` validates the disposition and is where an unknown value is rejected (T1). |

## Not in scope (referred out)

- Shell-quoting leak in sigil expansion (`$<who>` → `'world'`) is **COOK-193 item 6**, not a fail-loud defect — the value resolves; only its quoting is wrong.
- The `set -e` prelude showing in reported commands is **COOK-191** (strip-prelude), landing when this branch integrates onto the post-191 milestone.
