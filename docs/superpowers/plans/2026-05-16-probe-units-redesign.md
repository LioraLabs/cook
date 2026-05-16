# Probe Units Redesign Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Finalise CS-0074 by (a) renaming the `requires` field on `cook.add_unit` to `probes` and (b) making probe execution demand-driven — probes only run when transitively reached from a scheduled non-probe consumer.

**Architecture:** One algorithmic change in `cli/crates/cook-engine/src/dag_builder.rs::build_dag` (reachability prune over probe nodes), plus a mechanical rename across four crates (contracts → register → engine → luagen call sites). Standard chapter §22.5 receives three normative amendments paired with the code changes per spec-first hook. Examples are reshaped to put consumer `cook.add_unit` inside recipe bodies via `>>{ }` so the new demand-driven behaviour is observable in the runnable demos. Three updated conformance fixtures, one new fixture, one new integration test.

**Tech Stack:** Rust (workspace `cli/`), mlua, the in-tree Cook Standard (Starlight MDX), Cook conformance harness (`cargo test -p cook-lang --test conformance`).

**Spec:** `docs/superpowers/specs/2026-05-16-probe-units-redesign-design.md`. Read first; this plan implements that spec.

---

## File Structure

**Modified:**

- `standard/src/content/docs/22-probe-units.mdx` — §22.5.5 (field rename + narrative), §22.5.6 (sigil auto-populates `probes`), §22.5.7 (new "Demand-driven scheduling" subsection + worked example).
- `standard/src/content/docs/appendix/E-changes.mdx` — CS-0074 entry rewrite to reflect final shape.
- `cli/crates/cook-contracts/src/lib.rs` — `CapturedUnit.requires: Vec<String>` → `CapturedUnit.probes: Vec<String>`. Update test fixtures in the same file that construct `CapturedUnit`.
- `cli/crates/cook-register/src/unit_api.rs` — read Lua-table key `"probes"` instead of `"requires"`; reject `"requires"` key with a source-locatable diagnostic; field writes go to `unit.probes`.
- `cli/crates/cook-register/src/engine.rs` — requires-resolution loop reads `unit.probes`; diagnostic text updated.
- `cli/crates/cook-register/src/tests.rs`, `cli/crates/cook-register/src/probe_api.rs` tests — `requires` → `probes` in fixtures + assertions.
- `cli/crates/cook-engine/src/dag_builder.rs` — (a) edge-wiring loop reads `unit.probes`; (b) new `compute_consumed_probe_keys` helper; (c) `build_dag` skips probe nodes whose key is not in `consumed`; (d) new unit tests for prune behaviour.
- `cli/crates/cook-luagen/src/template.rs` and `cli/crates/cook-register/src/unit_api.rs::try_expand_probe_templates` — sigil-pipeline auto-population writes to the `probes` field.
- `examples/probe-basic/Cookfile`, `examples/probe-chain/Cookfile`, `examples/probe-cache-share/Cookfile` — reshape + rename.
- `standard/conformance/positive/probe-register-simple/Cookfile`, `probe-requires-chain/Cookfile`, `probe-template-desugaring/Cookfile`, `probe-value-types/Cookfile` — rename in fixture Cookfiles; re-baseline `parse.txt` and `register_ok.txt`.
- `standard/conformance/negative/probe-duplicate-key/Cookfile`, `probe-unresolved-require/Cookfile`, `probe-cycle/Cookfile` — rename + diagnostic-text baseline updates as needed.
- `cli/crates/cook-cli/tests/probe_integration.rs` — rename + new `probe_unreached_is_not_executed` test.

**Created:**

- `standard/conformance/positive/probe-unreached-pruned/Cookfile` + `parse.txt` + `register_ok.txt` — new positive fixture asserting unreached probe is pruned from DAG and writes no cache artifact.

---

## Section A — Field rename: `requires` → `probes` on `cook.add_unit`

Paired with Standard §22.5.5 + §22.5.6 updates per spec-first hook (CLAUDE.md). All changes in this section land in one commit so language-surface + reference implementation move together.

### Task A1: Update Standard §22.5.5 and §22.5.6 for field rename

**Files:**
- Modify: `standard/src/content/docs/22-probe-units.mdx` — §22.5.5 and §22.5.6 sections plus Example 22.5.6.1.

- [ ] **Step 1: Locate §22.5.5 heading** with `grep -n "## 22.5.5\." standard/src/content/docs/22-probe-units.mdx`. Read the section (it begins at line ~100).

- [ ] **Step 2: Rewrite §22.5.5.** Replace the section body so the field name reads `probes` throughout. The narrative now reads:

> `cook.add_unit({ ..., probes = { "key1", "key2" } })` declares that the unit consumes the named probe values. The `probes` field is an optional list of probe-key strings; omitting it is equivalent to `probes = {}`. (Distinct from `opts.inputs.requires` per §{cat.probes.inputs}, which declares probe-to-probe dependencies inside a probe.)
>
> A conforming implementation MUST, after the register pass completes:
>
> 1. Resolve each key against the set of registered probes. An unknown key MUST raise:
>    ```
>    unit '<name>' lists probe key '<key>' in `probes` but no such probe was declared
>    ```
> 2. Add a DAG edge from each resolved probe to the consumer unit.
> 3. Fold each resolved probe's fingerprint into the consumer unit's fingerprint.
>
> The relative order of `cook.probe` and `cook.add_unit` calls within one register pass is unconstrained; resolution is deferred to end-of-pass.

- [ ] **Step 3: Update §22.5.6 narrative.** Replace any sentence in §22.5.6 that references "declared in `requires`" with "declared in `probes`". Specifically the opening sentence becomes:

> Consumer commands MAY include `$<key>`, `$<key.field>`, and `$<key.field[i]>` placeholders that reference probe outputs declared in `probes`.

- [ ] **Step 4: Update Example 22.5.6.1.** Replace `requires = { "cc:compiler", "cc:zlib" }` with `probes = { "cc:compiler", "cc:zlib" }`.

- [ ] **Step 5: Run Standard render check** (informative — fails closed if Astro is broken):

```bash
cd standard && pnpm install --silent && pnpm build 2>&1 | tail -20
```
Expected: build succeeds, no broken links.

### Task A2: Rename `CapturedUnit.requires` → `CapturedUnit.probes` in contracts

**Files:**
- Modify: `cli/crates/cook-contracts/src/lib.rs` — struct field rename + same-file fixtures.

- [ ] **Step 1: Read the `CapturedUnit` definition** at `cli/crates/cook-contracts/src/lib.rs:196`. Confirm shape:

```rust
pub struct CapturedUnit {
    pub payload: WorkPayload,
    pub cache_meta: Option<CacheMeta>,
    pub dep_kind: DepKind,
    pub requires: Vec<String>,
}
```

- [ ] **Step 2: Rename the field.** Edit line 201 (or current location) to:

```rust
pub probes: Vec<String>,
```

- [ ] **Step 3: Sweep same-file test fixtures.** Replace each `requires: vec![...]` literal within a `CapturedUnit { ... }` construction in `cook-contracts/src/lib.rs` with `probes: vec![...]`. There are ~6 sites (lines 68, 201, 409, 441, 447, 511, 544, 589 per earlier grep — re-confirm with `grep -n "requires:" cli/crates/cook-contracts/src/lib.rs`).

- [ ] **Step 4: Run contracts unit tests.**

```bash
cargo test -p cook-contracts
```
Expected: passes. If a test still references `.requires` field access, update it to `.probes` and rerun.

### Task A3: Update register-phase Lua reader to accept `probes`, reject `requires`

**Files:**
- Modify: `cli/crates/cook-register/src/unit_api.rs` — Lua-table key read in `cook.add_unit` impl.

- [ ] **Step 1: Locate the current `requires` reader** at `cli/crates/cook-register/src/unit_api.rs:484` (search: `grep -n '"requires"' cli/crates/cook-register/src/unit_api.rs`).

- [ ] **Step 2: Add a guard that rejects the legacy `requires` key with a clear diagnostic.** Insert before the existing read (line ~484):

```rust
// Reject legacy `requires` field name (CS-0074 phase 2 rename).
// The branch is unmerged and 0.11 is unreleased, so no compat shim.
if let Ok(LuaValue::Table(_)) = tbl.get::<LuaValue>("requires") {
    return Err(LuaError::runtime(
        "cook.add_unit: field `requires` is no longer accepted for probe references; rename to `probes`".to_string(),
    ));
}
```

- [ ] **Step 3: Change the read key from `"requires"` to `"probes"`.** Replace:

```rust
let mut requires: Vec<String> = match tbl.get::<LuaValue>("requires") {
```
with:
```rust
let mut probes: Vec<String> = match tbl.get::<LuaValue>("probes") {
```

- [ ] **Step 4: Update inner error messages from "requires" to "probes".** In the same match arms, replace the two error strings:

```rust
"cook.add_unit: probes must be a list of strings: {e}"
```
and
```rust
"cook.add_unit: probes must be a list of strings (or nil)"
```

- [ ] **Step 5: Update the local variable name and the field-write site.** Anywhere in this function body that references the local `requires` (the Vec we just renamed `probes`) and writes it to the captured unit, change `requires: probes_local` style usage to `probes: probes_local`. The CapturedUnit construction sites at the end of this function need `probes,` instead of `requires,`.

- [ ] **Step 6: Also update `try_expand_probe_templates` auto-population.** Search the file for the place where the sigil scanner's returned probe keys are merged into the unit's requires list (it's in or near `try_expand_probe_templates`). Rename the variable / field accordingly so the auto-populated keys end up in `unit.probes`.

- [ ] **Step 7: Compile.**

```bash
cargo check -p cook-register
```
Expected: passes after the field/local rename is consistent.

### Task A4: Update register engine diagnostic + downstream field access

**Files:**
- Modify: `cli/crates/cook-register/src/engine.rs` — requires-resolution loop (lines 357–387 per earlier read).
- Modify: `cli/crates/cook-engine/src/dag_builder.rs` — probe→consumer edge wiring (lines 126–143).
- Modify: `cli/crates/cook-register/src/tests.rs`, `cli/crates/cook-register/src/probe_api.rs` — test assertions referencing `requires`.

- [ ] **Step 1: Update `register/engine.rs` resolver.** Find the loop iterating `unit.requires` (around line 364). Replace `for key in &unit.requires` with `for key in &unit.probes`. Update the diagnostic format string:

```rust
return Err(RegisterError::Lua(mlua::Error::runtime(format!(
    "unit '{}' lists probe key '{}' in `probes` but no such probe was declared",
    unit_label, key
))));
```

- [ ] **Step 2: Update `dag_builder.rs` edge wiring.** Find `for req_key in &unit.requires` (line ~130). Replace with `for req_key in &unit.probes`.

- [ ] **Step 3: Sweep `cook-register` tests.** Find all test cases that build a `CapturedUnit { ... requires: vec![...], ... }` or assert against `.requires`:

```bash
grep -n "requires" cli/crates/cook-register/src/tests.rs cli/crates/cook-register/src/probe_api.rs
```
Rename each occurrence inside `CapturedUnit { }` literals and `.requires` field reads to `probes`. Lua-side fixtures that pass `requires = {...}` to `cook.add_unit` change to `probes = {...}`.

- [ ] **Step 4: Build the workspace.**

```bash
cargo check --workspace
```
Expected: clean. Any remaining compile error names a forgotten site — fix in place.

- [ ] **Step 5: Run all tests.**

```bash
cargo test --workspace
```
Expected: all existing tests pass except any that explicitly assert the legacy `requires` is accepted — those should fail. Update those tests' fixtures to use `probes`, plus add one new test that asserts the legacy `requires` key is rejected:

```rust
#[test]
fn legacy_requires_field_is_rejected() {
    let source = r#"
        cook.probe("k", { inputs = {}, produce = [[ return 1 ]] })
        cook.recipe("r", {}, function()
            cook.add_unit({
                name = "u", inputs = {}, outputs = {"o"},
                requires = {"k"},
                command = "echo",
            })
        end)
    "#;
    let err = run_register(source, "r").expect_err("must reject legacy requires");
    assert!(err.to_string().contains("rename to `probes`"));
}
```
Place this test in `cli/crates/cook-register/src/tests.rs` or `probe_api.rs` (whichever already houses similar negative tests).

- [ ] **Step 6: Commit section A.**

```bash
git add standard/src/content/docs/22-probe-units.mdx \
        cli/crates/cook-contracts/src/lib.rs \
        cli/crates/cook-register/src/unit_api.rs \
        cli/crates/cook-register/src/engine.rs \
        cli/crates/cook-register/src/tests.rs \
        cli/crates/cook-register/src/probe_api.rs \
        cli/crates/cook-engine/src/dag_builder.rs
git commit -m "$(cat <<'EOF'
feat(cs-0074): rename cook.add_unit.requires to probes (Standard §22.5.5/§22.5.6)

The field is strictly probe keys; the name `probes` distinguishes it
from cook.recipe.requires (recipe deps) and cook.probe.inputs.requires
(cache-fingerprint upstream).  Legacy `requires` on cook.add_unit is
rejected with a source-locatable diagnostic; no compat shim since the
branch is unmerged and 0.11 is unreleased.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Section B — Demand-driven probe scheduling

Paired with Standard §22.5.7 amendment. TDD: write failing tests for the new prune behaviour, watch them fail, implement the algorithm, watch them pass.

### Task B1: Add §22.5.7 demand-driven subsection to Standard

**Files:**
- Modify: `standard/src/content/docs/22-probe-units.mdx` — §22.5.7 (Execution semantics).

- [ ] **Step 1: Locate §22.5.7 heading** with `grep -n "## 22.5.7\." standard/src/content/docs/22-probe-units.mdx`.

- [ ] **Step 2: Append a "Demand-driven scheduling" paragraph at the end of §22.5.7** (before the on-disk-layout paragraph if present, or as a new closing paragraph):

```
**Demand-driven scheduling.** A probe unit MUST NOT execute on an invocation unless at least one scheduled non-probe unit lists the probe's key in its `probes` field, directly or transitively through probe-on-probe `inputs.requires` chains. A conforming implementation MUST silently omit unreached probe nodes from the execution DAG. The implementation MUST NOT compute, look up, or persist the fingerprint of an unreached probe; MUST NOT emit a user-facing diagnostic; and MUST NOT log a warning at the user-facing diagnostic layer. Debug logging at lower verbosity tiers is permitted.
```

- [ ] **Step 3: Add Example 22.5.7.1** (worked example) immediately after the new paragraph:

````
### Example 22.5.7.1

```
register
    cook.probe("env:slow", {
        inputs = {},
        produce = [[ return { ts = os.time() } ]],
    })

recipe foo
    >>{
        cook.add_unit({
            name = "u1", inputs = {}, outputs = {"build/foo.txt"},
            probes = {"env:slow"},
            command = "echo $<env:slow.ts> > build/foo.txt",
        })
    }
    > cook.sh("cat build/foo.txt")

recipe bar
    > cook.sh("echo bar")
```

`cook foo` MUST execute the `env:slow` probe and `u1` exactly once.
`cook bar` MUST execute no probes; `env:slow` is unreached and its
fingerprint MUST NOT be computed.
````

### Task B2: Write failing test — unreached probe is pruned

**Files:**
- Modify: `cli/crates/cook-engine/src/dag_builder.rs` — append to the `#[cfg(test)] mod tests { ... }` block at the bottom.

- [ ] **Step 1: Add the test.** Append to `dag_builder.rs::tests`. Note: `WorkPayload::Probe` carries `{ key, produce, line }`; the matching `ProbeUnit` (with inputs) goes into `RecipeUnits.probes`. Both must be populated for a probe to be recognised end-to-end.

```rust
#[test]
fn unreached_probe_is_pruned_from_dag() {
    use cook_contracts::{CapturedUnit, DepKind, ProbeUnit, ProbeInputs, WorkPayload};

    let probe_payload = WorkPayload::Probe {
        key: "k:unused".to_string(),
        produce: "return 1".to_string(),
        line: 1,
    };
    let probe_meta = ProbeUnit {
        key: "k:unused".to_string(),
        produce_source: "return 1".to_string(),
        produce_line: 1,
        inputs: ProbeInputs::default(),
    };

    let units = RecipeUnits {
        recipe_name: "r".to_string(),
        deps: vec![],
        units: vec![
            CapturedUnit {
                payload: probe_payload,
                cache_meta: None,
                dep_kind: DepKind::Sequential,
                probes: vec![],
            },
            CapturedUnit {
                payload: WorkPayload::Shell {
                    cmd: "echo hello".to_string(),
                    line: 2,
                },
                cache_meta: None,
                dep_kind: DepKind::Sequential,
                probes: vec![],
            },
        ],
        step_groups: vec![],
        working_dir: std::path::PathBuf::from("/"),
        env_vars: std::collections::BTreeMap::new(),
        terminal_outputs: vec![],
        dep_edges: vec![],
        probes: vec![probe_meta],
    };

    let dag = build_dag(vec![units]).expect("dag build");
    let probe_nodes: Vec<_> = (0..dag.len())
        .map(|i| dag.node(i))
        .filter(|n| matches!(n.payload().payload, Some(WorkPayload::Probe { .. })))
        .collect();
    assert!(probe_nodes.is_empty(), "unreached probe must not appear in DAG");
}
```

(If the existing dag tests already define helper builders, mirror them. The point is: one Probe payload + one Shell payload, no `probes` references on the Shell unit, and assert the resulting DAG contains zero `WorkPayload::Probe` nodes.)

- [ ] **Step 2: Run the test; expect failure.**

```bash
cargo test -p cook-engine unreached_probe_is_pruned_from_dag -- --nocapture
```
Expected: FAIL — assertion fires because the current `build_dag` adds every unit unconditionally.

### Task B3: Write failing test — probe-on-probe chain prunes correctly

**Files:**
- Modify: `cli/crates/cook-engine/src/dag_builder.rs::tests`.

- [ ] **Step 1: Add the test.** Two probes in a chain (`A` upstream, `B` downstream-of-A) plus one consumer of `B`. Assert all three present in DAG. Then re-run without the consumer; assert all three absent.

```rust
#[test]
fn probe_chain_keeps_upstream_when_downstream_consumed() {
    use cook_contracts::{CapturedUnit, DepKind, ProbeUnit, ProbeInputs, WorkPayload};

    let probe_a_payload = WorkPayload::Probe {
        key: "k:a".to_string(),
        produce: "return 1".to_string(),
        line: 1,
    };
    let probe_b_payload = WorkPayload::Probe {
        key: "k:b".to_string(),
        produce: "return 2".to_string(),
        line: 2,
    };
    let probe_a_meta = ProbeUnit {
        key: "k:a".to_string(),
        produce_source: "return 1".to_string(),
        produce_line: 1,
        inputs: ProbeInputs::default(),
    };
    let probe_b_meta = ProbeUnit {
        key: "k:b".to_string(),
        produce_source: "return 2".to_string(),
        produce_line: 2,
        inputs: ProbeInputs {
            requires: vec!["k:a".to_string()],
            ..ProbeInputs::default()
        },
    };
    let probe_a = CapturedUnit {
        payload: probe_a_payload,
        cache_meta: None,
        dep_kind: DepKind::Sequential,
        probes: vec![],
    };
    let probe_b = CapturedUnit {
        payload: probe_b_payload,
        cache_meta: None,
        dep_kind: DepKind::Sequential,
        probes: vec![],
    };
    let consumer = CapturedUnit {
        payload: WorkPayload::Shell { cmd: "echo".to_string(), line: 3 },
        cache_meta: None,
        dep_kind: DepKind::Sequential,
        probes: vec!["k:b".to_string()],
    };

    let make_ru = |units: Vec<CapturedUnit>| RecipeUnits {
        recipe_name: "r".to_string(),
        units,
        deps: vec![],
        step_groups: vec![],
        working_dir: std::path::PathBuf::from("/"),
        env_vars: std::collections::BTreeMap::new(),
        terminal_outputs: vec![],
        dep_edges: vec![],
        probes: vec![probe_a_meta.clone(), probe_b_meta.clone()],
    };

    let with_consumer = make_ru(vec![probe_a.clone(), probe_b.clone(), consumer]);
    let dag = build_dag(vec![with_consumer]).unwrap();
    let probe_count = (0..dag.len())
        .map(|i| dag.node(i))
        .filter(|n| matches!(n.payload().payload, Some(WorkPayload::Probe { .. })))
        .count();
    assert_eq!(probe_count, 2, "both probes must be present when downstream is consumed");

    let without_consumer = make_ru(vec![probe_a, probe_b]);
    let dag2 = build_dag(vec![without_consumer]).unwrap();
    let probe_count2 = (0..dag2.len())
        .map(|i| dag2.node(i))
        .filter(|n| matches!(n.payload().payload, Some(WorkPayload::Probe { .. })))
        .count();
    assert_eq!(probe_count2, 0, "both probes must be pruned when nothing consumes downstream");
}
```

- [ ] **Step 2: Run it; expect failure (assertion #2 fires — both probes currently run unconditionally).**

```bash
cargo test -p cook-engine probe_chain_keeps_upstream_when_downstream_consumed
```
Expected: FAIL.

### Task B4: Write failing test — multi-RecipeUnits wave prunes independently

**Files:**
- Modify: `cli/crates/cook-engine/src/dag_builder.rs::tests`.

- [ ] **Step 1: Add the test.** Two `RecipeUnits` in one wave. `foo` has a consumer of `k:p`; `bar` does not. Assert the probe appears once in the DAG (under `foo`'s recipe region) and zero times under `bar`'s region.

```rust
#[test]
fn multi_recipe_wave_prunes_independently() {
    use cook_contracts::{CapturedUnit, DepKind, ProbeUnit, ProbeInputs, WorkPayload};

    fn make_recipe(name: &str, has_consumer: bool) -> RecipeUnits {
        let probe_meta = ProbeUnit {
            key: "k:p".to_string(),
            produce_source: "return 1".to_string(),
            produce_line: 1,
            inputs: ProbeInputs::default(),
        };
        let mut units = vec![CapturedUnit {
            payload: WorkPayload::Probe {
                key: "k:p".to_string(),
                produce: "return 1".to_string(),
                line: 1,
            },
            cache_meta: None,
            dep_kind: DepKind::Sequential,
            probes: vec![],
        }];
        units.push(CapturedUnit {
            payload: WorkPayload::Shell { cmd: "echo".to_string(), line: 2 },
            cache_meta: None,
            dep_kind: DepKind::Sequential,
            probes: if has_consumer { vec!["k:p".to_string()] } else { vec![] },
        });
        RecipeUnits {
            recipe_name: name.to_string(),
            units,
            deps: vec![],
            step_groups: vec![],
            working_dir: std::path::PathBuf::from("/"),
            env_vars: std::collections::BTreeMap::new(),
            terminal_outputs: vec![],
            dep_edges: vec![],
            probes: vec![probe_meta],
        }
    }

    let foo = make_recipe("foo", true);
    let bar = make_recipe("bar", false);
    let dag = build_dag(vec![foo, bar]).unwrap();
    let probe_node_recipes: Vec<String> = (0..dag.len())
        .map(|i| dag.node(i))
        .filter(|n| matches!(n.payload().payload, Some(WorkPayload::Probe { .. })))
        .map(|n| n.payload().recipe_name.clone())
        .collect();
    assert_eq!(probe_node_recipes, vec!["foo".to_string()],
        "probe present only in the recipe that consumes it");
}
```

- [ ] **Step 2: Run it; expect failure.**

```bash
cargo test -p cook-engine multi_recipe_wave_prunes_independently
```
Expected: FAIL.

### Task B5: Implement reachability prune in `build_dag`

**Files:**
- Modify: `cli/crates/cook-engine/src/dag_builder.rs::build_dag` (line ~50 onwards).

- [ ] **Step 1: Add a helper at module scope** (above `build_dag`). The transitive close needs access to `ProbeInputs.requires`, which lives on `ProbeUnit` in `RecipeUnits.probes` — NOT in `WorkPayload::Probe` (which only carries `key`, `produce`, `line`). So the helper takes both:

```rust
/// Compute the set of probe keys reached by at least one non-probe consumer
/// in `units`, transitively closing under probe-on-probe `inputs.requires`
/// looked up via `probes` (the ProbeUnit list from RecipeUnits.probes).
/// Returns probe keys in deterministic sorted order via the underlying BTreeSet.
fn compute_consumed_probe_keys(
    units: &[CapturedUnit],
    probes: &[cook_contracts::ProbeUnit],
) -> BTreeSet<String> {
    use std::collections::BTreeMap;
    use cook_contracts::WorkPayload;

    // Index ProbeUnit data by key, for upstream-requires lookup.
    let probe_by_key: BTreeMap<&str, &cook_contracts::ProbeUnit> =
        probes.iter().map(|p| (p.key.as_str(), p)).collect();

    // Seed: keys listed by any non-probe unit's `probes`.
    let mut consumed: BTreeSet<String> = BTreeSet::new();
    for u in units {
        if !matches!(u.payload, WorkPayload::Probe { .. }) {
            for k in &u.probes {
                consumed.insert(k.clone());
            }
        }
    }

    // Transitive close under probe-on-probe inputs.requires.
    let mut worklist: Vec<String> = consumed.iter().cloned().collect();
    while let Some(k) = worklist.pop() {
        if let Some(probe) = probe_by_key.get(k.as_str()) {
            for upstream in &probe.inputs.requires {
                if consumed.insert(upstream.clone()) {
                    worklist.push(upstream.clone());
                }
            }
        }
    }
    consumed
}
```

- [ ] **Step 2: Hook it into the per-RecipeUnits loop.** Inside `build_dag`'s `for ru in &recipe_units` body, right after `probe_unit_index_by_key` is built, compute the prune set:

```rust
let consumed = compute_consumed_probe_keys(&ru.units, &ru.probes);

// Skip indices: probe units whose key is not consumed.
let skip_indices: BTreeSet<usize> = ru.units.iter().enumerate()
    .filter_map(|(i, u)| match &u.payload {
        WorkPayload::Probe { key, .. } if !consumed.contains(key) => Some(i),
        _ => None,
    })
    .collect();
```

- [ ] **Step 3: Skip pruned probes in the add-node loop.** Inside `for (unit_idx, unit) in ru.units.iter().enumerate()`, add at the top of the loop body:

```rust
if skip_indices.contains(&unit_idx) {
    continue;
}
```

- [ ] **Step 4: Update probe→consumer wiring to ignore skipped probes.** In the existing block (line ~130) that does:

```rust
for req_key in &unit.probes {
    if let Some(&probe_unit_idx) = probe_unit_index_by_key.get(req_key) {
        if let Some(&probe_dag_id) = dag_id_by_unit_idx.get(&probe_unit_idx) {
            ...
        }
    }
}
```
This is already correct after the skip because `dag_id_by_unit_idx` is populated only for non-skipped probes; a probe pruned via `skip_indices` won't appear in `dag_id_by_unit_idx`, and the `if let Some(&probe_dag_id)` arm safely no-ops. No code change here. Sanity-check this assumption in the next step.

- [ ] **Step 5: Run the three failing tests; expect pass.**

```bash
cargo test -p cook-engine unreached_probe_is_pruned_from_dag \
                         probe_chain_keeps_upstream_when_downstream_consumed \
                         multi_recipe_wave_prunes_independently
```
Expected: PASS for all three.

- [ ] **Step 6: Run the full engine test suite + the workspace suite to confirm no regression.**

```bash
cargo test -p cook-engine && cargo test --workspace
```
Expected: all pass. If a pre-existing test relied on "probe always appears in DAG with no consumer," it must be updated to add a consumer, since the old behaviour was the bug.

- [ ] **Step 7: Run the three demo examples to confirm end-to-end behaviour.**

```bash
for ex in probe-basic probe-chain probe-cache-share; do
    rm -rf examples/$ex/.cook examples/$ex/build
    (cd examples/$ex && cargo run --quiet --manifest-path ../../cli/Cargo.toml --bin cook -- build 2>&1 | tail -10)
done
```
Expected: each runs end-to-end with the existing example shapes (which will be reshaped in Section D). For now this just confirms the engine change doesn't regress the existing happy path.

- [ ] **Step 8: Commit section B.**

```bash
git add standard/src/content/docs/22-probe-units.mdx \
        cli/crates/cook-engine/src/dag_builder.rs
git commit -m "$(cat <<'EOF'
feat(cs-0074): demand-driven probe scheduling — prune unreached probes (§22.5.7)

build_dag now computes the set of probe keys transitively reached by a
non-probe consumer in each RecipeUnits, and skips probe units whose key
is not in that set.  Adds compute_consumed_probe_keys helper with
deterministic BTreeSet iteration; the existing probe→consumer edge
wiring no-ops correctly for pruned probes since their dag_ids are never
inserted.

Standard §22.5.7 gains a normative "Demand-driven scheduling" clause and
worked Example 22.5.7.1.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Section C — Appendix E refresh

### Task C1: Rewrite CS-0074 entry in App. E

**Files:**
- Modify: `standard/src/content/docs/appendix/E-changes.mdx` — line ~1274 onwards (the existing CS-0074 entry).

- [ ] **Step 1: Locate the entry** with `grep -n "## CS-0074" standard/src/content/docs/appendix/E-changes.mdx`.

- [ ] **Step 2: Rewrite the summary paragraph(s) of CS-0074** so they reflect the final shape. The new opening paragraph reads:

> **Summary.** CS-0074 introduces probe units. New register-phase API `cook.probe(key, opts)` registers a DAG unit whose output is a Lua value rather than a file (§22.5). Consumer units declare probe-edge dependencies via a `probes` field on `cook.add_unit`; the `$<key:field>` sigil in command templates auto-populates this field (§22.5.5, §22.5.6). Probe values are msgpack-encoded and stored via the existing cache backend; the `ArtifactMeta.kind` field disambiguates probe-value artifacts from file artifacts (§22.5.4, §22.5.7). Probes execute on demand: a probe MUST NOT run unless at least one scheduled non-probe consumer transitively references its key (§22.5.7). `cook.cache.set` is deprecated and raises a runtime error on the execute-phase VM (§24). `cook.cache.get` on the execute-phase VM reads from a per-run probe-value store populated by the scheduler (§24).

- [ ] **Step 3: Update the "Sections affected" list** to enumerate §22.5 (new chapter), §17 (cache cross-ref), §21 (Lua-API pointer), §24 (both-phase get tightening), and App. E (this entry). If the line at the top of the changelog refers to CS-0074 with the old description (line 12 from grep: "v0.11 — CS-0074. Probe units: ..."), update that synopsis line as well so it matches the final feature description:

```
- **v0.11** (`cs-standard/v0.11`, 2026-05-15) — CS-0074. Probe units: new §22.5 `cook.probe` register-phase API; `probes` field on `cook.add_unit` for consumer→probe DAG edges; demand-driven scheduling (probes execute only when reached); msgpack probe-value caching with `kind: "probe_value"` artifact-meta tag; `cook.cache.set` deprecated (§24); `cook.cache.get` execute-phase tightened.
```

- [ ] **Step 4: Update the "Reference implementation" section of the CS-0074 entry** to list the crates/files involved post-redesign:

```
- standard/src/content/docs/22-probe-units.mdx (chapter)
- standard/src/content/docs/appendix/E-changes.mdx (this entry)
- standard/src/content/docs/21-lua-api.mdx (§21 cook.probe pointer + cook.cache.get cross-phase note)
- standard/src/content/docs/24-both-phase.mdx (cook.cache.get tightening)
- cli/crates/cook-contracts/src/lib.rs (ProbeUnit, ProbeInputs, WorkPayload::Probe, RecipeUnits.probes, CapturedUnit.probes)
- cli/crates/cook-register/src/probe_api.rs, src/unit_api.rs, src/engine.rs, src/probe_value.rs
- cli/crates/cook-fingerprint/src/probe.rs
- cli/crates/cook-engine/src/dag_builder.rs (probe→consumer edges + demand-driven prune)
- cli/crates/cook-engine/src/executor.rs (probe cache hit/miss + upstream fingerprint propagation)
- cli/crates/cook-luaotp/src/probe_value.rs (worker-VM probe dispatch + probe-value store)
- cli/crates/cook-luagen/src/sigil.rs, src/resolver.rs, src/template.rs ($<key:field> pipeline)
```

- [ ] **Step 5: Commit.**

```bash
git add standard/src/content/docs/appendix/E-changes.mdx
git commit -m "$(cat <<'EOF'
docs(cs-0074): App E entry refresh — final feature shape

Reflects the final CS-0074 shape after the probes-field rename and
demand-driven scheduling: `probes` field on cook.add_unit, demand-driven
prune in dag_builder, $<key:field> sigil pipeline, msgpack value
caching.  Supersedes the prose introduced when CS-0074 first landed on
feature/probe-units.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Section D — Example reshape

Three Cookfiles. Consumer `cook.add_unit` moves inside the recipe body via `>>{ }` so the demand-driven behaviour is observable in the runnable demos.

### Task D1: Reshape `examples/probe-basic/Cookfile`

**Files:**
- Modify: `examples/probe-basic/Cookfile`.

- [ ] **Step 1: Write the new file contents.**

```
register
    cook.probe("demo:compiler", {
        inputs = { env = {"PATH"}, tools = {"cc"} },
        produce = [[
            local path = cook.sh("which cc"):gsub("%s+$", "")
            return { path = path }
        ]],
    })

recipe build
    >>{
        cook.add_unit({
            name    = "demo-binary",
            inputs  = {"main.c"},
            outputs = {"build/demo"},
            probes  = {"demo:compiler"},
            command = "$<demo:compiler.path> main.c -o build/demo",
        })
    }
    > cook.sh("./build/demo")
```

- [ ] **Step 2: Clear stale build state and run end-to-end.**

```bash
rm -rf examples/probe-basic/.cook examples/probe-basic/build
(cd examples/probe-basic && cargo run --quiet --manifest-path ../../cli/Cargo.toml --bin cook -- build 2>&1 | tail -15)
```
Expected: probe runs, `demo-binary` unit builds `build/demo`, imperative step prints "hello" (or whatever `main.c` outputs). Exit 0.

- [ ] **Step 3: Run a second time and confirm cache hit on the probe.**

```bash
(cd examples/probe-basic && cargo run --quiet --manifest-path ../../cli/Cargo.toml --bin cook -- build 2>&1 | tail -15)
```
Expected: probe shows `cached` in the line summary; `demo-binary` shows `cached`.

### Task D2: Reshape `examples/probe-chain/Cookfile`

**Files:**
- Modify: `examples/probe-chain/Cookfile`.

- [ ] **Step 1: Write the new file contents.**

```
register
    cook.probe("demo:cc-path", {
        inputs = { env = {"PATH"}, tools = {"cc"} },
        produce = [[
            local path = cook.sh("which cc"):gsub("%s+$", "")
            return { path = path }
        ]],
    })

    cook.probe("demo:cc-version", {
        inputs = { requires = {"demo:cc-path"} },
        produce = [[
            local cc = cook.cache.get("demo:cc-path").path
            local ver = cook.sh(cc .. " -dumpversion 2>&1"):gsub("%s+$", "")
            return { version = ver }
        ]],
    })

recipe build
    >>{
        cook.add_unit({
            name    = "print-version",
            inputs  = {},
            outputs = {"build/version.txt"},
            probes  = {"demo:cc-version"},
            command = "mkdir -p build && echo $<demo:cc-version.version> > build/version.txt",
        })
    }
    > cook.sh("cat build/version.txt")
```

- [ ] **Step 2: Run end-to-end.**

```bash
rm -rf examples/probe-chain/.cook examples/probe-chain/build
(cd examples/probe-chain && cargo run --quiet --manifest-path ../../cli/Cargo.toml --bin cook -- build 2>&1 | tail -15)
```
Expected: both probes run (chain — upstream first), unit produces `build/version.txt`, cat step prints the version. Exit 0.

### Task D3: Rename `examples/probe-cache-share/Cookfile`

**Files:**
- Modify: `examples/probe-cache-share/Cookfile`.

- [ ] **Step 1: Read the current file** to confirm shape (it should already match the recommended pattern from in-tree edits, only needing `requires` → `probes` rename).

- [ ] **Step 2: Apply the rename.** Replace the single occurrence of `requires = {"demo:slow"}` with `probes = {"demo:slow"}` inside the `>>{ cook.add_unit({...}) }` block.

- [ ] **Step 3: Run end-to-end.**

```bash
rm -rf examples/probe-cache-share/.cook examples/probe-cache-share/build
(cd examples/probe-cache-share && cargo run --quiet --manifest-path ../../cli/Cargo.toml --bin cook -- build 2>&1 | tail -15)
```
Expected: probe runs (sleep 1 + date), `record` writes `build/probed.txt`, imperative step cats the timestamp. Exit 0.

- [ ] **Step 4: Run a second time** and confirm cache-hit.

```bash
(cd examples/probe-cache-share && cargo run --quiet --manifest-path ../../cli/Cargo.toml --bin cook -- build 2>&1 | tail -15)
```
Expected: probe `cached`, record `cached`, cat reads the previously-written timestamp; no second `sleep 1`. Total elapsed time well under 1s.

- [ ] **Step 5: Commit section D.**

```bash
git add examples/probe-basic/Cookfile examples/probe-chain/Cookfile examples/probe-cache-share/Cookfile
git commit -m "$(cat <<'EOF'
examples(cs-0074): reshape probe demos — consumers inside recipe body

Each demo moves the consumer cook.add_unit inside `recipe build` via
`>>{ }`, so the demand-driven probe behaviour from §22.5.7 is observable
in the runnable examples.  Field rename requires→probes applied
throughout.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Section E — Conformance corpus

### Task E1: Apply rename to existing positive fixtures

**Files:**
- Modify: `standard/conformance/positive/probe-register-simple/Cookfile`
- Modify: `standard/conformance/positive/probe-requires-chain/Cookfile`
- Modify: `standard/conformance/positive/probe-template-desugaring/Cookfile`
- Modify: `standard/conformance/positive/probe-value-types/Cookfile`

- [ ] **Step 1: Sweep each fixture's Cookfile.** Replace any `requires = { ... }` field on a `cook.add_unit` call with `probes = { ... }`. Do **not** touch `cook.probe.inputs.requires` (those stay).

```bash
grep -rn "requires" standard/conformance/positive/probe-{register-simple,requires-chain,template-desugaring,value-types}/Cookfile
```
For each match in a `cook.add_unit({...})` context, apply the rename.

- [ ] **Step 2: For `probe-requires-chain`** specifically, the fixture currently puts its consumer at the top level. Reshape it to put the consumer inside a recipe body via `>>{ }`, mirroring `examples/probe-chain` shape, so the fixture exercises the recommended pattern.

- [ ] **Step 3: Re-baseline `parse.txt` and `register_ok.txt`** for each updated fixture. The conformance harness baseline files are generated by `cargo test -p cook-lang --test conformance -- <fixture-name>` and may need regeneration if the parse output (line numbers, token positions) shifts. Use the project's existing baseline-update workflow:

```bash
# Try running the conformance test; on diff failure, inspect actual output and update baselines.
cargo test -p cook-lang --test conformance probe-register-simple
cargo test -p cook-lang --test conformance probe-requires-chain
cargo test -p cook-lang --test conformance probe-template-desugaring
cargo test -p cook-lang --test conformance probe-value-types
```
If any test fails on baseline mismatch, regenerate via the project's standard mechanism (look for a `UPDATE=1` env or `--update-baselines` flag in the conformance test harness; check `cli/crates/cook-lang/tests/conformance.rs` source if unclear).

### Task E2: Apply rename to existing negative fixtures

**Files:**
- Modify: `standard/conformance/negative/probe-duplicate-key/Cookfile`
- Modify: `standard/conformance/negative/probe-unresolved-require/Cookfile`
- Modify: `standard/conformance/negative/probe-cycle/Cookfile`
- Modify: corresponding `error.txt` baseline files in each fixture directory.

- [ ] **Step 1: Sweep negative fixtures' Cookfiles.** Apply `requires` → `probes` rename on any `cook.add_unit` call. (`probe-cycle` exercises `inputs.requires` on `cook.probe`; that stays.)

- [ ] **Step 2: Update `probe-unresolved-require`'s expected diagnostic baseline.** The fixture's `error.txt` (or equivalent) currently asserts a diagnostic mentioning "requires." Update to the new diagnostic text:

```
unit '<name>' lists probe key '<key>' in `probes` but no such probe was declared
```
The exact `<name>` and `<key>` placeholders are the literal strings from the fixture's Cookfile — substitute them. Rename the fixture itself if it makes sense post-rename: `probe-unresolved-require` → `probe-unresolved-probes` (optional cleanup; do this if the rename feels straightforward, otherwise leave the directory name alone and update prose only).

- [ ] **Step 3: Run negative conformance tests.**

```bash
cargo test -p cook-lang --test conformance probe-duplicate-key
cargo test -p cook-lang --test conformance probe-unresolved-require
cargo test -p cook-lang --test conformance probe-cycle
```
Expected: each fixture's expected-error path matches the new diagnostic text.

### Task E3: Add new positive fixture `probe-unreached-pruned`

**Files:**
- Create: `standard/conformance/positive/probe-unreached-pruned/Cookfile`
- Create: `standard/conformance/positive/probe-unreached-pruned/parse.txt`
- Create: `standard/conformance/positive/probe-unreached-pruned/register_ok.txt`

- [ ] **Step 1: Write the fixture Cookfile.**

```
register
    cook.probe("demo:unused", {
        inputs = {},
        produce = [[ return { v = 1 } ]],
    })

recipe build
    > cook.sh("echo hello")
```

- [ ] **Step 2: Run the conformance harness with baseline-generation enabled** to seed `parse.txt` and `register_ok.txt`. Use the project's existing mechanism (consult `cli/crates/cook-lang/tests/conformance.rs` to find the env var or flag):

```bash
# Example pattern (verify against the actual harness):
UPDATE_BASELINES=1 cargo test -p cook-lang --test conformance probe-unreached-pruned 2>&1 | tail -20
```
Then check in the generated files. Inspect them manually to confirm they make sense (the parse.txt should show a clean parse; register_ok.txt should show the probe registered + the recipe with one shell step).

- [ ] **Step 3: Add a DAG-inspection assertion.** This requires understanding how the existing harness exposes the post-register DAG. Two options:

  (a) If the conformance harness already supports a `dag.txt` baseline file per fixture, write one for `probe-unreached-pruned` showing zero `WorkPayload::Probe` nodes for `demo:unused`. Check the harness source for `dag.txt` handling: `grep -n "dag\\.txt\\|build_dag\\|Dag" cli/crates/cook-lang/tests/conformance.rs`.

  (b) If no such mechanism exists, add a Rust integration test in `cli/crates/cook-cli/tests/probe_integration.rs` (covered in Section F) that loads this fixture's Cookfile, runs the register pass, builds the DAG, and asserts zero probe nodes. Document this in the fixture's `README.md` (a one-line note) explaining that DAG-prune assertion happens via the integration test rather than a baseline file.

For (b), the fixture's purpose is to lock the parse + register expectations; the runtime prune is asserted out-of-band.

- [ ] **Step 4: Run the new fixture.**

```bash
cargo test -p cook-lang --test conformance probe-unreached-pruned
```
Expected: PASS.

### Task E4: Run the full conformance harness

- [ ] **Step 1: Run all probe-related conformance tests.**

```bash
cargo test -p cook-lang --test conformance probe
```
Expected: all pass.

- [ ] **Step 2: Run the entire conformance harness** to catch any cross-fixture regressions:

```bash
cargo test -p cook-lang --test conformance
```
Expected: all pass.

- [ ] **Step 3: Commit section E.**

```bash
git add standard/conformance/positive/probe-{register-simple,requires-chain,template-desugaring,value-types,unreached-pruned} \
        standard/conformance/negative/probe-{duplicate-key,unresolved-require,cycle}
git commit -m "$(cat <<'EOF'
test(cs-0074): conformance corpus rename + new probe-unreached-pruned fixture

Applies `requires`→`probes` rename across existing positive and negative
probe fixtures and re-baselines parse.txt / register_ok.txt / error.txt
as needed.  New positive fixture `probe-unreached-pruned` locks the
expected parse + register state for a top-level probe with no consumer;
the demand-driven prune itself is asserted in the integration test
(see probe_integration.rs::probe_unreached_is_not_executed).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Section F — Integration test

### Task F1: Apply rename to existing integration test

**Files:**
- Modify: `cli/crates/cook-cli/tests/probe_integration.rs`.

- [ ] **Step 1: Sweep `cook.add_unit({requires=...})` → `cook.add_unit({probes=...})`** in test fixture strings within the file.

```bash
grep -n "requires" cli/crates/cook-cli/tests/probe_integration.rs
```
Apply rename to every `cook.add_unit` site. `inputs.requires` on `cook.probe` stays.

- [ ] **Step 2: Run the existing test.**

```bash
cargo test -p cook-cli --test probe_integration probe_consumer_end_to_end_first_run_then_cache_hit
```
Expected: PASS.

### Task F2: Add `probe_unreached_is_not_executed` integration test

**Files:**
- Modify: `cli/crates/cook-cli/tests/probe_integration.rs`.

- [ ] **Step 1: Append the new test.** Use the existing test as a template for fixture-setup boilerplate (tempdir, write Cookfile, invoke `cook build`, inspect output dir):

```rust
#[test]
fn probe_unreached_is_not_executed() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cookfile = r#"
register
    cook.probe("demo:unused", {
        inputs = {},
        produce = [[ return { v = 1 } ]],
    })

recipe build
    > cook.sh("echo hello")
"#;
    std::fs::write(tmp.path().join("Cookfile"), cookfile).unwrap();

    let status = std::process::Command::new(env!("CARGO_BIN_EXE_cook"))
        .arg("build")
        .current_dir(tmp.path())
        .status()
        .expect("cook build");
    assert!(status.success(), "cook build must succeed");

    // No probe artifact should have been written.
    let cache_dir = tmp.path().join(".cook").join("cache");
    if cache_dir.exists() {
        // Walk for any artifact-meta sidecar with kind == "probe_value".
        let mut found_probe_artifact = false;
        for entry in walkdir::WalkDir::new(&cache_dir).into_iter().flatten() {
            if entry.path().file_name() == Some(std::ffi::OsStr::new("meta.toml"))
               || entry.path().extension() == Some(std::ffi::OsStr::new("meta"))
            {
                let content = std::fs::read_to_string(entry.path()).unwrap_or_default();
                if content.contains("kind = \"probe_value\"") || content.contains("probe_value") {
                    found_probe_artifact = true;
                    break;
                }
            }
        }
        assert!(!found_probe_artifact,
            "unreached probe must not write a probe-value artifact under .cook/cache/");
    }
    // If cache_dir doesn't exist at all, that's also a valid pass (no probe ran).
}
```

(Verify the `walkdir` dep is available in `cook-cli/Cargo.toml`'s `[dev-dependencies]`; if not, either add it or replace the walk with a recursive `read_dir` helper. The meta-file detection scheme — file name `meta.toml` vs an extension — should mirror what `cook-cache` actually writes; check `cli/crates/cook-cache/src/` for the artifact-on-disk layout if unsure.)

- [ ] **Step 2: Run the new test.**

```bash
cargo test -p cook-cli --test probe_integration probe_unreached_is_not_executed -- --nocapture
```
Expected: PASS. If the meta-detection scheme is wrong, the test will incorrectly fail — adjust the detection to match the actual on-disk layout.

- [ ] **Step 3: Run the full probe integration suite.**

```bash
cargo test -p cook-cli --test probe_integration
```
Expected: all pass.

- [ ] **Step 4: Commit section F.**

```bash
git add cli/crates/cook-cli/tests/probe_integration.rs cli/crates/cook-cli/Cargo.toml
git commit -m "$(cat <<'EOF'
test(cs-0074): integration tests — probes rename + unreached-not-executed

probe_consumer_end_to_end_first_run_then_cache_hit fixture rename to
`probes`; new probe_unreached_is_not_executed asserts a top-level probe
with no recipe-body consumer writes no probe-value artifact under
.cook/cache/.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Section G — Wrap-up

### Task G1: Full workspace sweep

- [ ] **Step 1: Build the whole workspace and run all tests** to confirm nothing else broke:

```bash
cargo build --workspace
cargo test --workspace
```
Expected: clean build, all tests pass.

- [ ] **Step 2: Run the spec-first pre-commit hook check** to confirm Standard + code stayed lockstep:

```bash
# The pre-commit hook is configured via core.hooksPath = .githooks (see CLAUDE.md).
# Manually invoke for the new commits:
.githooks/pre-commit 2>&1 | tail -20
```
Expected: hook passes (Standard updates accompanied every language-surface code commit per the spec-first rule).

- [ ] **Step 3: `git log` summary check.** Confirm the commit series tells a coherent story:

```bash
git log --oneline main..HEAD | head -20
```
Expected: roughly six commits (rename + §22.5.5-6, demand-driven + §22.5.7, App E refresh, examples, conformance, integration). Stem from `957d613` (the design-doc commit).

### Task G2: Update Linear ticket (optional)

- [ ] **Step 1:** Move SHI-214 (or whichever Linear issue tracks this work) to "In review" if the project uses Linear. Skip if no Linear ticket exists for this redesign.

---

## Self-review checklist

After the plan is fully implemented, run through this list before declaring done:

- [ ] Standard §22.5.5 uses `probes` throughout; §22.5.6 mentions `probes` for the sigil's target; §22.5.7 has the "Demand-driven scheduling" paragraph and Example 22.5.7.1.
- [ ] App. E CS-0074 entry rewritten; the v0.11 synopsis line at the top of E-changes.mdx matches.
- [ ] `CapturedUnit.requires` is gone from the codebase (`grep -rn "requires:" cli/ | grep -v "inputs.requires\|recipe.*requires"` should show no probe-related hits on add_unit).
- [ ] `cook.add_unit({requires=...})` produces a clear diagnostic in Cookfiles that haven't been migrated.
- [ ] `examples/probe-basic`, `probe-chain`, `probe-cache-share` all run end-to-end and cache-hit on second run.
- [ ] `cargo test --workspace` is green.
- [ ] `cargo test -p cook-lang --test conformance` is green.
- [ ] The new fixture `probe-unreached-pruned` exists with `parse.txt` + `register_ok.txt` baselines.
- [ ] The new integration test `probe_unreached_is_not_executed` passes.

---

## Notes for the implementing engineer

- **Spec-first hook.** CLAUDE.md mandates that every commit touching language-surface paths pairs with a Standard change. Don't bypass — the hook is configured via `git config core.hooksPath .githooks`. Each section in this plan respects that rule by bundling Standard updates with the code commit they correspond to.
- **`cook.probe.inputs.requires` is unchanged.** The rename in this plan is *only* the field on `cook.add_unit`. Be careful when sweeping — `grep -rn requires` will hit unchanged sites; don't blindly rename them.
- **Test harness baseline regeneration.** If you can't find the conformance-harness mechanism for updating `parse.txt` baselines, the harness is in `cli/crates/cook-lang/tests/conformance.rs`. Reading the top of that file will reveal the env var or flag the project uses (commonly `UPDATE_BASELINES=1` or similar).
- **Worktree.** This plan was written against the worktree at `/home/alex/dev/cook/.worktrees/probe-units` on branch `feature/probe-units`. Confirm before starting.
- **Existing tests that asserted "probe always runs."** A small number of pre-Section-B tests may have implicit assumptions that a probe in `ru.units` always becomes a DAG node. If Section B step 6 surfaces such a regression, fix the test to add a consumer (which is the correct expression of the test intent post-redesign).
- **Verification discipline.** Per CLAUDE.md and the `verification-before-completion` skill, do not mark a task complete until its verification step succeeds with the expected output captured.
