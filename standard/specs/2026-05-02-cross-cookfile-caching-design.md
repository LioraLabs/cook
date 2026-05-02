# Design: Cross-Cookfile body refs, workspace-root sigil, and caching across Cookfile boundaries

**Date:** 2026-05-02
**Status:** Design — pending implementation plan
**Standard change ID:** CS-NNNN (assigned at PR time)
**Linear epic:** SHI-140 follow-up — *Cross-Cookfile caching*
**Predecessors:**
- [2026-05-01-cache-cloud-readiness-design.md](./2026-05-01-cache-cloud-readiness-design.md)
- [2026-05-02-cache-restore-and-dep-inputs-design.md](./2026-05-02-cache-restore-and-dep-inputs-design.md)
**Scope:** `standard/src/content/docs/07-cross-cookfile-composition.mdx`, `cli/crates/cook-lang` (lexer/parser/grammar), `cli/crates/cook-luagen`, `cli/crates/cook-cli` (workspace + pipeline), `cli/crates/cook-register`, `cli/crates/cook-engine`, `examples/cache_benchmarks`.

## 1. Motivation

The 2026-05-02 cache restore-and-dep-inputs design closed three observable gaps for **single-Cookfile** caching: cross-recipe dep paths now land in `cache_meta.input_paths`, restore-on-hit recovers drifted output bytes from the artifact store, and per-output artifact keys make multi-output recipes uploadable. That work also added a `cache_benchmarks/lib/` subdirectory imported via `import lib ./lib` and verified scenarios 10–13.

While verifying scenario 10 the following additional gaps were observed:

1. **Cross-Cookfile body-ref substitution does not work.** A token `{lib.lib_build}` appearing in a parent recipe's `using` shell block falls through codegen's recipe-name lookup (which sees only the parent Cookfile's local recipe names) and lowers to `cook.env["lib.lib_build"]` (nil). The `cook.dep_output("lib.lib_build")` lowering is never emitted. §5.5 (string-substitution) and §7.3 (cross-Cookfile composition) require this lowering.

2. **Per-Registry `terminal_outputs` blocks cross-Cookfile lookup.** Each imported Cookfile gets its own `Registry` instance from `build_workspace_registries` (cook-cli/src/pipeline.rs:443). Each Registry has its own `terminal_outputs: Rc<RefCell<BTreeMap<String, Vec<String>>>>` (cook-register/src/engine.rs:21,31). When `lib.lib_build` registers, it writes to `lib`'s Registry under bare key `"lib_build"`. When the parent recipe's `cook.dep_output("lib.lib_build")` evaluates, it reads the parent Registry's map and misses (wrong map; wrong key shape).

3. **Workspace shape is under-specified.** §7.2 admits any path token, including upward (`..`) and absolute paths. Sibling-or-cousin imports through `..` confuse cache-portability assumptions and complicate workspace-root identification when `cook` is invoked from a subdirectory. The Standard does not currently specify how cross-Cookfile body-ref substitution interacts with the importer's working directory, nor how `cookfile_path` in `cloud_key` is computed across nested directory layouts.

This design closes those gaps end-to-end. It introduces a tree-shaped workspace model with an opt-in workspace-root sigil for cross-cutting libraries, a workspace-shared terminal-outputs map with importer-side path rewriting, and a normative statement of cache-portability invariants.

## 2. Non-goals

- **Cloud backend implementation.** SHI-24 still owns the network implementation. This design ensures `cookfile_path` and `cloud_key` are stable across teammates so the cloud backend drops in unchanged when ready.
- **Transitive cross-Cookfile recipe references.** §7.3 is non-transitive: the lookup set in importer A includes recipes from imported B, but **not** recipes from B's own imports. If A wants to reach a recipe in C (imported by B), B must re-export through one of its own recipes. Adding transitive references is a future spec change.
- **Module sharing across Cookfile boundaries.** §7.4 already specifies that `use` declarations are lexical per Cookfile. This design does not relax that.
- **Workspace-wide dependency analysis.** Cycle detection across import edges, duplicate-alias rejection, and per-Cookfile lookup-set computation are unchanged in spirit; only the tree-shape constraint and sigil grammar are new.
- **Artifact store schema changes.** The CAS layout at `~/.cache/cook/cloud/<aa>/<bb…>` and the `artifact_key = SHA-256(cloud_key || u32_le(idx) || path_bytes)` derivation are unchanged. No on-disk artifact format bump.

## 3. Architecture

### 3.1. Workspace shape: tree-only imports + workspace-root sigil

A **workspace** is a finite, acyclic set of Cookfiles related by `import` declarations, rooted at a single Cookfile (the **workspace root**). The Standard requires the import graph to be expressible as a tree-with-overlay:

- **Tree backbone.** Every `import` whose `<path>` does not begin with the workspace-root sigil (§3.1.1) MUST resolve to a directory at or below the importer's directory in the filesystem, expressible as a forward-only relative path. The path token MUST NOT contain `..` segments and MUST NOT be absolute. This is enforced syntactically at parse time.
- **Sigil overlay.** An `import` whose `<path>` begins with `//` is **workspace-root-anchored** (§3.1.1). Such imports may resolve to any directory at or below the workspace root, regardless of the importer's filesystem position. Sigil paths MAY participate in diamond imports (§3.1.5).
- **Strict acyclicity.** The composed import graph (tree edges ∪ sigil edges) MUST be acyclic. Cycle detection uses canonicalised paths.

#### 3.1.1. The `//` workspace-root sigil

A path token of the form `//SEGMENTS` where `SEGMENTS` is a forward-only relative path (no `..`, no leading `/`) is a **workspace-root-anchored path**. Such paths resolve as `<workspace_root>/SEGMENTS`. The workspace root is determined per §3.1.2.

```cook
import core //core/lib              # anchored at workspace root
import shared //teams/shared        # ditto, deeper path
import lib ./lib                    # tree-relative (existing behavior)
import broken ../sibling            # REJECTED — tree paths must be forward-only
import abs /tmp/x                   # REJECTED — absolute paths disallowed
```

Sigil paths interact with tree paths as follows:

- A Cookfile MAY mix tree-relative and sigil-anchored imports.
- A sigil import's resolved directory MAY itself contain Cookfiles that use sigil imports; those resolve against the same workspace root (not the directly-importing Cookfile).
- A sigil import's resolved directory MUST be at or below the workspace root after canonicalisation. Symlinks that escape the workspace root are rejected at workspace load.

#### 3.1.2. Workspace-root determination

When `cook` is invoked, the workspace root is determined by the following ordered procedure. The first rule that succeeds is authoritative:

1. **Explicit override.** If `cook --root <path>` was passed, the workspace root is `<path>` (canonicalised). The invoked Cookfile MUST be at or below `<path>`.
2. **Marker file.** Walk up from the invoked Cookfile's directory. If any ancestor directory contains a `.cookroot` file (zero-content sentinel, name normative), the **first** such ancestor is the workspace root.
3. **Tree-import inference.** Walk up from the invoked Cookfile's directory. At each ancestor that contains a `Cookfile`, parse it. If the ancestor's Cookfile transitively imports the invoked Cookfile's directory through **tree-relative imports only**, mark the ancestor as a candidate. Continue walking; the **highest** such candidate is the workspace root.
4. **Default.** If no ancestor satisfies (1)–(3), the invoked Cookfile's own directory is the workspace root.

> **Definition (transitively imports).** Cookfile X *directly imports* directory D when X has an `import <name> <path>` declaration whose canonicalised resolved directory equals D. X *transitively imports* D when there is a chain X = X₀ → X₁ → … → Xₙ = D where each Xᵢ → Xᵢ₊₁ is a direct import. For workspace-root inference (rule 3), each link in the chain MUST be tree-relative; sigil-anchored imports are ineligible because their resolution presupposes a workspace root.

The procedure terminates because each step walks to a strict parent directory; the filesystem root is the upper bound. Once the workspace root is determined, it is fixed for the remainder of the invocation.

#### 3.1.3. Standalone-subtree behaviour

When the workspace-root determination procedure picks an ancestor that does not contain `.cookroot`, but the invoked Cookfile (or any Cookfile transitively imported under it) uses sigil imports whose targets do not exist under the inferred root, the workspace fails to load with a diagnostic naming the offending sigil and the inferred root. Authors using sigil imports SHOULD drop a `.cookroot` marker at the intended workspace root to make the contract explicit.

#### 3.1.4. Diagnostic surface

- A tree-relative `<path>` containing a `..` segment is rejected at parse time. Diagnostic: `import path '<path>': '..' segments are not permitted; use the workspace-root sigil '//path' for cross-cutting imports`.
- A path that is absolute (begins with `/` and is not the sigil `//`) is rejected at parse time. Diagnostic: `import path '<path>': absolute paths are not permitted; tree-relative or '//' sigil`.
- A sigil path containing `..` after the sigil is rejected at parse time. Diagnostic: `import path '<path>': '..' segments are not permitted after '//'`.
- A sigil path that resolves to a directory outside the workspace root after symlink canonicalisation is rejected at workspace load. Diagnostic: `import '<name>': sigil path resolves outside workspace root '<root>'`.

#### 3.1.5. Diamond imports

§7.5's existing rule that two distinct imports resolving to the same canonicalised directory MUST load that directory once and reuse it remains in effect, but applies only to **sigil-anchored** imports. Tree-relative imports cannot form diamonds: under the tree constraint, two distinct tree-relative paths from sibling subtrees cannot canonicalise to the same directory without `..` traversal, which is forbidden.

### 3.2. Workspace-shared terminal outputs

The `terminal_outputs` map is hoisted from per-Registry ownership to per-workspace ownership. A single shared map is constructed when the workspace is loaded and is handed (by clone) to each Registry. Keys are **fully qualified** recipe names (`alias.recipe` or just `recipe` for the root Cookfile). Values are the recipe's terminal output paths in the importee's own view (relative to the importee's `working_dir`).

Concrete shape:

```rust
pub type SharedTerminalOutputs = Arc<Mutex<BTreeMap<String, Vec<String>>>>;
```

The shared map replaces the per-Registry `Rc<RefCell<…>>` (cook-register/src/engine.rs:21,31). `Arc<Mutex<…>>` is required because the workspace lives across the wave boundary in `cook-engine::run::run`, where future evolution may register concurrently.

Registry construction takes the shared map plus the Registry's own **fully-qualified prefix** (e.g., `"lib"` for the lib Registry, `""` for the root Registry). After `register_recipe(recipe_name)` produces a `RecipeUnits`, the Registry inserts under the qualified key `"{prefix}.{recipe_name}"` (or just `"{recipe_name}"` when prefix is empty).

### 3.3. Importer-side path rewriting at substitution

When a recipe's body contains `{alias.recipe}` (or `{recipe}` for same-Cookfile refs), codegen lowers the token to `cook.dep_output("alias.recipe")`. The `cook.dep_output` Lua API (cook-register/src/dep_output_api.rs) consults the shared `terminal_outputs` map and rewrites the importee-relative paths into **importer-relative** paths before returning them.

The rewriting rule is:

```
substituted_path[i] = relative_path_from(importer.working_dir, importee.working_dir).join(importee_outputs[i])
```

Where `relative_path_from` is the syntactic relative path computation. Under the tree constraint, this path is forward-only when the import is tree-relative. Under sigil imports, this path may contain `..` segments because the importer is not necessarily an ancestor of the importee.

The `cook.dep_output` API requires knowledge of each alias's importer-relative directory at the call site. The workspace's `namespace_map` (cook-cli/src/workspace.rs:24) already records `(parent_canonical, alias, target_canonical)` triples. The Registry receives, in addition to the shared map, an `alias_dirs: BTreeMap<String, PathBuf>` keyed by alias and valued by the alias's directory **relative to the Registry's own working_dir**.

For same-Cookfile references (no `alias.` prefix), the rewriting is a no-op: the importee is the importer.

### 3.4. Codegen recipe-names: workspace-aware lookup set

`cook_luagen::generate_with_names_checked` (called from cook-cli/src/pipeline.rs:41, cook-cli/src/workspace.rs:49,137) currently receives only the **local** Cookfile's recipe names. To emit `cook.dep_output("lib.lib_build")` instead of `cook.env["lib.lib_build"]` for `{lib.lib_build}`, the recipe-names set passed to codegen MUST be the union per §7.3:

- The local Cookfile's recipe names.
- The set `{alias.recipe : alias is an import alias of the current Cookfile, recipe is a recipe in the imported Cookfile}`.

This union is computed per-Cookfile. The root Cookfile's union includes the root's own recipes plus `{lib.recipe : recipe in lib's Cookfile}` for each `import lib ./lib`. The union does NOT include nested imports (no `lib.shared.recipe`); §7.3 is non-transitive.

Two call sites need updating:
- `cook-cli/src/pipeline.rs::read_and_parse` (currently single-Cookfile-only). When invoked under a workspace, must compute the union from the workspace's loaded imports.
- `cook-cli/src/workspace.rs::Workspace::load` and `load_imports`. Each Cookfile parsed during workspace loading needs its own union computed before `cook_luagen::generate` is called.

The accessor-split rule of §5.2 step (3) extends naturally: `{alias.recipe.ACCESSOR}` is parsed by checking whether `alias.recipe` is in the union (whole-token match), then splitting on the LAST `.` to recover the accessor.

### 3.5. Cache portability invariants

Three normative invariants make caches portable across teammates and across workspace-relocation moves on disk. All three flow from the tree-shape constraint and the workspace-root sigil semantics.

#### 3.5.1. `cookfile_path` is workspace-root-relative

`cook-register/src/engine.rs::cookfile_path_relative_to` already produces `cookfile_abs.strip_prefix(project_root)`, formatted with forward-slash separators. This string forms part of `cloud_key` derivation through `recipe_namespace = format!("{project_id}/{cookfile_path}::{recipe_name}")` (per the 2026-05-01 cache cloud-readiness design §3).

**Invariant.** For a given workspace and a given Cookfile within it, `cookfile_path` is identical across all teammates and all absolute-prefix relocations of the workspace. This holds because the workspace root is uniquely identified per §3.1.2, every Cookfile's position relative to that root is structural (forward-only path under tree imports; canonicalised position under sigil imports), and no part of the absolute filesystem path leaks into `cookfile_path`.

#### 3.5.2. `cache_key` uniqueness is per-`.bin`-file

The local cache index `<cookfile_dir>/.cook/cache/<recipe>.bin` holds `BTreeMap<cache_key, StepEntry>` where `cache_key` is built by `cook-register/src/unit_api.rs::build_local_cache_key` from `(output_paths[0], context_hash, env_contribution)` (with a `command_hash` fallback when the unit has no outputs). `cache_key` does NOT include `recipe_namespace`; it is unique only within the Cookfile that owns the `.bin` file.

**Invariant.** Cross-Cookfile uniqueness comes from per-Cookfile cache locality: each Cookfile's `<cookfile_dir>/.cook/cache/` is independent. Two recipes named `build` in different Cookfiles, each producing their own `build/foo.o`, never share an index file and never collide. Removing the per-Cookfile locality (e.g., in favour of a workspace-shared `<root>/.cook/cache/`) would require encoding `recipe_namespace` into `cache_key`. This design preserves the locality, so `cache_key` stays unchanged.

#### 3.5.3. `cache_meta.input_paths` is importer-relative

For a unit registered by recipe R in Cookfile C (working_dir W), `cache_meta.input_paths` contains paths relative to W. This holds for both same-Cookfile inputs and cross-Cookfile dep inputs; the substitution-time path rewriting (§3.3) ensures that cross-Cookfile dep paths arrive at the cache layer already in importer-relative form.

For sigil imports the path may contain `..` segments. The cache check uses `W.join(input_path)` to locate the file on disk; `..` segments resolve correctly against W. The recorded path string is reproducible across teammates because both the importer's tree-position and the importee's tree-position are workspace-root-anchored.

### 3.6. Per-iteration cache granularity

The existing per-iteration cache shape is preserved without modification. Each one-to-one cook-step iteration produces a `WorkPayload` whose `cache_meta.cache_key` is that iteration's primary output path (with optional `@<context_hash>:<env_contribution>` variant suffix). Multiple iterations of the same step register as multiple `StepEntry`s in the same `<recipe>.bin` file's inner map, each with its own `inputs[]`/`outputs[]`. If only one ingredient changes, only that iteration's `StepEntry` invalidates; sibling iterations hit and skip.

This carries over to cross-Cookfile consumption: a consuming recipe iterating over `{lib.lib_build}` (in an output pattern) drives off lib's terminal output list — each iteration sees one path from lib and registers as one work unit with its own `StepEntry`. Per-iteration granularity all the way down.

## 4. Algorithms

### 4.1. Per-Cookfile recipe-name union construction

```rust
fn union_recipe_names(
    cookfile: &Cookfile,
    aliases_to_imports: &BTreeMap<String, &Cookfile>, // alias -> imported cookfile
) -> BTreeSet<String> {
    let mut set = BTreeSet::new();
    for r in &cookfile.recipes {
        set.insert(r.name.clone());
    }
    for (alias, imp_cookfile) in aliases_to_imports {
        for r in &imp_cookfile.recipes {
            set.insert(format!("{alias}.{}", r.name));
        }
    }
    set
}
```

The map `aliases_to_imports` is built per-Cookfile from its `import` declarations and the workspace's loaded import set (`Workspace::imports`). Sigil and tree-relative imports contribute uniformly; the alias's namespace and the imported Cookfile's recipe set are all that matter.

### 4.2. Workspace-root inference walk

```rust
fn infer_workspace_root(invoked_cookfile_dir: &Path) -> Result<PathBuf, CookError> {
    // Rule 1 handled at CLI parse time (--root flag).
    // Rule 2 (.cookroot marker):
    let mut cur = invoked_cookfile_dir.to_path_buf();
    loop {
        if cur.join(".cookroot").exists() {
            return Ok(cur);
        }
        match cur.parent() {
            Some(p) => cur = p.to_path_buf(),
            None => break,
        }
    }
    // Rule 3 (tree-import inference): walk up, parse each ancestor Cookfile,
    // check whether it transitively (via tree-relative imports only) imports
    // the invoked directory. Track the highest match.
    let mut highest: Option<PathBuf> = None;
    let mut cur = invoked_cookfile_dir.parent().map(|p| p.to_path_buf());
    while let Some(d) = cur {
        if d.join("Cookfile").exists() {
            if cookfile_transitively_imports_via_tree(
                &d.join("Cookfile"),
                invoked_cookfile_dir,
            )? {
                highest = Some(d.clone());
            }
        }
        cur = d.parent().map(|p| p.to_path_buf());
    }
    // Rule 4 (default):
    Ok(highest.unwrap_or_else(|| invoked_cookfile_dir.to_path_buf()))
}
```

`cookfile_transitively_imports_via_tree` parses the candidate Cookfile, walks its tree-relative `import` declarations, and recursively checks whether any reaches the invoked directory. Sigil imports are skipped during this walk (they require a workspace root, which is what we are computing). The recursion terminates because the import graph is finite (the workspace is bounded by the filesystem) and the canonicalised-path visited set prevents revisits.

### 4.3. Substitution-time path rewriting

```rust
// In dep_output_api.rs, the cook.dep_output(name) implementation:
fn dep_output(name: &str, ctx: &SubstitutionContext) -> Vec<String> {
    let outputs = ctx.terminal_outputs.lock().unwrap()
        .get(name).cloned().unwrap_or_default();
    if let Some(dot) = name.find('.') {
        let alias = &name[..dot];
        if let Some(alias_dir) = ctx.alias_dirs.get(alias) {
            return outputs
                .into_iter()
                .map(|p| alias_dir.join(&p).to_string_lossy().into_owned())
                .collect();
        }
    }
    outputs // same-Cookfile case: paths already in importer view
}
```

`ctx.alias_dirs` is the importer Registry's view of each alias's directory, computed once at Registry construction from the workspace's `namespace_map`. For tree-relative imports, alias_dir is the forward-only relative path. For sigil imports, alias_dir is the path from the importer's working_dir to the sigil target, which may contain `..` segments.

### 4.4. Workspace-root validation against sigil targets

After workspace load, every sigil import's target directory MUST be canonicalised and verified to be at or below the workspace root. This is a single pass over `Workspace::namespace_map` filtering to sigil entries. Any entry whose canonical target is outside the root is rejected at workspace load with the §3.1.4 diagnostic.

## 5. Spec amendments

This design amends `standard/src/content/docs/07-cross-cookfile-composition.mdx` as follows.

### 5.1. §7.2 — `import <name> <path>` declarations

Add normative grammar constraints on `<path>`:

- A `<path>` token MUST be one of:
  - **Tree-relative:** begins with `./` or with a bare segment; contains no `..` segments; is not absolute.
  - **Sigil-anchored:** begins with `//`; contains no `..` segments after the sigil; is not absolute beyond the sigil.

Add the rejection rules of §3.1.4 normatively.

Replace Example 7.2.1 with a worked example showing tree-relative and sigil imports side by side.

### 5.2. §7.3 — Qualified name references

The lookup-set definition is unchanged in spirit (§7.3's existing text is correct), but add a normative cross-reference to the `//` sigil: qualified references work uniformly for tree-relative and sigil-anchored imports. The lookup set per §7.3 is constructed as in §4.1 of this design.

Add a normative paragraph: **string-substitution applies after importer-relative path rewriting** (§3.3 of this design). The substituted string is the space-joined concatenation of the importer-relative paths, not the importee's own paths.

### 5.3. §7.5 — Duplicate and cycle detection

Restate the diamond-dedup clause to apply only to sigil-anchored imports. Add a note that tree-relative imports cannot form diamonds under the §7.2 forward-only constraint.

Cycle detection across the full import graph (tree ∪ sigil) is unchanged — both kinds of edges participate.

### 5.4. New §7.6 — Workspace root determination

Add a new section specifying the procedure of §3.1.2 of this design: explicit override via `--root`, marker file `.cookroot`, tree-import inference, default. Specify that the workspace root is fixed for the duration of an invocation.

### 5.5. New §7.7 — Cache portability invariants

Add a normative section stating the three invariants of §3.5: `cookfile_path` is workspace-root-relative; `cache_key` uniqueness is per-`.bin`-file scope; `cache_meta.input_paths` is importer-relative.

## 6. Implementation surface (informative)

Modules touched:

```
cli/crates/
├── cook-lang/                    grammar + parser changes (§5.1)
│   ├── src/lexer.rs              path-token validation
│   └── src/grammar.rs            import-decl path constraints
├── cook-luagen/
│   ├── src/dep_ref.rs            extract_recipe_names_with_imports helper
│   └── src/template.rs           expand_body_token consumes union set
├── cook-cli/
│   ├── src/workspace.rs          tree-only check; sigil resolution; root inference
│   ├── src/pipeline.rs           thread shared terminal_outputs + alias_dirs
│   └── src/cli.rs                --root flag
├── cook-register/
│   ├── src/engine.rs             accept shared SharedTerminalOutputs + alias_dirs
│   └── src/dep_output_api.rs     importer-side path rewriting
└── cook-engine/
    └── src/run.rs                construct workspace-shared map; pass to Registries
```

The Lua API surface is unchanged: `cook.dep_output(name)` and `cook.dep_output_list(name)` keep their existing signatures; only their internal lookup widens.

The on-disk cache schema is unchanged: `RecipeCache::version` stays at 3. Existing `<recipe>.bin` files load and validate; cross-Cookfile cache hits become possible without migration.

## 7. Test plan

### 7.1. Standard conformance

A new conformance test suite exercises:

- `import core //core/lib` parses and resolves.
- `import bad ../sibling` rejects at parse with the §3.1.4 diagnostic.
- `import bad /tmp/x` rejects at parse with the §3.1.4 diagnostic.
- `import bad //../escape` rejects at parse with the §3.1.4 diagnostic.
- A sigil import whose target escapes the workspace root via symlink rejects at workspace load.

### 7.2. Workspace-root inference unit tests

- Marker file at intermediate ancestor → root = that ancestor.
- No marker; tree-import chain reaches invoked dir from grandparent; intermediate parent has no Cookfile → root = grandparent.
- No marker; no Cookfile above invoked dir transitively imports it → root = invoked dir's own directory.
- `--root` flag overrides both marker and inference.

### 7.3. Cross-Cookfile substitution and caching

- Single-import tree-relative case (workspace already has `cache_benchmarks/lib/`):
  - `{lib.lib_build}` substitutes to `lib/build/lib.o` in the parent's command.
  - First run: `lib.lib_build` and `demo` both execute.
  - Second run: both hit and skip.
  - Touch `lib/src/lib.c`: only `lib.lib_build` and `demo` rebuild (per §3.6).

- Sigil import case (new fixture `cache_benchmarks/sigil/`):
  - Workspace root contains `core/proto/Cookfile` and `apps/web/Cookfile`.
  - `apps/web/Cookfile` declares `import proto //core/proto`.
  - `{proto.gen}` in `apps/web` substitutes to `../../core/proto/build/proto.h`.
  - Cache check joins against `apps/web`'s working_dir; resolves correctly.

- Diamond import via sigil:
  - Workspace contains `shared/lib/`, `apps/a/`, `apps/b/`.
  - Both `apps/a/Cookfile` and `apps/b/Cookfile` declare `import shared //shared/lib`.
  - `Workspace::load` deduplicates `shared/lib/` to a single `LoadedCookfile`.

- Restore-on-hit across import boundary:
  - Variant toggle (debug↔release) on a parent recipe consuming `{lib.lib_build}`.
  - First release run: lib's release bytes uploaded under their `artifact_key`.
  - Debug run: lib's debug bytes stomp on-disk `lib/build/lib.o`.
  - Second release run: cache_key matches release entry; on-disk hash mismatches; restore-on-hit pulls release bytes from `~/.cache/cook/cloud/<artifact_key>` and skips.

### 7.4. End-to-end (cache_benchmarks)

`examples/cache_benchmarks/verify.sh` gains scenarios 14–17:

| # | Scenario | Expected |
|---|---|---|
| 14 | `{lib.lib_build}` body substitution | demo's `cache_meta.input_paths` contains `lib/build/lib.o`; second run hits. |
| 15 | Sigil-anchored fixture under `examples/cache_benchmarks_sigil/` | parses, builds, caches, replays correctly. |
| 16 | Workspace-root inference from a deep subdir | `cook` invoked in `lib/` finds the parent root via tree-import inference. |
| 17 | `.cookroot` marker overrides inference | drop a `.cookroot` mid-tree; re-run; root inference picks the marker. |

## 8. Backwards compatibility

- **Existing single-Cookfile builds:** unchanged. The recipe-names union for a Cookfile with no imports equals its local recipe names. Codegen output is identical.
- **Existing workspaces using only tree-relative imports:** unchanged at the surface. Behavioural fix: `{lib.lib_build}` style body refs now work correctly (previously degraded to `cook.env`).
- **Existing workspaces using `..` or absolute paths in imports:** REJECTED at parse with the §3.1.4 diagnostic. Migration: rewrite the import as a tree-relative path or as `//` sigil. The Standard takes a hard break here — the prior absence of constraint was a spec gap, not a feature.
- **On-disk caches:** unchanged. `CACHE_VERSION` stays at 3. Existing `.bin` files load. Cross-Cookfile cache hits start working immediately.
- **Artifact store:** unchanged. Existing artifacts remain addressable by their `artifact_key`; new uploads use the same scheme.

## 9. Open questions

None blocking implementation. One note for future work:

1. **Transitive cross-Cookfile recipe references.** §7.3 is non-transitive by deliberate choice (capability-style locality). If real workspaces hit cases where re-export through intermediate Cookfiles becomes onerous, a future spec change could introduce a syntax for explicit re-export (e.g., `export shared.proto.gen as my_proto`) without weakening the per-Cookfile lookup-set rule.

## Appendix A. Worked examples

### A.1. Tree-relative single import

```
ws/
├── Cookfile          import lib ./lib
│                     recipe demo
│                         cook "build/demo" using { gcc -o {out} main.c {lib.lib_build} }
├── lib/
│   └── Cookfile      recipe lib_build
│                         cook "build/lib.o" using { gcc -c lib.c -o {out} }
```

`{lib.lib_build}` in the root Cookfile substitutes to `lib/build/lib.o` (importer-relative). The cache check for `demo`'s unit hashes `ws/lib/build/lib.o`. `cookfile_path` for `lib_build`'s artifacts is `lib/Cookfile`.

### A.2. Sigil import for cross-cutting library

```
ws/
├── .cookroot
├── Cookfile          import core //core/lib
│                     import web ./apps/web
├── core/
│   └── lib/
│       └── Cookfile  recipe core_lib
│                         cook "build/core.o" using { gcc -c core.c -o {out} }
└── apps/
    └── web/
        └── Cookfile  import core //core/lib
                      recipe web_app
                          cook "build/web" using { gcc -o {out} app.c {core.core_lib} }
```

`{core.core_lib}` in `apps/web/Cookfile` substitutes to `../../core/lib/build/core.o` (importer-relative path with `..` segments). The cache check for `web_app`'s unit hashes `ws/apps/web/../../core/lib/build/core.o` = `ws/core/lib/build/core.o`. Both root and `apps/web` import `//core/lib`; `Workspace::load` dedupes to a single `LoadedCookfile`.

### A.3. Workspace-root inference from deep subdir

User runs `cook -f apps/web/Cookfile build` from `ws/apps/web/`. Walk-up procedure:

1. `ws/apps/web/.cookroot`? no.
2. `ws/apps/.cookroot`? no.
3. `ws/.cookroot`? yes → root = `ws/`.

`apps/web/Cookfile`'s sigil import `//core/lib` resolves to `ws/core/lib/`. Build proceeds.

If `ws/.cookroot` were absent: walk up parsing each ancestor Cookfile. `ws/apps/Cookfile` doesn't exist (skip). `ws/Cookfile` exists; tree-relative imports include `./apps/web` (transitively reaches the invoked directory) — candidate. No higher Cookfile. Inferred root = `ws/`.
