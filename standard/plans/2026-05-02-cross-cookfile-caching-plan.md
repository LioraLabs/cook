# Cross-Cookfile Caching — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Apply the design in `standard/specs/2026-05-02-cross-cookfile-caching-design.md`. Make `{alias.recipe}` body refs work across Cookfile boundaries, lock down workspace shape (tree-only imports + `//` workspace-root sigil), implement workspace-root determination, and ensure caches are correctly keyed and substituted across the import graph.

**Architecture:** Eight phases. **Phase 0** lands the spec amendments to `standard/src/content/docs/07-cross-cookfile-composition.mdx` (per the spec-first rule). **Phase 1** adds parser-level path-token validation (reject `..`/absolute, accept `//` sigil). **Phase 2** implements the workspace-root determination procedure (`--root` → `.cookroot` → tree-import inference + sigil-validation → self-root or reject). **Phase 3** wires sigil resolution and updates `Workspace::load`. **Phase 4** computes per-Cookfile `alias_dirs` maps. **Phase 5** widens codegen's recipe-name lookup set to the §7.3 union. **Phase 6** hoists `terminal_outputs` to a workspace-shared `Arc<Mutex<>>`, keyed by qualified name. **Phase 7** implements importer-side path rewriting in `cook.dep_output` and `cook.dep_output_list`. **Phase 8** lands integration tests, conformance tests, and new `verify.sh` scenarios (including a sigil-anchored fixture).

**Tech Stack:** Rust 2024 (cargo workspace at `cli/`), `mlua` (existing, Lua VM), `pathdiff` (new dep, syntactic relative-path computation), `tracing` (existing, warn diagnostics), `tempfile` (existing dev-dep), `clap` (existing, CLI parsing). Spec docs are MDX (Astro Starlight) at `standard/src/content/docs/`. Tests via `cargo test` and the existing `cargo test -p cook-lang --test conformance` harness.

---

## Working directory and prerequisites

All paths relative to `/home/alex/dev/cook` unless noted. Run from the repo root.

Confirm spec-first hook is installed:

```bash
git -C /home/alex/dev/cook config --get core.hooksPath
# Expected: .githooks
```

If empty: `git -C /home/alex/dev/cook config core.hooksPath .githooks`.

Cargo workspace root is `cli/`. Run cargo commands as:

```bash
cd /home/alex/dev/cook/cli && cargo test -p cook-lang
```

Standard build verification (Phase 0):

```bash
cd /home/alex/dev/cook/standard && pnpm build
```

Conformance harness (used in Phase 1, Phase 8):

```bash
cd /home/alex/dev/cook/cli && cargo test -p cook-lang --test conformance
```

Full workspace verification (Phases 5–8):

```bash
cd /home/alex/dev/cook/cli && cargo build -q && cargo test -q
```

End-to-end fixture (Phase 8):

```bash
bash /home/alex/dev/cook/examples/cache_benchmarks/verify.sh
bash /home/alex/dev/cook/examples/cache_benchmarks_sigil/verify.sh
```

---

## File structure

| File | Status | Responsibility | Tasks |
|---|---|---|---|
| `standard/src/content/docs/07-cross-cookfile-composition.mdx` | Modify | §7.2 path-token grammar; §7.3 lookup-set union + importer-relative substitution; §7.5 diamond clause scoped to sigils; new §7.6 workspace-root determination; new §7.7 cache portability invariants. | 0.1, 0.2, 0.3 |
| `standard/src/content/docs/A-grammar.mdx` | Modify | Grammar production for `<path>` token: tree-relative vs sigil-anchored; reject `..`/absolute. | 0.4 |
| `cli/crates/cook-lang/src/lib.rs` | Modify | Validate `ImportDecl` paths at parse time: reject `..` segments, reject absolute, accept `//` sigil. | 1.1, 1.2, 1.3 |
| `cli/crates/cook-lang/src/ast.rs` | Modify | `ImportPath` enum (`Tree(String)` / `Sigil(String)`); `ImportDecl.path` field becomes `ImportPath`. | 1.1 |
| `cli/crates/cook-cli/src/cli.rs` | Modify | Add `--root <path>` global flag. | 2.1 |
| `cli/crates/cook-cli/src/workspace.rs` | Modify | `Workspace::load` takes resolved root; new `resolve_workspace_root` fn implementing §3.1.2; sigil resolution; alias_dirs computation. | 2.2, 2.3, 2.4, 2.5, 3.1, 3.2, 4.1 |
| `cli/crates/cook-cli/src/pipeline.rs` | Modify | Call `resolve_workspace_root` before `Workspace::load`; pass `--root` through; thread shared `terminal_outputs` and `alias_dirs` into Registry construction; pass workspace-aware recipe-name union to codegen. | 2.1, 5.2, 6.5, 4.2 |
| `cli/crates/cook-luagen/src/dep_ref.rs` | Modify | Add `extract_recipe_names_with_imports` helper that builds the §7.3 union set. | 5.1 |
| `cli/crates/cook-register/src/dep_output_api.rs` | Modify | `SharedTerminalOutputs` becomes `Arc<Mutex<…>>`; `cook.dep_output` and `cook.dep_output_list` look up qualified names and rewrite paths via `alias_dirs`. | 6.1, 7.1 |
| `cli/crates/cook-register/src/engine.rs` | Modify | `Registry` accepts external `SharedTerminalOutputs`, `qualified_prefix: String`, and `alias_dirs: BTreeMap<String, PathBuf>`. Insert under qualified key after `register_recipe`. | 6.2, 6.3, 6.4 |
| `cli/crates/cook-register/src/unit_api.rs` | Modify | Update tests to use new shared-map shape. | 6.1 |
| `cli/crates/cook-engine/src/run.rs` | Modify | Construct workspace-shared `SharedTerminalOutputs` once per invocation; pass to all Registries via the registries map. | 6.5 |
| `cli/crates/cook-cli/Cargo.toml` | Modify | Add `pathdiff` dep for syntactic relative-path computation. | 4.1 |
| `cli/crates/cook-register/Cargo.toml` | Modify | Add `pathdiff` dep. | 7.1 |
| `examples/cache_benchmarks/verify.sh` | Modify | Add scenarios 14–17 (cross-Cookfile body subst; restore-on-hit across import; deep-subdir inference; `.cookroot` override). | 8.1 |
| `examples/cache_benchmarks_sigil/` | Create | New fixture: workspace with `core/lib/` and `apps/web/`; `apps/web` declares `import core //core/lib`; verify.sh exercises the sigil pathway. | 8.2 |
| `cli/crates/cook-lang/tests/conformance/imports.rs` | Modify (or Create) | Conformance tests for grammar rejections (`..`, absolute, sigil-internal `..`). | 8.3 |

---

## Phase 0: Standard amendments (spec-first)

The Cook Standard governs language changes. These tasks land the §7 amendments before any code changes touch the parser or the workspace loader. The pre-commit hook will reject code-only commits that change Cookfile surface syntax without updating the Standard, so spec amendments come first.

### Task 0.1: §7.2 path-token grammar amendment

**Files:**
- Modify: `standard/src/content/docs/07-cross-cookfile-composition.mdx`

- [ ] **Step 1: Read the current §7.2 text** to confirm starting state.

```bash
grep -n "## 7.2" /home/alex/dev/cook/standard/src/content/docs/07-cross-cookfile-composition.mdx
sed -n '12,40p' /home/alex/dev/cook/standard/src/content/docs/07-cross-cookfile-composition.mdx
```

- [ ] **Step 2: Replace §7.2's normative paragraph** with the new tree-relative + sigil grammar. After the existing "An `import` declaration has the form …" paragraph, add the constraint paragraph:

> A conforming implementation MUST validate the `<path>` token at parse time according to one of two shapes:
>
> - **Tree-relative.** `<path>` begins with `./` or with a bare segment, contains no `..` segments, and is not absolute. The path resolves relative to the directory of the enclosing Cookfile.
> - **Sigil-anchored.** `<path>` begins with `//` (the **workspace-root sigil**). The remainder of `<path>` after the sigil is forward-only (no `..`, no leading `/`). The path resolves relative to the workspace root (§7.6).
>
> A `<path>` matching neither shape MUST be rejected at parse time. Specifically:
>
> - `<path>` containing a `..` segment (in the tree-relative form) MUST be rejected. Diagnostic: `import path '<path>': '..' segments are not permitted; use the workspace-root sigil '//path' for cross-cutting imports`.
> - `<path>` that is absolute and does not begin with `//` MUST be rejected. Diagnostic: `import path '<path>': absolute paths are not permitted; tree-relative or '//' sigil`.
> - `<path>` beginning with `//` and containing a `..` segment after the sigil MUST be rejected. Diagnostic: `import path '<path>': '..' segments are not permitted after '//'`.
> - A sigil-anchored path that resolves to a directory outside the workspace root after symlink canonicalisation MUST be rejected at workspace load (§7.6). Diagnostic: `import '<name>': sigil path resolves outside workspace root '<root>'`.

Append a new normative paragraph:

> A conforming implementation MUST treat tree-relative and sigil-anchored imports uniformly for the purposes of qualified name references (§7.3), dependency implications (§5.6), and duplicate-alias detection (§7.5). The two shapes differ only in their resolution rule.

- [ ] **Step 3: Update Example 7.2.1** to demonstrate both shapes:

```cook
import backend ./services/backend       # tree-relative
import frontend ./apps/frontend         # tree-relative
import core //core/lib                  # sigil-anchored to workspace root

recipe "bundle": "backend.build" "frontend.build" "core.core_lib"
```

Update the explanatory paragraph below the example:

> Here `backend` and `frontend` are tree-relative aliases for sibling Cookfiles below the importer's directory; `core` is anchored at the workspace root via the `//` sigil and may resolve to any directory at or below the workspace root, regardless of where the importing Cookfile sits in the tree. The dependencies `"backend.build"`, `"frontend.build"`, and `"core.core_lib"` refer to recipes in those Cookfiles uniformly; the first segment before the dot selects the import alias, and the remainder is the recipe name in the imported Cookfile.

- [ ] **Step 4: Verify standard builds** locally.

Run: `cd /home/alex/dev/cook/standard && pnpm build`
Expected: clean build, no broken references.

- [ ] **Step 5: Commit**

```bash
cd /home/alex/dev/cook
git add standard/src/content/docs/07-cross-cookfile-composition.mdx
git commit -m "spec(§7.2): tree-relative + // sigil import path grammar with normative rejections"
```

### Task 0.2: §7.5 diamond clause refinement

**Files:**
- Modify: `standard/src/content/docs/07-cross-cookfile-composition.mdx`

- [ ] **Step 1: Locate §7.5** in the file.

```bash
grep -n "## 7.5" /home/alex/dev/cook/standard/src/content/docs/07-cross-cookfile-composition.mdx
```

- [ ] **Step 2: Amend the diamond paragraph.** Replace the existing "A conforming implementation MUST NOT reject the diamond case…" paragraph with:

> A conforming implementation MUST NOT reject the diamond case for **sigil-anchored imports**: if two distinct sigil-anchored imports both resolve to the same canonicalised directory, the implementation MUST load that directory once and reuse it. Tree-relative imports cannot form a diamond under §7.2 — two distinct tree-relative paths from sibling subtrees cannot canonicalise to the same directory without `..` traversal, which the §7.2 grammar forbids. Diamond deduplication for sigil imports is not a cycle.

- [ ] **Step 3: Verify standard builds.**

Run: `cd /home/alex/dev/cook/standard && pnpm build`
Expected: clean.

- [ ] **Step 4: Commit**

```bash
cd /home/alex/dev/cook
git add standard/src/content/docs/07-cross-cookfile-composition.mdx
git commit -m "spec(§7.5): scope diamond clause to sigil-anchored imports"
```

### Task 0.3: New §7.6 (workspace-root determination) and §7.7 (cache portability invariants)

**Files:**
- Modify: `standard/src/content/docs/07-cross-cookfile-composition.mdx`

- [ ] **Step 1: Append §7.6 to the file.** After the existing §7.5, add:

```markdown
## 7.6. Workspace root determination [#modules.workspace-root]

A **workspace root** is the directory against which `//`-anchored sigil imports (§7.2) resolve. A conforming implementation MUST establish the workspace root at invocation time using the following ordered procedure; the first rule that succeeds is authoritative.

1. **Explicit override.** If the implementation provides an explicit-root invocation flag (e.g., `cook --root <path>`) and one was supplied, the workspace root is `<path>` (canonicalised). The invoked Cookfile MUST be at or below `<path>`.

2. **Marker file.** Walk upward from the invoked Cookfile's directory. If any ancestor directory contains a file named `.cookroot` (zero-content sentinel), the **first** such ancestor is the workspace root.

3. **Tree-import inference.** Walk upward from the invoked Cookfile's directory. At each ancestor that contains a `Cookfile`, parse it. The ancestor is a **candidate** when both:
   1. It transitively imports the invoked Cookfile's directory through tree-relative imports only (sigil-anchored imports are ineligible at this stage because their resolution presupposes a workspace root), AND
   2. With the candidate treated as the workspace root, every sigil-anchored import in any Cookfile reachable from the candidate (across both tree-relative and sigil-anchored edges) resolves to a directory at or below the candidate after canonicalisation.

   Continue walking; the **highest** candidate is the workspace root.

4. **Self-root.** If no ancestor satisfies (1)–(3) AND neither the invoked Cookfile nor any Cookfile transitively reachable from it (through tree-relative imports) declares any sigil-anchored import, the invoked Cookfile's own directory is the workspace root.

5. **Reject.** If no ancestor satisfies (1)–(3) AND any reachable Cookfile declares a sigil-anchored import, workspace load MUST fail with a diagnostic naming the invoked Cookfile, the offending sigil import, and the inability to identify a workspace root that anchors it.

A workspace root's defining property is that it anchors sigil paths. A Cookfile that transitively reaches a sigil import therefore cannot itself be a workspace root in the absence of an enclosing anchor; the rejection in (5) makes this contract explicit.

The procedure terminates because each step walks to a strict parent directory; the filesystem root is the upper bound. Once the workspace root is determined, it is fixed for the remainder of the invocation.

### Definition: transitively imports

Cookfile *X* **directly imports** directory *D* when *X* has an `import <name> <path>` declaration whose canonicalised resolved directory equals *D*. *X* **transitively imports** *D* when there is a chain *X = X₀ → X₁ → … → Xₙ = D* where each *Xᵢ → Xᵢ₊₁* is a direct import.

## 7.7. Cache portability invariants [#modules.cache-invariants]

A conforming implementation that maintains a cache of recipe execution results MUST hold the following invariants for the cache to be portable across teammates and across workspace-relocation moves on disk.

1. **`cookfile_path` is workspace-root-relative.** For every cached recipe, the `cookfile_path` recorded in the cache metadata MUST be the importee's Cookfile path expressed as a forward-slash relative path from the workspace root, and MUST contain no absolute-path components. This invariant follows from the workspace shape established by §7.2 and §7.6.

2. **`cache_key` uniqueness is per-Cookfile-cache-file scope.** A cache entry's identity within its index file is determined by its `cache_key` (typically derived from the unit's primary output path with optional variant suffix). The `cache_key` does not encode the recipe namespace; cross-Cookfile uniqueness is provided by **per-Cookfile cache locality** (each Cookfile maintains its own cache index file under its own directory).

3. **`cache_meta.input_paths` is importer-relative.** For a unit registered by recipe R in Cookfile C with working directory W, every entry in `cache_meta.input_paths` MUST be a path relative to W. This holds for both same-Cookfile inputs and cross-Cookfile dep inputs; substitution-time path rewriting (§7.3) ensures cross-Cookfile dep paths arrive in importer-relative form. For sigil-anchored imports the path may contain `..` segments; the cache check uses `W.join(input_path)` to locate the file on disk, and `..` segments resolve correctly against W.

A conforming implementation MAY layer additional content-addressed artifact storage atop the index files; that layer is outside the scope of this section.
```

- [ ] **Step 2: Verify standard builds.**

Run: `cd /home/alex/dev/cook/standard && pnpm build`
Expected: clean.

- [ ] **Step 3: Commit**

```bash
cd /home/alex/dev/cook
git add standard/src/content/docs/07-cross-cookfile-composition.mdx
git commit -m "spec(§7.6, §7.7): workspace-root determination + cache portability invariants"
```

### Task 0.4: Grammar production update in Appendix A

**Files:**
- Modify: `standard/src/content/docs/A-grammar.mdx` (or wherever the `<path>` production lives)

- [ ] **Step 1: Locate the path-token production.**

```bash
grep -n "path\s*=\|path_token\|<path>" /home/alex/dev/cook/standard/src/content/docs/A-grammar.mdx
```

If the file is named differently, locate via:

```bash
grep -rn "import_decl\|import-decl\|<path>" /home/alex/dev/cook/standard/src/content/docs/
```

- [ ] **Step 2: Update the production** to distinguish tree-relative vs sigil-anchored. Add (or amend) a production block:

```
import_path     = sigil_path | tree_path
sigil_path      = "//" path_segments
tree_path       = ( "./" | path_segment ) ( "/" path_segment )*
path_segments   = path_segment ( "/" path_segment )*
path_segment    = (* one or more path-safe characters; MUST NOT equal ".." *)
```

The exact syntactic form depends on the existing grammar style in the file; match the surrounding conventions.

- [ ] **Step 3: Verify standard builds.**

Run: `cd /home/alex/dev/cook/standard && pnpm build`
Expected: clean.

- [ ] **Step 4: Commit**

```bash
cd /home/alex/dev/cook
git add standard/src/content/docs/A-grammar.mdx
git commit -m "spec(App. A): import_path grammar production for tree vs // sigil shapes"
```

---

## Phase 1: Parser-level path-token validation

`Token::ImportDecl { name, path }` is constructed in `cook-lang/src/lexer.rs` (line ~191) and the `ast::ImportDecl` is built in `cook-lang/src/lib.rs` (line ~148). Path validation goes in `lib.rs::parse` because it has access to the line number and parse-error infrastructure, and because validation is a parse-time concern, not a token-formation concern.

### Task 1.1: Introduce `ImportPath` enum

**Files:**
- Modify: `cli/crates/cook-lang/src/ast.rs:8`

- [ ] **Step 1: Read current `ImportDecl` struct.**

Run: `sed -n '5,15p' /home/alex/dev/cook/cli/crates/cook-lang/src/ast.rs`
Expected: `pub path: String,` field.

- [ ] **Step 2: Add `ImportPath` enum and migrate the field.**

Replace the `ImportDecl` struct definition with:

```rust
/// The shape of an import path token (§7.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportPath {
    /// Tree-relative path: forward-only, no `..`, not absolute.
    /// Resolves relative to the importing Cookfile's directory.
    Tree(String),
    /// Sigil-anchored path: begins with `//`. The stored String is the
    /// path AFTER the sigil (forward-only, no `..`, no leading `/`).
    /// Resolves relative to the workspace root.
    Sigil(String),
}

impl ImportPath {
    /// The raw string form (for diagnostics and round-trip).
    pub fn as_str(&self) -> String {
        match self {
            ImportPath::Tree(s) => s.clone(),
            ImportPath::Sigil(s) => format!("//{s}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportDecl {
    pub name: String,
    pub path: ImportPath,
    pub line: usize,
}
```

- [ ] **Step 3: Build (will fail in lib.rs).**

Run: `cd /home/alex/dev/cook/cli && cargo build -p cook-lang 2>&1 | head -30`
Expected: errors at `lib.rs:148-152` and any test sites that use `imports[0].path` as a `String`.

- [ ] **Step 4: Update `lib.rs::parse` to construct `ImportPath`** (Task 1.2 will add validation; this step just makes it compile).

Edit `cli/crates/cook-lang/src/lib.rs` lines 148-152:

```rust
imports.push(ast::ImportDecl {
    name: name.clone(),
    path: ast::ImportPath::Tree(path.clone()),  // temporary — Task 1.2 dispatches
    line: tok.line,
});
```

- [ ] **Step 5: Update existing tests** that assert against `imports[N].path`. Find them:

```bash
grep -n "imports\[.*\]\.path" /home/alex/dev/cook/cli/crates/cook-lang/src/tests.rs
grep -rn "imports\[.*\]\.path\|i\.path\b" /home/alex/dev/cook/cli/crates/
```

For each match, replace `.path` direct comparisons with `.path.as_str()`:

```rust
assert_eq!(cookfile.imports[0].path.as_str(), "./services/backend");
```

- [ ] **Step 6: Run cook-lang tests.**

Run: `cd /home/alex/dev/cook/cli && cargo test -p cook-lang`
Expected: all tests pass.

- [ ] **Step 7: Commit**

```bash
cd /home/alex/dev/cook
git add cli/crates/cook-lang/
git commit -m "ast: ImportPath enum (Tree | Sigil); migrate ImportDecl.path"
```

### Task 1.2: Reject `..` segments in tree-relative paths

**Files:**
- Modify: `cli/crates/cook-lang/src/lib.rs:148`
- Modify: `cli/crates/cook-lang/src/tests.rs`

- [ ] **Step 1: Write the failing test.**

Append to `cli/crates/cook-lang/src/tests.rs`:

```rust
#[test]
fn test_parse_import_rejects_dotdot_segment() {
    let src = "import bad ../sibling\nrecipe \"x\"\n";
    let result = cook_lang::parse(src);
    assert!(result.is_err(), "expected parse error for '..' import path");
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("'..' segments are not permitted"),
        "expected diagnostic about '..', got: {msg}"
    );
}

#[test]
fn test_parse_import_rejects_embedded_dotdot() {
    let src = "import bad ./foo/../bar\nrecipe \"x\"\n";
    let result = cook_lang::parse(src);
    assert!(result.is_err(), "expected parse error for embedded '..'");
}
```

- [ ] **Step 2: Run tests to verify failure.**

Run: `cd /home/alex/dev/cook/cli && cargo test -p cook-lang test_parse_import_rejects_dotdot`
Expected: both tests FAIL (parse currently succeeds).

- [ ] **Step 3: Add a `validate_and_classify_import_path` helper** in `lib.rs`. Add this above the `parse` function:

```rust
/// Validate an import path token and classify it as tree-relative or sigil-anchored.
/// Per §7.2, returns Err for paths containing `..`, absolute paths (other than `//` sigils),
/// and sigil paths with `..` after the sigil.
fn validate_and_classify_import_path(raw: &str, line: usize) -> Result<ast::ImportPath, ParseError> {
    if let Some(after_sigil) = raw.strip_prefix("//") {
        // Sigil-anchored. Reject `..` segments after the sigil and leading `/`.
        if after_sigil.starts_with('/') {
            return Err(ParseError::Parse {
                line,
                message: format!(
                    "import path '{raw}': '/' immediately after '//' is not permitted"
                ),
            });
        }
        if path_contains_dotdot_segment(after_sigil) {
            return Err(ParseError::Parse {
                line,
                message: format!(
                    "import path '{raw}': '..' segments are not permitted after '//'"
                ),
            });
        }
        return Ok(ast::ImportPath::Sigil(after_sigil.to_string()));
    }
    // Tree-relative. Reject absolute paths (anything starting with '/' that is not '//').
    if raw.starts_with('/') {
        return Err(ParseError::Parse {
            line,
            message: format!(
                "import path '{raw}': absolute paths are not permitted; use a tree-relative path or '//' sigil"
            ),
        });
    }
    if path_contains_dotdot_segment(raw) {
        return Err(ParseError::Parse {
            line,
            message: format!(
                "import path '{raw}': '..' segments are not permitted; use the workspace-root sigil '//path' for cross-cutting imports"
            ),
        });
    }
    Ok(ast::ImportPath::Tree(raw.to_string()))
}

/// Returns true if `path` contains a `..` segment (a `..` between path separators
/// or as the entire path or a trailing/leading segment). Does NOT match `..` inside
/// a longer segment like `..foo`.
fn path_contains_dotdot_segment(path: &str) -> bool {
    path.split('/').any(|seg| seg == "..")
}
```

- [ ] **Step 4: Wire the validation into `parse`** at line ~148:

```rust
Token::ImportDecl { name, path } => {
    if seen_recipe {
        return Err(ParseError::Parse {
            line: tok.line,
            message: "import declarations must appear before recipes and chores".to_string(),
        });
    }
    if imports.iter().any(|i: &ast::ImportDecl| i.name == *name) {
        return Err(ParseError::Parse {
            line: tok.line,
            message: format!("duplicate import name '{}'", name),
        });
    }
    let parsed_path = validate_and_classify_import_path(path, tok.line)?;
    imports.push(ast::ImportDecl {
        name: name.clone(),
        path: parsed_path,
        line: tok.line,
    });
    pos += 1;
}
```

- [ ] **Step 5: Run tests to verify pass.**

Run: `cd /home/alex/dev/cook/cli && cargo test -p cook-lang test_parse_import_rejects_dotdot`
Expected: both tests PASS.

- [ ] **Step 6: Run all cook-lang tests** to ensure no regression.

Run: `cd /home/alex/dev/cook/cli && cargo test -p cook-lang`
Expected: all tests pass. The existing `test_import_decl_relative_parent` test (lexer.rs:584) MAY now fail if it expects `../path` to be lex-valid AND parse-valid; if so, retarget it to assert the post-parse rejection (move the assertion from "lexer accepts" to "parser rejects").

- [ ] **Step 7: Commit**

```bash
cd /home/alex/dev/cook
git add cli/crates/cook-lang/
git commit -m "parse(§7.2): reject '..' segments in import paths with normative diagnostic"
```

### Task 1.3: Reject absolute paths and accept `//` sigil

**Files:**
- Modify: `cli/crates/cook-lang/src/tests.rs`

- [ ] **Step 1: Add tests for absolute-path rejection and sigil acceptance.**

Append to `cli/crates/cook-lang/src/tests.rs`:

```rust
#[test]
fn test_parse_import_rejects_absolute_path() {
    let src = "import bad /tmp/x\nrecipe \"x\"\n";
    let result = cook_lang::parse(src);
    assert!(result.is_err(), "expected parse error for absolute import path");
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("absolute paths are not permitted"),
        "expected diagnostic about absolute paths, got: {msg}"
    );
}

#[test]
fn test_parse_import_accepts_sigil() {
    let src = "import core //core/lib\nrecipe \"x\"\n";
    let cookfile = cook_lang::parse(src).expect("sigil import should parse");
    assert_eq!(cookfile.imports.len(), 1);
    match &cookfile.imports[0].path {
        ast::ImportPath::Sigil(s) => assert_eq!(s, "core/lib"),
        other => panic!("expected Sigil, got {:?}", other),
    }
}

#[test]
fn test_parse_import_rejects_sigil_with_dotdot() {
    let src = "import bad //../escape\nrecipe \"x\"\n";
    let result = cook_lang::parse(src);
    assert!(result.is_err(), "expected parse error for '..' after sigil");
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("'..' segments are not permitted after '//'"),
        "expected sigil-dotdot diagnostic, got: {msg}"
    );
}
```

- [ ] **Step 2: Run tests.**

Run: `cd /home/alex/dev/cook/cli && cargo test -p cook-lang test_parse_import`
Expected: all four tests pass (the `validate_and_classify_import_path` from Task 1.2 already handles all cases).

- [ ] **Step 3: Commit**

```bash
cd /home/alex/dev/cook
git add cli/crates/cook-lang/src/tests.rs
git commit -m "test(§7.2): absolute-path rejection + // sigil acceptance + sigil-dotdot rejection"
```

---

## Phase 2: Workspace root determination

### Task 2.1: Add `--root` CLI flag

**Files:**
- Modify: `cli/crates/cook-cli/src/cli.rs:102` (after the `pub file: PathBuf` field)
- Modify: `cli/crates/cook-cli/src/pipeline.rs` (consumer)

- [ ] **Step 1: Add the flag to `Cli` struct.** After the `pub file` field at cli.rs:105, add:

```rust
    /// Override workspace root resolution. When supplied, the workspace root is
    /// taken to be this directory; the invoked Cookfile MUST be at or below it.
    /// When omitted, the workspace root is determined per §7.6 (marker file →
    /// tree-import inference → self-root or reject).
    #[arg(long = "root")]
    pub root: Option<PathBuf>,
```

- [ ] **Step 2: Build to confirm clap accepts the new arg.**

Run: `cd /home/alex/dev/cook/cli && cargo build -p cook-cli 2>&1 | tail -5`
Expected: clean build.

- [ ] **Step 3: Smoke-test the flag is recognized.**

Run: `cd /home/alex/dev/cook/cli && cargo run -p cook-cli -- --help 2>&1 | grep -A 2 'root'`
Expected: shows `--root <ROOT>` in the help output.

- [ ] **Step 4: Commit**

```bash
cd /home/alex/dev/cook
git add cli/crates/cook-cli/src/cli.rs
git commit -m "cli: add --root flag for explicit workspace root override (§7.6 rule 1)"
```

### Task 2.2: `resolve_workspace_root` skeleton

**Files:**
- Modify: `cli/crates/cook-cli/src/workspace.rs`

- [ ] **Step 1: Write the failing test** at the bottom of `workspace.rs::tests`:

```rust
#[test]
fn test_resolve_workspace_root_marker_file_takes_precedence() {
    let dir = TempDir::new().unwrap();
    fs::create_dir_all(dir.path().join("a/b/c")).unwrap();
    fs::write(dir.path().join("a/.cookroot"), "").unwrap();
    fs::write(dir.path().join("a/Cookfile"), "import b ./b\n").unwrap();
    fs::write(dir.path().join("a/b/Cookfile"), "import c ./c\n").unwrap();
    fs::write(dir.path().join("a/b/c/Cookfile"), "recipe \"x\"\n").unwrap();

    let invoked = dir.path().join("a/b/c/Cookfile");
    let root = resolve_workspace_root(&invoked, None).unwrap();
    let expected = std::fs::canonicalize(dir.path().join("a")).unwrap();
    let got = std::fs::canonicalize(root).unwrap();
    assert_eq!(got, expected);
}

#[test]
fn test_resolve_workspace_root_explicit_override() {
    let dir = TempDir::new().unwrap();
    fs::create_dir_all(dir.path().join("lib")).unwrap();
    fs::write(dir.path().join("lib/Cookfile"), "recipe \"x\"\n").unwrap();
    fs::write(dir.path().join("Cookfile"), "import lib ./lib\n").unwrap();

    let invoked = dir.path().join("lib/Cookfile");
    let root = resolve_workspace_root(&invoked, Some(dir.path().to_path_buf())).unwrap();
    let expected = std::fs::canonicalize(dir.path()).unwrap();
    let got = std::fs::canonicalize(root).unwrap();
    assert_eq!(got, expected);
}
```

- [ ] **Step 2: Run tests to verify failure.**

Run: `cd /home/alex/dev/cook/cli && cargo test -p cook-cli test_resolve_workspace_root`
Expected: FAIL with "cannot find function `resolve_workspace_root` in this scope".

- [ ] **Step 3: Add `resolve_workspace_root`** to `workspace.rs`. Append before the `#[cfg(test)]` block:

```rust
/// Resolve the workspace root for an invocation per §7.6.
///
/// Order:
/// 1. Explicit override (`override_root`, e.g., from `--root`).
/// 2. `.cookroot` marker file walk-up.
/// 3. Tree-import inference walk-up (Task 2.3).
/// 4. Self-root (no sigils anywhere reachable) — Task 2.5.
/// 5. Reject (sigils present but no anchor found) — Task 2.5.
pub fn resolve_workspace_root(
    invoked_cookfile: &Path,
    override_root: Option<PathBuf>,
) -> Result<PathBuf, CookError> {
    // Rule 1: explicit override.
    if let Some(root) = override_root {
        let root = std::fs::canonicalize(&root).map_err(|e| {
            CookError::Other(format!("--root '{}': {e}", root.display()))
        })?;
        let invoked_canon = std::fs::canonicalize(invoked_cookfile).map_err(|e| {
            CookError::Other(format!(
                "cannot resolve {}: {e}", invoked_cookfile.display()
            ))
        })?;
        if !invoked_canon.starts_with(&root) {
            return Err(CookError::Other(format!(
                "invoked Cookfile {} is not at or below --root {}",
                invoked_canon.display(),
                root.display()
            )));
        }
        return Ok(root);
    }

    // Rule 2: .cookroot marker walk-up.
    let invoked_dir = invoked_cookfile
        .parent()
        .unwrap_or(Path::new("."))
        .to_path_buf();
    let mut cur = std::fs::canonicalize(&invoked_dir).unwrap_or(invoked_dir.clone());
    loop {
        if cur.join(".cookroot").exists() {
            return Ok(cur);
        }
        match cur.parent() {
            Some(p) => cur = p.to_path_buf(),
            None => break,
        }
    }

    // Rule 3 + 4 + 5: implemented in Tasks 2.3–2.5; for now, fall through to self-root.
    let invoked_dir_canon = std::fs::canonicalize(&invoked_dir)
        .unwrap_or(invoked_dir);
    Ok(invoked_dir_canon)
}
```

- [ ] **Step 4: Run tests to verify pass.**

Run: `cd /home/alex/dev/cook/cli && cargo test -p cook-cli test_resolve_workspace_root`
Expected: both tests PASS.

- [ ] **Step 5: Commit**

```bash
cd /home/alex/dev/cook
git add cli/crates/cook-cli/src/workspace.rs
git commit -m "workspace(§7.6): resolve_workspace_root skeleton — rules 1 (--root) + 2 (.cookroot)"
```

### Task 2.3: Tree-import inference walk

**Files:**
- Modify: `cli/crates/cook-cli/src/workspace.rs`

- [ ] **Step 1: Write the failing test.**

```rust
#[test]
fn test_resolve_workspace_root_tree_inference() {
    let dir = TempDir::new().unwrap();
    fs::create_dir_all(dir.path().join("apps/web")).unwrap();
    // Root imports apps/web; no .cookroot marker.
    fs::write(dir.path().join("Cookfile"), "import web ./apps/web\nrecipe \"x\"\n").unwrap();
    fs::write(dir.path().join("apps/web/Cookfile"), "recipe \"build\"\n").unwrap();

    let invoked = dir.path().join("apps/web/Cookfile");
    let root = resolve_workspace_root(&invoked, None).unwrap();
    let expected = std::fs::canonicalize(dir.path()).unwrap();
    let got = std::fs::canonicalize(root).unwrap();
    assert_eq!(got, expected);
}

#[test]
fn test_resolve_workspace_root_tree_inference_skip_no_cookfile_ancestor() {
    let dir = TempDir::new().unwrap();
    fs::create_dir_all(dir.path().join("intermediate/leaf")).unwrap();
    // intermediate/ has no Cookfile; root jumps directly to ./intermediate/leaf.
    fs::write(
        dir.path().join("Cookfile"),
        "import leaf ./intermediate/leaf\nrecipe \"x\"\n",
    ).unwrap();
    fs::write(dir.path().join("intermediate/leaf/Cookfile"), "recipe \"build\"\n").unwrap();

    let invoked = dir.path().join("intermediate/leaf/Cookfile");
    let root = resolve_workspace_root(&invoked, None).unwrap();
    let expected = std::fs::canonicalize(dir.path()).unwrap();
    let got = std::fs::canonicalize(root).unwrap();
    assert_eq!(got, expected);
}
```

- [ ] **Step 2: Run tests to verify failure.**

Run: `cd /home/alex/dev/cook/cli && cargo test -p cook-cli test_resolve_workspace_root_tree_inference`
Expected: FAIL — without the inference walk, `resolve_workspace_root` falls through to self-root (the invoked dir).

- [ ] **Step 3: Implement the inference walk.** In `workspace.rs`, add:

```rust
/// Returns true if `candidate_cookfile` transitively imports `target_dir` via
/// tree-relative imports only. Sigil-anchored imports are skipped (they require
/// a workspace root, which is what we are computing).
fn cookfile_transitively_imports_via_tree(
    candidate_cookfile: &Path,
    target_dir: &Path,
) -> Result<bool, CookError> {
    let target_canon = std::fs::canonicalize(target_dir)
        .unwrap_or_else(|_| target_dir.to_path_buf());

    let mut visited: HashSet<PathBuf> = HashSet::new();
    let mut stack: Vec<PathBuf> = vec![candidate_cookfile.to_path_buf()];

    while let Some(cookfile_path) = stack.pop() {
        let cookfile_canon = std::fs::canonicalize(&cookfile_path)
            .unwrap_or_else(|_| cookfile_path.clone());
        if !visited.insert(cookfile_canon.clone()) {
            continue;
        }
        let cookfile_dir = cookfile_canon.parent().unwrap_or(Path::new("."));
        let source = match std::fs::read_to_string(&cookfile_canon) {
            Ok(s) => s,
            Err(_) => continue, // can't read; assume no contribution
        };
        let parsed = match cook_lang::parse(&source) {
            Ok(p) => p,
            Err(_) => continue, // can't parse; skip
        };
        for imp in &parsed.imports {
            // Only follow tree-relative imports during root inference.
            let tree_path = match &imp.path {
                cook_lang::ast::ImportPath::Tree(s) => s,
                cook_lang::ast::ImportPath::Sigil(_) => continue,
            };
            let imp_dir = cookfile_dir.join(tree_path);
            let imp_canon = std::fs::canonicalize(&imp_dir).unwrap_or(imp_dir.clone());
            if imp_canon == target_canon {
                return Ok(true);
            }
            let nested = imp_canon.join("Cookfile");
            if nested.exists() {
                stack.push(nested);
            }
        }
    }
    Ok(false)
}
```

Then update `resolve_workspace_root` to insert the inference walk between Rule 2 and the fall-through:

```rust
    // Rule 3: tree-import inference walk.
    let invoked_dir_canon = std::fs::canonicalize(&invoked_dir)
        .unwrap_or(invoked_dir.clone());
    let mut highest: Option<PathBuf> = None;
    let mut walk_cur = invoked_dir_canon.parent().map(|p| p.to_path_buf());
    while let Some(d) = walk_cur {
        let cookfile_at_d = d.join("Cookfile");
        if cookfile_at_d.exists()
            && cookfile_transitively_imports_via_tree(&cookfile_at_d, &invoked_dir_canon)?
        {
            highest = Some(d.clone());
        }
        walk_cur = d.parent().map(|p| p.to_path_buf());
    }
    if let Some(root) = highest {
        return Ok(root);
    }

    // Rule 4 (self-root) — Task 2.5 adds the sigil-presence gate.
    Ok(invoked_dir_canon)
```

- [ ] **Step 4: Run tests.**

Run: `cd /home/alex/dev/cook/cli && cargo test -p cook-cli test_resolve_workspace_root`
Expected: all four tests PASS.

- [ ] **Step 5: Commit**

```bash
cd /home/alex/dev/cook
git add cli/crates/cook-cli/src/workspace.rs
git commit -m "workspace(§7.6 rule 3): tree-import inference walk; highest-ancestor wins"
```

### Task 2.4: Sigil-validation gate on candidates

**Files:**
- Modify: `cli/crates/cook-cli/src/workspace.rs`

- [ ] **Step 1: Write the failing test.** This exercises the case where an ancestor would otherwise be a candidate, but a sigil deeper in the workspace escapes it:

```rust
#[test]
fn test_resolve_workspace_root_skips_candidate_that_doesnt_anchor_sigils() {
    let dir = TempDir::new().unwrap();
    // Two-level tree: dir/Cookfile imports inner/, inner/Cookfile uses //top/lib.
    // dir/top/lib/ exists. dir/inner/top/ does NOT exist.
    // If we naively picked dir/inner/ as the root (it imports nothing below it
    // that uses sigils… wait — it DOES, because inner/Cookfile uses one).
    // Actually: dir/inner/Cookfile uses //top/lib. dir/inner/ as root → //top/lib
    // resolves to dir/inner/top/lib (doesn't exist). dir/ as root → dir/top/lib (exists).
    // Inference walks from invoked dir/inner/leaf/ upward; both dir/inner/ and dir/ are
    // candidates by the tree-import rule. Sigil validation under dir/inner/ fails;
    // under dir/ succeeds. Highest valid candidate = dir/.
    fs::create_dir_all(dir.path().join("top/lib")).unwrap();
    fs::create_dir_all(dir.path().join("inner/leaf")).unwrap();
    fs::write(dir.path().join("Cookfile"), "import inner ./inner\nrecipe \"x\"\n").unwrap();
    fs::write(
        dir.path().join("inner/Cookfile"),
        "import lib //top/lib\nimport leaf ./leaf\nrecipe \"y\"\n",
    ).unwrap();
    fs::write(dir.path().join("inner/leaf/Cookfile"), "recipe \"build\"\n").unwrap();
    fs::write(dir.path().join("top/lib/Cookfile"), "recipe \"q\"\n").unwrap();

    let invoked = dir.path().join("inner/leaf/Cookfile");
    let root = resolve_workspace_root(&invoked, None).unwrap();
    let expected = std::fs::canonicalize(dir.path()).unwrap();
    let got = std::fs::canonicalize(root).unwrap();
    assert_eq!(got, expected, "expected dir/ as root (anchors //top/lib), got {got:?}");
}
```

- [ ] **Step 2: Run tests to verify failure.**

Run: `cd /home/alex/dev/cook/cli && cargo test -p cook-cli test_resolve_workspace_root_skips_candidate`
Expected: FAIL — without the sigil gate, `dir/inner/` is accepted as a candidate.

- [ ] **Step 3: Add `all_reachable_sigils_resolve_under` helper:**

```rust
/// With `candidate_root` treated as the workspace root, walk every Cookfile
/// reachable from `candidate_root/Cookfile` (across both tree-relative and
/// sigil-anchored imports) and verify that every sigil-anchored import target
/// canonicalises to a directory at or below `candidate_root`.
fn all_reachable_sigils_resolve_under(candidate_root: &Path) -> Result<bool, CookError> {
    let root_canon = std::fs::canonicalize(candidate_root)
        .unwrap_or_else(|_| candidate_root.to_path_buf());
    let entry = root_canon.join("Cookfile");
    if !entry.exists() {
        // Candidate has no Cookfile — vacuously satisfies sigil resolution since
        // there's nothing to walk. (Edge case: shouldn't happen in practice
        // because the candidate was selected by transitive-import check.)
        return Ok(true);
    }

    let mut visited: HashSet<PathBuf> = HashSet::new();
    let mut stack: Vec<PathBuf> = vec![entry];

    while let Some(cookfile_path) = stack.pop() {
        let cf_canon = std::fs::canonicalize(&cookfile_path)
            .unwrap_or_else(|_| cookfile_path.clone());
        if !visited.insert(cf_canon.clone()) {
            continue;
        }
        let cf_dir = cf_canon.parent().unwrap_or(Path::new("."));
        let source = match std::fs::read_to_string(&cf_canon) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let parsed = match cook_lang::parse(&source) {
            Ok(p) => p,
            Err(_) => continue,
        };
        for imp in &parsed.imports {
            let imp_dir = match &imp.path {
                cook_lang::ast::ImportPath::Tree(s) => cf_dir.join(s),
                cook_lang::ast::ImportPath::Sigil(s) => root_canon.join(s),
            };
            let imp_canon = match std::fs::canonicalize(&imp_dir) {
                Ok(c) => c,
                Err(_) => return Ok(false), // unresolvable target → candidate fails
            };
            // Sigil targets MUST resolve under the candidate root.
            if matches!(&imp.path, cook_lang::ast::ImportPath::Sigil(_))
                && !imp_canon.starts_with(&root_canon)
            {
                return Ok(false);
            }
            let nested = imp_canon.join("Cookfile");
            if nested.exists() {
                stack.push(nested);
            }
        }
    }
    Ok(true)
}
```

- [ ] **Step 4: Gate the inference walk on the sigil check.** In `resolve_workspace_root`, replace the candidate-acceptance condition:

```rust
        if cookfile_at_d.exists()
            && cookfile_transitively_imports_via_tree(&cookfile_at_d, &invoked_dir_canon)?
            && all_reachable_sigils_resolve_under(&d)?
        {
            highest = Some(d.clone());
        }
```

- [ ] **Step 5: Run tests.**

Run: `cd /home/alex/dev/cook/cli && cargo test -p cook-cli test_resolve_workspace_root`
Expected: all five tests PASS.

- [ ] **Step 6: Commit**

```bash
cd /home/alex/dev/cook
git add cli/crates/cook-cli/src/workspace.rs
git commit -m "workspace(§7.6 rule 3): gate candidates on sigil-resolution under candidate root"
```

### Task 2.5: Self-root vs reject branch

**Files:**
- Modify: `cli/crates/cook-cli/src/workspace.rs`

- [ ] **Step 1: Write the failing test.**

```rust
#[test]
fn test_resolve_workspace_root_rejects_self_root_with_sigils() {
    let dir = TempDir::new().unwrap();
    // Standalone Cookfile that uses sigils, with no enclosing workspace.
    fs::write(
        dir.path().join("Cookfile"),
        "import top //top/lib\nrecipe \"x\"\n",
    ).unwrap();

    let invoked = dir.path().join("Cookfile");
    let result = resolve_workspace_root(&invoked, None);
    assert!(result.is_err(), "expected reject for sigil import without anchor");
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("workspace root") || msg.contains("anchor"),
        "expected reject diagnostic, got: {msg}"
    );
}

#[test]
fn test_resolve_workspace_root_self_root_no_sigils() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("Cookfile"), "recipe \"x\"\n").unwrap();

    let invoked = dir.path().join("Cookfile");
    let root = resolve_workspace_root(&invoked, None).unwrap();
    let expected = std::fs::canonicalize(dir.path()).unwrap();
    let got = std::fs::canonicalize(root).unwrap();
    assert_eq!(got, expected);
}
```

- [ ] **Step 2: Run tests.**

Run: `cd /home/alex/dev/cook/cli && cargo test -p cook-cli test_resolve_workspace_root`
Expected: `test_resolve_workspace_root_rejects_self_root_with_sigils` FAILS (currently self-roots silently); `test_resolve_workspace_root_self_root_no_sigils` PASSES.

- [ ] **Step 3: Add `any_reachable_sigil_imports` helper:**

```rust
/// Returns true if `invoked_cookfile` or any Cookfile transitively reachable
/// from it via tree-relative imports declares any sigil-anchored import.
fn any_reachable_sigil_imports(invoked_cookfile: &Path) -> Result<bool, CookError> {
    let mut visited: HashSet<PathBuf> = HashSet::new();
    let mut stack: Vec<PathBuf> = vec![invoked_cookfile.to_path_buf()];

    while let Some(cookfile_path) = stack.pop() {
        let cf_canon = std::fs::canonicalize(&cookfile_path)
            .unwrap_or_else(|_| cookfile_path.clone());
        if !visited.insert(cf_canon.clone()) {
            continue;
        }
        let cf_dir = cf_canon.parent().unwrap_or(Path::new("."));
        let source = match std::fs::read_to_string(&cf_canon) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let parsed = match cook_lang::parse(&source) {
            Ok(p) => p,
            Err(_) => continue,
        };
        for imp in &parsed.imports {
            if matches!(&imp.path, cook_lang::ast::ImportPath::Sigil(_)) {
                return Ok(true);
            }
            if let cook_lang::ast::ImportPath::Tree(s) = &imp.path {
                let imp_dir = cf_dir.join(s);
                let nested = imp_dir.join("Cookfile");
                if nested.exists() {
                    stack.push(nested);
                }
            }
        }
    }
    Ok(false)
}
```

- [ ] **Step 4: Replace the fall-through in `resolve_workspace_root`** with the rule 4 / rule 5 split:

```rust
    // Rules 4 and 5: no ancestor satisfied. Self-root if no sigils anywhere
    // reachable; reject otherwise.
    if any_reachable_sigil_imports(invoked_cookfile)? {
        return Err(CookError::Other(format!(
            "Cookfile {} declares sigil imports but no enclosing workspace root \
             could be identified. Drop a .cookroot marker at the workspace root \
             or pass --root.",
            invoked_cookfile.display(),
        )));
    }
    Ok(invoked_dir_canon)
```

- [ ] **Step 5: Run tests.**

Run: `cd /home/alex/dev/cook/cli && cargo test -p cook-cli test_resolve_workspace_root`
Expected: all seven tests PASS.

- [ ] **Step 6: Commit**

```bash
cd /home/alex/dev/cook
git add cli/crates/cook-cli/src/workspace.rs
git commit -m "workspace(§7.6 rules 4-5): self-root for sigil-free; reject when sigils need anchor"
```

### Task 2.6: Wire `resolve_workspace_root` into pipeline

**Files:**
- Modify: `cli/crates/cook-cli/src/pipeline.rs`
- Modify: `cli/crates/cook-cli/src/workspace.rs`

- [ ] **Step 1: Update `Workspace::load` signature** to accept the resolved root.

In `cli/crates/cook-cli/src/workspace.rs`, change `load` from:

```rust
pub fn load(cookfile_path: &Path, _cli_sets: &[String]) -> Result<Self, CookError> {
```

to:

```rust
pub fn load(
    cookfile_path: &Path,
    workspace_root: &Path,
    _cli_sets: &[String],
) -> Result<Self, CookError> {
```

Inside `load`, store the root in the `Workspace` struct (add a `pub root: PathBuf` field):

```rust
pub struct Workspace {
    pub root: LoadedCookfile,
    pub workspace_root: PathBuf,
    pub imports: BTreeMap<PathBuf, LoadedCookfile>,
    pub namespace_map: Vec<(PathBuf, String, PathBuf)>,
}
```

(Rename the existing `pub root: LoadedCookfile` field to `pub entry: LoadedCookfile` to free the `root` name for the workspace-root path. Update all references.)

Wait — that's a rename storm. Cleaner: keep the existing `root: LoadedCookfile` and add `workspace_root: PathBuf`:

```rust
pub struct Workspace {
    pub root: LoadedCookfile,
    pub workspace_root: PathBuf,
    pub imports: BTreeMap<PathBuf, LoadedCookfile>,
    pub namespace_map: Vec<(PathBuf, String, PathBuf)>,
}
```

Inside `load`:

```rust
        Ok(Workspace {
            root: LoadedCookfile { cookfile, lua_source, dir: root_dir },
            workspace_root: workspace_root.to_path_buf(),
            imports,
            namespace_map,
        })
```

- [ ] **Step 2: Update `Workspace::load` to resolve sigil imports** during `load_imports`. The current `load_imports` only joins paths with the importer's directory. Add the sigil branch:

In `load_imports`, replace:

```rust
            let import_dir = cookfile_dir.join(&import_decl.path);
```

with:

```rust
            let import_dir = match &import_decl.path {
                cook_lang::ast::ImportPath::Tree(p) => cookfile_dir.join(p),
                cook_lang::ast::ImportPath::Sigil(p) => workspace_root.join(p),
            };
```

You will need to thread `workspace_root: &Path` into `load_imports`. Update the recursive call site to pass it through.

Then, after canonicalisation, validate that sigil targets resolve under the workspace root:

```rust
            let canonical = std::fs::canonicalize(&import_dir).map_err(|e| {
                CookError::Other(format!(
                    "Import '{}': cannot resolve '{}': {e}",
                    import_decl.name, import_decl.path.as_str()
                ))
            })?;
            if matches!(&import_decl.path, cook_lang::ast::ImportPath::Sigil(_))
                && !canonical.starts_with(workspace_root)
            {
                return Err(CookError::Other(format!(
                    "Import '{}': sigil path resolves outside workspace root '{}'",
                    import_decl.name,
                    workspace_root.display()
                )));
            }
```

- [ ] **Step 3: Update `read_and_parse` and `Workspace::load` call sites** in `pipeline.rs`. Replace:

```rust
        let workspace = Workspace::load(&cli.file, &cli.set)?;
```

with:

```rust
        let workspace_root = crate::workspace::resolve_workspace_root(
            &cli.file,
            cli.root.clone(),
        )?;
        let workspace = Workspace::load(&cli.file, &workspace_root, &cli.set)?;
```

There are three call sites in `pipeline.rs` (lines 527, 651, and one more — find them with `grep -n "Workspace::load"`). Update each.

- [ ] **Step 4: Update `Workspace::load` callers in tests.** The tests in `workspace.rs::tests` call `Workspace::load(&dir.path().join("Cookfile"), &[])`. Update them to:

```rust
let entry = dir.path().join("Cookfile");
let root = std::fs::canonicalize(dir.path()).unwrap();
let ws = Workspace::load(&entry, &root, &[]).unwrap();
```

- [ ] **Step 5: Build + run all cook-cli tests.**

Run: `cd /home/alex/dev/cook/cli && cargo test -p cook-cli`
Expected: all tests pass. If sigil-target validation makes any prior workspace tests fail, that's a real test fix — adjust the test's directory layout to satisfy the new constraint.

- [ ] **Step 6: Commit**

```bash
cd /home/alex/dev/cook
git add cli/crates/cook-cli/
git commit -m "workspace: thread resolved workspace_root through load; resolve sigil imports"
```

---

## Phase 3: Sigil resolution and diamond/cycle detection

### Task 3.1: Sigil-aware diamond dedup

**Files:**
- Modify: `cli/crates/cook-cli/src/workspace.rs`

- [ ] **Step 1: Write the failing test.**

```rust
#[test]
fn test_diamond_via_sigil_dedups() {
    let dir = TempDir::new().unwrap();
    fs::create_dir_all(dir.path().join("shared/lib")).unwrap();
    fs::create_dir_all(dir.path().join("apps/a")).unwrap();
    fs::create_dir_all(dir.path().join("apps/b")).unwrap();
    fs::write(dir.path().join("shared/lib/Cookfile"), "recipe \"shared\"\n").unwrap();
    fs::write(
        dir.path().join("apps/a/Cookfile"),
        "import shared //shared/lib\nrecipe \"a\"\n",
    ).unwrap();
    fs::write(
        dir.path().join("apps/b/Cookfile"),
        "import shared //shared/lib\nrecipe \"b\"\n",
    ).unwrap();
    fs::write(
        dir.path().join("Cookfile"),
        "import a ./apps/a\nimport b ./apps/b\nrecipe \"top\"\n",
    ).unwrap();

    let entry = dir.path().join("Cookfile");
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let ws = Workspace::load(&entry, &root, &[]).unwrap();
    let shared_count = ws
        .imports
        .keys()
        .filter(|p| p.to_string_lossy().contains("shared/lib"))
        .count();
    assert_eq!(shared_count, 1, "shared/lib must dedup across diamond imports");
}
```

- [ ] **Step 2: Run tests.**

Run: `cd /home/alex/dev/cook/cli && cargo test -p cook-cli test_diamond_via_sigil`
Expected: PASS — the existing `imports: BTreeMap<PathBuf, _>` already dedups by canonical path. This is a regression-test commit confirming sigil-anchored diamonds work.

- [ ] **Step 3: Commit**

```bash
cd /home/alex/dev/cook
git add cli/crates/cook-cli/src/workspace.rs
git commit -m "test(§7.5): diamond dedup via sigil imports — regression coverage"
```

### Task 3.2: Cycle detection across mixed edges

**Files:**
- Modify: `cli/crates/cook-cli/src/workspace.rs`

- [ ] **Step 1: Write the failing test.**

```rust
#[test]
fn test_cycle_via_sigil_rejected() {
    let dir = TempDir::new().unwrap();
    fs::create_dir_all(dir.path().join("a")).unwrap();
    fs::create_dir_all(dir.path().join("b")).unwrap();
    fs::write(dir.path().join("a/Cookfile"), "import b //b\nrecipe \"x\"\n").unwrap();
    fs::write(dir.path().join("b/Cookfile"), "import a //a\nrecipe \"y\"\n").unwrap();
    fs::write(
        dir.path().join("Cookfile"),
        "import a ./a\nimport b ./b\nrecipe \"top\"\n",
    ).unwrap();
    fs::write(dir.path().join(".cookroot"), "").unwrap();

    let entry = dir.path().join("Cookfile");
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let result = Workspace::load(&entry, &root, &[]);
    assert!(result.is_err(), "expected cycle detection to reject");
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.to_lowercase().contains("cycle") || msg.to_lowercase().contains("circular"),
        "expected cycle diagnostic, got: {msg}"
    );
}
```

- [ ] **Step 2: Run tests.**

Run: `cd /home/alex/dev/cook/cli && cargo test -p cook-cli test_cycle_via_sigil`
Expected: PASS — the existing `visited: HashSet<PathBuf>` cycle detection in `load_imports` already operates on canonical paths and is agnostic to whether the edge was tree-relative or sigil-anchored.

If it fails, the sigil resolution path needs to feed canonicalised targets into `visited` the same way tree paths do. Inspect `load_imports` and ensure `visited.insert(canonical.clone())` runs for both shapes.

- [ ] **Step 3: Commit**

```bash
cd /home/alex/dev/cook
git add cli/crates/cook-cli/src/workspace.rs
git commit -m "test(§7.5): cycle detection across mixed tree+sigil edges — regression coverage"
```

---

## Phase 4: Per-Cookfile alias_dirs map

### Task 4.1: Compute `alias_dirs` per Cookfile

**Files:**
- Modify: `cli/crates/cook-cli/Cargo.toml`
- Modify: `cli/crates/cook-cli/src/workspace.rs`

- [ ] **Step 1: Add `pathdiff` dependency.** In `cli/crates/cook-cli/Cargo.toml`, under `[dependencies]`, add:

```toml
pathdiff = "0.2"
```

- [ ] **Step 2: Write the failing test.**

```rust
#[test]
fn test_alias_dirs_for_root_tree_import() {
    let dir = TempDir::new().unwrap();
    fs::create_dir_all(dir.path().join("lib")).unwrap();
    fs::write(dir.path().join("lib/Cookfile"), "recipe \"build\"\n").unwrap();
    fs::write(
        dir.path().join("Cookfile"),
        "import lib ./lib\nrecipe \"top\"\n",
    ).unwrap();
    fs::write(dir.path().join(".cookroot"), "").unwrap();

    let entry = dir.path().join("Cookfile");
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let ws = Workspace::load(&entry, &root, &[]).unwrap();
    let root_canon = std::fs::canonicalize(&ws.root.dir).unwrap();
    let alias_dirs = ws.alias_dirs_for(&root_canon);
    assert_eq!(alias_dirs.len(), 1);
    assert_eq!(alias_dirs.get("lib"), Some(&PathBuf::from("lib")));
}

#[test]
fn test_alias_dirs_for_sigil_import_with_dotdot() {
    let dir = TempDir::new().unwrap();
    fs::create_dir_all(dir.path().join("core/lib")).unwrap();
    fs::create_dir_all(dir.path().join("apps/web")).unwrap();
    fs::write(dir.path().join("core/lib/Cookfile"), "recipe \"core\"\n").unwrap();
    fs::write(
        dir.path().join("apps/web/Cookfile"),
        "import core //core/lib\nrecipe \"app\"\n",
    ).unwrap();
    fs::write(
        dir.path().join("Cookfile"),
        "import web ./apps/web\nrecipe \"top\"\n",
    ).unwrap();
    fs::write(dir.path().join(".cookroot"), "").unwrap();

    let entry = dir.path().join("Cookfile");
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let ws = Workspace::load(&entry, &root, &[]).unwrap();
    let web_dir = std::fs::canonicalize(dir.path().join("apps/web")).unwrap();
    let alias_dirs = ws.alias_dirs_for(&web_dir);
    assert_eq!(alias_dirs.get("core"), Some(&PathBuf::from("../../core/lib")));
}
```

- [ ] **Step 3: Run tests to verify failure.**

Run: `cd /home/alex/dev/cook/cli && cargo test -p cook-cli test_alias_dirs`
Expected: FAIL — `alias_dirs_for` doesn't exist yet.

- [ ] **Step 4: Implement `alias_dirs_for`** in `workspace.rs`:

```rust
impl Workspace {
    /// For a given importer Cookfile directory `importer_dir` (canonical),
    /// return a map from each of its import aliases to the syntactic relative
    /// path from `importer_dir` to the alias's target directory.
    ///
    /// This map is what `cook.dep_output` uses at substitution time to rewrite
    /// importee-relative paths into importer-relative paths.
    pub fn alias_dirs_for(&self, importer_dir: &Path) -> BTreeMap<String, PathBuf> {
        let mut out = BTreeMap::new();
        let importer_canon = std::fs::canonicalize(importer_dir)
            .unwrap_or_else(|_| importer_dir.to_path_buf());
        for (parent_canon, alias, target_canon) in &self.namespace_map {
            if parent_canon != &importer_canon {
                continue;
            }
            let rel = pathdiff::diff_paths(target_canon, &importer_canon)
                .unwrap_or_else(|| target_canon.clone());
            out.insert(alias.clone(), rel);
        }
        out
    }
}
```

- [ ] **Step 5: Run tests.**

Run: `cd /home/alex/dev/cook/cli && cargo test -p cook-cli test_alias_dirs`
Expected: both tests PASS.

- [ ] **Step 6: Commit**

```bash
cd /home/alex/dev/cook
git add cli/crates/cook-cli/Cargo.toml cli/crates/cook-cli/src/workspace.rs
git commit -m "workspace: alias_dirs_for(importer_dir) — importer-relative alias paths for substitution"
```

### Task 4.2: Thread `alias_dirs` into Registry construction

**Files:**
- Modify: `cli/crates/cook-cli/src/pipeline.rs`

- [ ] **Step 1: Update `build_workspace_registries`** to compute `alias_dirs` per Registry. The Registry will accept this map in Phase 6 (Task 6.3); for now, compute and pass it via a richer struct.

In `pipeline.rs`, change `build_workspace_registries` to return the Registry plus its alias_dirs:

```rust
pub struct RegistryEntry {
    pub registry: cook_register::Registry,
    pub lua_source: String,
    pub alias_dirs: BTreeMap<String, PathBuf>,
}

fn build_workspace_registries(
    workspace: &Workspace,
    config: Option<&str>,
    cli_sets: &[String],
) -> Result<BTreeMap<String, RegistryEntry>, CookError> {
    // ... existing setup ...
    let mut registries: BTreeMap<String, RegistryEntry> = BTreeMap::new();

    let root_alias_dirs = workspace.alias_dirs_for(&workspace.root.dir);
    let root_registry = cook_register::Registry::new(workspace.root.dir.clone(), root_env)
        .with_selected_config(config.map(|s| s.to_string()));
    registries.insert(
        String::new(),
        RegistryEntry {
            registry: root_registry,
            lua_source: workspace.root.lua_source.clone(),
            alias_dirs: root_alias_dirs,
        },
    );

    for (canonical_path, loaded) in &workspace.imports {
        let prefix = find_full_prefix(workspace, canonical_path);
        let import_env = resolve_env(config, std::collections::HashMap::new(), cli_sets)?;
        let alias_dirs = workspace.alias_dirs_for(&loaded.dir);
        let registry = cook_register::Registry::new(loaded.dir.clone(), import_env)
            .with_selected_config(config.map(|s| s.to_string()));
        registries.insert(
            prefix,
            RegistryEntry {
                registry,
                lua_source: loaded.lua_source.clone(),
                alias_dirs,
            },
        );
    }
    Ok(registries)
}
```

The single-Cookfile path also needs to return a `RegistryEntry` (with empty alias_dirs):

```rust
fn build_single_registries(
    cookfile_dir: &Path,
    env_vars: std::collections::HashMap<String, String>,
    lua_source: String,
    selected_config: Option<&str>,
) -> BTreeMap<String, RegistryEntry> {
    let registry = cook_register::Registry::new(cookfile_dir.to_path_buf(), env_vars)
        .with_selected_config(selected_config.map(|s| s.to_string()));
    let mut registries = BTreeMap::new();
    registries.insert(
        String::new(),
        RegistryEntry { registry, lua_source, alias_dirs: BTreeMap::new() },
    );
    registries
}
```

- [ ] **Step 2: Update consumers** of the registries map. The signature change from `BTreeMap<String, (Registry, String)>` to `BTreeMap<String, RegistryEntry>` cascades; update `run_with_progress`, `cook_engine::run::run` call sites, and any other place that pattern-matches the tuple.

Look for: `grep -n "(registry, lua_source)" cli/crates/cook-cli/src/pipeline.rs`

For each consumer, replace tuple destructuring with field access:

```rust
let entry = registries.get(&prefix).ok_or(...)?;
// entry.registry, entry.lua_source, entry.alias_dirs
```

- [ ] **Step 3: Update `cook-engine::run::run`** signature in `cli/crates/cook-engine/src/run.rs`. Currently it takes `&BTreeMap<String, (cook_register::Registry, String)>`. Change to a public `RegistryEntry` type — define it in `cook-engine` (or in `cook-contracts` if there's a shared types crate; the existing `cook-contracts` crate is the right home).

Add `pub struct RegistryEntry { pub registry: Registry, pub lua_source: String, pub alias_dirs: BTreeMap<String, PathBuf> }` to `cook-engine` (or wherever Registry already crosses crate boundaries — most likely `cook-engine` since it depends on `cook-register`).

If introducing a struct across crates is too much churn, an alternative is to leave the engine signature as a 3-tuple `(Registry, String, BTreeMap<String, PathBuf>)`. The struct is cleaner long-term; pick whichever feels right. **Author's note:** the struct shape, with named fields, is preferable for readability — choose that path unless it requires touching > 3 files.

- [ ] **Step 4: Build the workspace.**

Run: `cd /home/alex/dev/cook/cli && cargo build -q`
Expected: clean build. If type errors cascade past 3-4 sites, simplify to the 3-tuple alternative.

- [ ] **Step 5: Run tests.**

Run: `cd /home/alex/dev/cook/cli && cargo test`
Expected: all tests pass. (Registry behavior is unchanged at this point; alias_dirs is computed but unused.)

- [ ] **Step 6: Commit**

```bash
cd /home/alex/dev/cook
git add cli/crates/cook-cli/src/pipeline.rs cli/crates/cook-engine/ cli/crates/cook-register/
git commit -m "pipeline: thread per-Registry alias_dirs through pipeline + engine boundary"
```

---

## Phase 5: Codegen workspace-aware lookup set

### Task 5.1: `extract_recipe_names_with_imports` helper

**Files:**
- Modify: `cli/crates/cook-luagen/src/dep_ref.rs`

- [ ] **Step 1: Write the failing test** at the bottom of `dep_ref.rs::tests`:

```rust
#[test]
fn test_extract_recipe_names_with_imports_includes_aliased() {
    use cook_lang::ast::*;

    let lib_cookfile = make_cookfile(vec![
        make_recipe("lib_build", vec![]),
        make_recipe("lib_test", vec![]),
    ]);
    let main_cookfile = make_cookfile(vec![make_recipe("demo", vec![])]);

    let mut imports_by_alias: BTreeMap<String, &Cookfile> = BTreeMap::new();
    imports_by_alias.insert("lib".to_string(), &lib_cookfile);

    let names = extract_recipe_names_with_imports(&main_cookfile, &imports_by_alias);
    assert!(names.contains("demo"));
    assert!(names.contains("lib.lib_build"));
    assert!(names.contains("lib.lib_test"));
    assert_eq!(names.len(), 3);
}

#[test]
fn test_extract_recipe_names_with_imports_no_imports_equals_local() {
    use cook_lang::ast::*;

    let cookfile = make_cookfile(vec![make_recipe("a", vec![]), make_recipe("b", vec![])]);
    let imports_by_alias: BTreeMap<String, &Cookfile> = BTreeMap::new();
    let names = extract_recipe_names_with_imports(&cookfile, &imports_by_alias);
    let local = extract_recipe_names(&cookfile);
    assert_eq!(names, local);
}
```

- [ ] **Step 2: Run tests to verify failure.**

Run: `cd /home/alex/dev/cook/cli && cargo test -p cook-luagen test_extract_recipe_names_with_imports`
Expected: FAIL — function doesn't exist.

- [ ] **Step 3: Add the helper to `dep_ref.rs`.** After `extract_recipe_names`:

```rust
use std::collections::BTreeMap;

/// Per §7.3, the lookup set for resolving qualified name references is the
/// union of:
/// - The current Cookfile's recipe names.
/// - The set `{alias.recipe : alias is an import alias of the current Cookfile,
///   recipe is a recipe in the imported Cookfile}`.
///
/// This helper builds that union. It is non-transitive: nested-import recipes
/// (e.g., `lib.shared.recipe`) are NOT included.
pub fn extract_recipe_names_with_imports(
    cookfile: &Cookfile,
    imports_by_alias: &BTreeMap<String, &Cookfile>,
) -> BTreeSet<String> {
    let mut set: BTreeSet<String> = cookfile.recipes.iter().map(|r| r.name.clone()).collect();
    for (alias, imp) in imports_by_alias {
        for r in &imp.recipes {
            set.insert(format!("{alias}.{}", r.name));
        }
    }
    set
}
```

- [ ] **Step 4: Run tests.**

Run: `cd /home/alex/dev/cook/cli && cargo test -p cook-luagen test_extract_recipe_names_with_imports`
Expected: both tests PASS.

- [ ] **Step 5: Commit**

```bash
cd /home/alex/dev/cook
git add cli/crates/cook-luagen/src/dep_ref.rs
git commit -m "luagen(§7.3): extract_recipe_names_with_imports — workspace-aware union lookup set"
```

### Task 5.2: Wire the union into pipeline and Workspace::load

**Files:**
- Modify: `cli/crates/cook-cli/src/pipeline.rs:37,41`
- Modify: `cli/crates/cook-cli/src/workspace.rs:48,49,136-137`

- [ ] **Step 1: Update single-Cookfile codegen call** in `pipeline.rs::read_and_parse`:

The single-Cookfile path has no imports, so the union equals the local set. The existing call `extract_recipe_names(&cookfile)` is correct — leave it alone for the single-Cookfile path.

The workspace path needs a different treatment because `read_and_parse` is called BEFORE `Workspace::load` and doesn't yet know about imports. Refactor: split codegen into two phases.

In `read_and_parse`, change to:

```rust
pub fn read_and_parse(cli: &Cli) -> Result<(cook_lang::ast::Cookfile, String), CookError> {
    let source = std::fs::read_to_string(&cli.file)
        .map_err(|e| CookError::Other(format!("cannot read {}: {e}", cli.file.display())))?;

    let cookfile =
        cook_lang::parse(&source).map_err(|e| CookError::ParseError(e.to_string()))?;

    // For workspace builds, the union including imports is built later in
    // build_workspace_registries when the workspace has been loaded. For
    // single-Cookfile builds, the union equals the local recipe names.
    let recipe_names = if cookfile.imports.is_empty() {
        cook_luagen::dep_ref::extract_recipe_names(&cookfile)
    } else {
        // Workspace build — the lua_source we generate here will be
        // overwritten in build_workspace_registries with the union version.
        // Generate a placeholder using local names; it MAY be discarded.
        cook_luagen::dep_ref::extract_recipe_names(&cookfile)
    };

    let lua_source = cook_luagen::generate_with_names_checked(&cookfile, &recipe_names)
        .map_err(|e| CookError::Other(e.to_string()))?;

    let (_, warnings) =
        cook_luagen::generate_with_names_and_warnings(&cookfile, &recipe_names);
    for w in warnings {
        eprintln!("cook: warning: {}", w);
    }

    Ok((cookfile, lua_source))
}
```

- [ ] **Step 2: In `Workspace::load` and `load_imports`, regenerate `lua_source` per-Cookfile** using the §7.3 union.

In `load_imports`, **after** the recursive call returns (so `imports` for the current Cookfile's children are populated), iterate this Cookfile's import declarations and build `imports_by_alias`:

```rust
        // After all of THIS cookfile's imports have been loaded into `imports`:
        // build the alias→Cookfile map for THIS cookfile, then re-generate its
        // lua_source with the workspace-aware union.
        let mut imports_by_alias: BTreeMap<String, &cook_lang::ast::Cookfile> = BTreeMap::new();
        for import_decl in &cookfile.imports {
            let imp_dir = match &import_decl.path {
                cook_lang::ast::ImportPath::Tree(p) => cookfile_dir.join(p),
                cook_lang::ast::ImportPath::Sigil(p) => workspace_root.join(p),
            };
            let imp_canon = std::fs::canonicalize(&imp_dir)
                .unwrap_or_else(|_| imp_dir.clone());
            if let Some(loaded) = imports.get(&imp_canon) {
                imports_by_alias.insert(import_decl.name.clone(), &loaded.cookfile);
            }
        }
        // Note: the above only works inside load_imports when we have
        // a stable reference to `imports`. `cookfile` here refers to the
        // current Cookfile being processed. The actual regeneration target
        // depends on which Cookfile this iteration is for.
```

This is fiddly because `load_imports` processes children, not the parent. Restructure: after the entire workspace tree is loaded, do a second pass that regenerates each Cookfile's `lua_source` using the union.

In `Workspace::load`, AFTER `load_imports` completes, add:

```rust
        let mut workspace = Workspace {
            root: LoadedCookfile { cookfile, lua_source, dir: root_dir },
            workspace_root: workspace_root.to_path_buf(),
            imports,
            namespace_map,
        };

        // Second pass: regenerate lua_source per Cookfile using the §7.3 union
        // (local recipes + alias.recipe pairs from each Cookfile's own imports).
        regenerate_lua_sources_with_unions(&mut workspace)?;

        Ok(workspace)
```

Implement `regenerate_lua_sources_with_unions`:

```rust
fn regenerate_lua_sources_with_unions(workspace: &mut Workspace) -> Result<(), CookError> {
    // Snapshot the cookfile-by-canonical-path map for cross-references.
    let canon_to_cookfile: BTreeMap<PathBuf, cook_lang::ast::Cookfile> = workspace
        .imports
        .iter()
        .map(|(p, l)| (p.clone(), l.cookfile.clone()))
        .chain(std::iter::once((
            std::fs::canonicalize(&workspace.root.dir)
                .unwrap_or_else(|_| workspace.root.dir.clone()),
            workspace.root.cookfile.clone(),
        )))
        .collect();

    let regen = |cookfile_dir: &Path, cookfile: &cook_lang::ast::Cookfile|
        -> Result<String, CookError> {
        let mut imports_by_alias: BTreeMap<String, &cook_lang::ast::Cookfile> = BTreeMap::new();
        for imp_decl in &cookfile.imports {
            let imp_dir = match &imp_decl.path {
                cook_lang::ast::ImportPath::Tree(p) => cookfile_dir.join(p),
                cook_lang::ast::ImportPath::Sigil(p) => workspace.workspace_root.join(p),
            };
            let imp_canon = std::fs::canonicalize(&imp_dir).unwrap_or(imp_dir);
            if let Some(c) = canon_to_cookfile.get(&imp_canon) {
                imports_by_alias.insert(imp_decl.name.clone(), c);
            }
        }
        let union = cook_luagen::dep_ref::extract_recipe_names_with_imports(
            cookfile, &imports_by_alias,
        );
        cook_luagen::generate_with_names_checked(cookfile, &union)
            .map_err(|e| CookError::Other(e.to_string()))
    };

    // Regenerate root.
    let root_dir_canon = std::fs::canonicalize(&workspace.root.dir)
        .unwrap_or_else(|_| workspace.root.dir.clone());
    let new_root_lua = regen(&root_dir_canon, &workspace.root.cookfile)?;
    workspace.root.lua_source = new_root_lua;

    // Regenerate imports.
    for (canon_path, loaded) in workspace.imports.iter_mut() {
        let dir_canon = canon_path.clone();
        let new_lua = regen(&dir_canon, &loaded.cookfile)?;
        loaded.lua_source = new_lua;
    }
    Ok(())
}
```

(The lifetime juggling with `imports_by_alias: BTreeMap<String, &Cookfile>` requires owning `canon_to_cookfile` for the duration. The shape above clones `Cookfile` into the map; `Cookfile` is `Clone` per `cook-lang/src/ast.rs`. Verify.)

- [ ] **Step 3: Build.**

Run: `cd /home/alex/dev/cook/cli && cargo build -q`
Expected: clean. If `Cookfile` isn't `Clone`, derive it: edit `cli/crates/cook-lang/src/ast.rs` and add `#[derive(Clone)]` (it likely already has it).

- [ ] **Step 4: Add an integration test** that proves cross-Cookfile body subst works at the codegen layer. Append to `cli/crates/cook-luagen/src/tests.rs` or create `cli/crates/cook-cli/tests/cross_cookfile_codegen.rs`:

```rust
#[test]
fn test_workspace_codegen_emits_dep_output_for_alias_recipe() {
    use std::fs;
    use tempfile::TempDir;

    let dir = TempDir::new().unwrap();
    fs::create_dir_all(dir.path().join("lib")).unwrap();
    fs::write(
        dir.path().join("lib/Cookfile"),
        "recipe lib_build\n    cook \"build/lib.o\" using { echo {out} }\n",
    ).unwrap();
    fs::write(
        dir.path().join("Cookfile"),
        "import lib ./lib\nrecipe demo\n    cook \"build/demo\" using { echo {lib.lib_build} }\n",
    ).unwrap();
    fs::write(dir.path().join(".cookroot"), "").unwrap();

    let entry = dir.path().join("Cookfile");
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let ws = cook_cli::workspace::Workspace::load(&entry, &root, &[]).unwrap();

    // The root cookfile's lua_source should now contain `cook.dep_output("lib.lib_build")`.
    assert!(
        ws.root.lua_source.contains("cook.dep_output(\"lib.lib_build\")"),
        "expected dep_output(lib.lib_build) emission, got:\n{}",
        ws.root.lua_source
    );
}
```

(Adjust the `cook_cli::workspace::Workspace` import path to match how `cook_cli` exposes its modules. If `cook_cli` is binary-only and doesn't export library APIs, place the test inline in `pipeline.rs` or `workspace.rs::tests`.)

- [ ] **Step 5: Run the test.**

Run: `cd /home/alex/dev/cook/cli && cargo test test_workspace_codegen_emits_dep_output_for_alias_recipe`
Expected: PASS.

- [ ] **Step 6: Run the full workspace.**

Run: `cd /home/alex/dev/cook/cli && cargo test -q`
Expected: clean.

- [ ] **Step 7: Commit**

```bash
cd /home/alex/dev/cook
git add cli/crates/cook-cli/ cli/crates/cook-luagen/
git commit -m "luagen(§7.3): regenerate lua_source per-Cookfile with workspace-aware union recipe names"
```

---

## Phase 6: Workspace-shared `terminal_outputs`

### Task 6.1: Convert `SharedTerminalOutputs` to `Arc<Mutex<…>>`

**Files:**
- Modify: `cli/crates/cook-register/src/dep_output_api.rs`
- Modify: `cli/crates/cook-register/src/engine.rs`
- Modify: `cli/crates/cook-register/src/unit_api.rs` (test sites)

- [ ] **Step 1: Update the type alias** in `dep_output_api.rs:10`:

```rust
use std::sync::{Arc, Mutex};
use std::collections::BTreeMap;

/// Shared storage for terminal outputs of registered recipes, keyed by
/// **fully-qualified** recipe name (e.g., `"lib.lib_build"` or just
/// `"build"` for root-Cookfile recipes). Hoisted to workspace scope so all
/// Registries write to and read from the same map.
pub type SharedTerminalOutputs = Arc<Mutex<BTreeMap<String, Vec<String>>>>;
```

- [ ] **Step 2: Update `register_dep_output_api`** to use `.lock()` instead of `.borrow()`:

```rust
    let to = terminal_outputs.clone();
    let cs = capture_state.clone();
    let dep_output_fn = lua.create_function(move |_, name: String| {
        let store = to.lock().expect("terminal_outputs mutex poisoned");
        let outputs = store.get(&name).ok_or_else(|| {
            mlua::Error::RuntimeError(format!(
                "recipe '{}' has no terminal output (not registered or has no cook steps)",
                name
            ))
        })?;
        // (path rewriting added in Task 7.1)
        {
            let mut state = cs.borrow_mut();
            if !state.step_group_dep_refs.contains(&name) {
                state.step_group_dep_refs.push(name.clone());
            }
        }
        Ok(outputs.join(" "))
    })?;
```

Same for `dep_output_list_fn`.

- [ ] **Step 3: Update `Registry`** in `engine.rs:21` to hold `SharedTerminalOutputs` (already imported), but no longer construct it locally — Task 6.2 hoists construction. For now, change the type:

```rust
pub struct Registry {
    working_dir: PathBuf,
    env_vars: Rc<RefCell<HashMap<String, String>>>,
    export_store: SharedExportStore,
    terminal_outputs: SharedTerminalOutputs,
    selected_config: Option<String>,
}
```

(Already correct — the field already uses `SharedTerminalOutputs`. The shape change to `Arc<Mutex<>>` propagates automatically.)

In `Registry::new`, change the construction:

```rust
    pub fn new(working_dir: PathBuf, env_vars: HashMap<String, String>) -> Self {
        Self {
            working_dir,
            env_vars: Rc::new(RefCell::new(env_vars)),
            export_store: Rc::new(RefCell::new(BTreeMap::new())),
            terminal_outputs: Arc::new(Mutex::new(BTreeMap::new())),
            selected_config: None,
        }
    }
```

In `register_recipe` (line 157), change the insert:

```rust
        self.terminal_outputs
            .lock()
            .expect("terminal_outputs mutex poisoned")
            .insert(recipe_name.to_string(), terminal_outputs_list.clone());
```

- [ ] **Step 4: Update test sites** in `dep_output_api.rs::tests`:

Replace `Rc::new(RefCell::new(BTreeMap::new()))` (line 87) with:

```rust
let terminal_outputs: SharedTerminalOutputs = Arc::new(Mutex::new(BTreeMap::new()));
```

And `outputs.borrow_mut().insert(...)` becomes `outputs.lock().unwrap().insert(...)`. Same for any read sites (`outputs.borrow()`).

Apply the same updates to `unit_api.rs::tests` (lines 319, 662, 701-705).

- [ ] **Step 5: Build + run cook-register tests.**

Run: `cd /home/alex/dev/cook/cli && cargo test -p cook-register`
Expected: all tests pass.

- [ ] **Step 6: Commit**

```bash
cd /home/alex/dev/cook
git add cli/crates/cook-register/
git commit -m "register: SharedTerminalOutputs -> Arc<Mutex<...>> for workspace-scope sharing"
```

### Task 6.2: Hoist `terminal_outputs` construction to workspace level

**Files:**
- Modify: `cli/crates/cook-cli/src/pipeline.rs`
- Modify: `cli/crates/cook-register/src/engine.rs`

- [ ] **Step 1: Add a `with_shared_terminal_outputs` builder** to `Registry`:

```rust
impl Registry {
    pub fn with_shared_terminal_outputs(mut self, shared: SharedTerminalOutputs) -> Self {
        self.terminal_outputs = shared;
        self
    }
}
```

- [ ] **Step 2: In `pipeline.rs::build_workspace_registries`,** construct one shared map at the start and hand it to every Registry:

```rust
fn build_workspace_registries(
    workspace: &Workspace,
    config: Option<&str>,
    cli_sets: &[String],
) -> Result<BTreeMap<String, RegistryEntry>, CookError> {
    let dotenv_vars = load_env(&workspace.root.dir);
    let root_env = resolve_env(config, dotenv_vars, cli_sets)?;

    let shared_outputs: cook_register::SharedTerminalOutputs =
        std::sync::Arc::new(std::sync::Mutex::new(std::collections::BTreeMap::new()));

    let mut registries: BTreeMap<String, RegistryEntry> = BTreeMap::new();

    let root_alias_dirs = workspace.alias_dirs_for(&workspace.root.dir);
    let root_registry = cook_register::Registry::new(workspace.root.dir.clone(), root_env)
        .with_selected_config(config.map(|s| s.to_string()))
        .with_shared_terminal_outputs(shared_outputs.clone());
    registries.insert(
        String::new(),
        RegistryEntry {
            registry: root_registry,
            lua_source: workspace.root.lua_source.clone(),
            alias_dirs: root_alias_dirs,
        },
    );

    for (canonical_path, loaded) in &workspace.imports {
        let prefix = find_full_prefix(workspace, canonical_path);
        let import_env = resolve_env(config, std::collections::HashMap::new(), cli_sets)?;
        let alias_dirs = workspace.alias_dirs_for(&loaded.dir);
        let registry = cook_register::Registry::new(loaded.dir.clone(), import_env)
            .with_selected_config(config.map(|s| s.to_string()))
            .with_shared_terminal_outputs(shared_outputs.clone());
        registries.insert(
            prefix,
            RegistryEntry { registry, lua_source: loaded.lua_source.clone(), alias_dirs },
        );
    }
    Ok(registries)
}
```

Re-export `SharedTerminalOutputs` from `cook-register/src/lib.rs` if it isn't already:

```rust
pub use crate::dep_output_api::SharedTerminalOutputs;
```

- [ ] **Step 3: Build + run.**

Run: `cd /home/alex/dev/cook/cli && cargo build -q && cargo test -q`
Expected: clean.

- [ ] **Step 4: Commit**

```bash
cd /home/alex/dev/cook
git add cli/crates/cook-cli/src/pipeline.rs cli/crates/cook-register/
git commit -m "pipeline: hoist terminal_outputs to workspace scope; one Arc<Mutex<>> per invocation"
```

### Task 6.3: Registry accepts `qualified_prefix` for keyed insertion

**Files:**
- Modify: `cli/crates/cook-register/src/engine.rs`
- Modify: `cli/crates/cook-cli/src/pipeline.rs`

- [ ] **Step 1: Add `qualified_prefix`** to `Registry`:

```rust
pub struct Registry {
    working_dir: PathBuf,
    env_vars: Rc<RefCell<HashMap<String, String>>>,
    export_store: SharedExportStore,
    terminal_outputs: SharedTerminalOutputs,
    selected_config: Option<String>,
    qualified_prefix: String,
}
```

In `Registry::new`, default to empty:

```rust
            qualified_prefix: String::new(),
```

Add a builder:

```rust
    pub fn with_qualified_prefix(mut self, prefix: String) -> Self {
        self.qualified_prefix = prefix;
        self
    }
```

- [ ] **Step 2: Update the insert site** in `register_recipe` (line 159):

```rust
        let qualified_name = if self.qualified_prefix.is_empty() {
            recipe_name.to_string()
        } else {
            format!("{}.{}", self.qualified_prefix, recipe_name)
        };
        self.terminal_outputs
            .lock()
            .expect("terminal_outputs mutex poisoned")
            .insert(qualified_name, terminal_outputs_list.clone());
```

- [ ] **Step 3: Update `pipeline.rs`** to pass the prefix during Registry construction. In `build_workspace_registries`:

```rust
    // root has empty prefix
    let root_registry = cook_register::Registry::new(workspace.root.dir.clone(), root_env)
        .with_selected_config(config.map(|s| s.to_string()))
        .with_shared_terminal_outputs(shared_outputs.clone())
        .with_qualified_prefix(String::new());

    // imports use their full prefix (e.g., "lib", or "team.shared")
    for (canonical_path, loaded) in &workspace.imports {
        let prefix = find_full_prefix(workspace, canonical_path);
        // ...
        let registry = cook_register::Registry::new(loaded.dir.clone(), import_env)
            .with_selected_config(config.map(|s| s.to_string()))
            .with_shared_terminal_outputs(shared_outputs.clone())
            .with_qualified_prefix(prefix.clone());
        registries.insert(prefix, RegistryEntry { /* ... */ });
    }
```

- [ ] **Step 4: Add a unit test** in `engine.rs::tests` (or wherever tests live):

```rust
#[test]
fn test_register_recipe_inserts_with_qualified_prefix() {
    use std::path::PathBuf;
    let tmp = tempfile::tempdir().unwrap();
    let cookfile_path = tmp.path().join("Cookfile");
    std::fs::write(&cookfile_path, "recipe build\n").unwrap();

    let shared = std::sync::Arc::new(std::sync::Mutex::new(std::collections::BTreeMap::new()));
    let lua_source = "function R_build() end";
    let registry = Registry::new(tmp.path().to_path_buf(), HashMap::new())
        .with_shared_terminal_outputs(shared.clone())
        .with_qualified_prefix("lib".to_string());
    let _ = registry.register_recipe(lua_source, "build", None);

    let map = shared.lock().unwrap();
    assert!(map.contains_key("lib.build"), "expected key 'lib.build', got: {:?}", map.keys().collect::<Vec<_>>());
    assert!(!map.contains_key("build"), "should NOT contain bare 'build'");
}
```

(The `lua_source` minimal-stub may not work as-is; consult the existing test patterns in `tests.rs` and adapt. The intent: prove that the qualified key is what lands in the map.)

- [ ] **Step 5: Run tests.**

Run: `cd /home/alex/dev/cook/cli && cargo test -p cook-register`
Expected: clean. If the unit test's lua_source doesn't compile, simplify or use the existing mock.

- [ ] **Step 6: Commit**

```bash
cd /home/alex/dev/cook
git add cli/crates/cook-register/ cli/crates/cook-cli/src/pipeline.rs
git commit -m "register: Registry.qualified_prefix; insert under fully-qualified key after register_recipe"
```

### Task 6.4: Update `cook-engine::run::run` for the new shape

**Files:**
- Modify: `cli/crates/cook-engine/src/run.rs:178-205`

- [ ] **Step 1: Update the registries lookup** to use `RegistryEntry` (struct) field access instead of tuple destructuring. At line 178:

```rust
            let entry = registries.get(&prefix).ok_or_else(|| {
                EngineError::RegistrationFailed {
                    recipe: name.clone(),
                    message: format!("no registry for prefix '{prefix}'"),
                }
            })?;
            let registry = &entry.registry;
            let lua_source = &entry.lua_source;
```

(The struct propagation from Task 4.2 may have already touched this. Confirm the type signature of `registries: &BTreeMap<String, RegistryEntry>`.)

- [ ] **Step 2: Build + run.**

Run: `cd /home/alex/dev/cook/cli && cargo build -q && cargo test -q`
Expected: clean.

- [ ] **Step 3: Commit** (if anything changed; otherwise this task is a no-op confirming earlier work).

```bash
cd /home/alex/dev/cook
git add cli/crates/cook-engine/src/run.rs
git commit -m "engine: run() consumes RegistryEntry struct shape" --allow-empty
```

### Task 6.5: Re-test Workspace::load through to engine

**Files:** (no changes; verification step)

- [ ] **Step 1: Run the existing cache_benchmarks fixture** which already exercises one tree-relative import (lib).

Run: `bash /home/alex/dev/cook/examples/cache_benchmarks/verify.sh`
Expected: scenarios 1–13 PASS. Scenario 10 (which already exists) verifies cross-Cookfile registration; it should now also benefit from the shared map (no behavioral regression — the addendum's path was relying on the existing per-Registry map for same-Cookfile lookups).

- [ ] **Step 2: Commit if anything regressed; otherwise no-op.**

---

## Phase 7: Importer-side path rewriting

### Task 7.1: Add `alias_dirs` to `register_dep_output_api` and rewrite paths

**Files:**
- Modify: `cli/crates/cook-register/Cargo.toml`
- Modify: `cli/crates/cook-register/src/dep_output_api.rs`
- Modify: `cli/crates/cook-register/src/engine.rs`

- [ ] **Step 1: Add `pathdiff` dep** to `cli/crates/cook-register/Cargo.toml`:

```toml
[dependencies]
# ... existing ...
pathdiff = "0.2"
```

(Already added in 4.1 to cook-cli; `pathdiff` is small and adding it twice is acceptable, or hoist to workspace deps if the cargo workspace is configured for it.)

- [ ] **Step 2: Write the failing test** in `dep_output_api.rs::tests`:

```rust
#[test]
fn test_dep_output_rewrites_qualified_paths_with_alias_dir() {
    use std::path::PathBuf;
    let (lua, outputs, cs) = setup_lua();
    outputs.lock().unwrap().insert(
        "lib.lib_build".into(),
        vec!["build/lib.o".into()],
    );
    let mut alias_dirs = BTreeMap::new();
    alias_dirs.insert("lib".to_string(), PathBuf::from("lib"));

    register_dep_output_api(&lua, outputs, cs, alias_dirs).unwrap();
    let result: String = lua
        .load(r#"return cook.dep_output("lib.lib_build")"#)
        .eval()
        .unwrap();
    // Expected: "lib/build/lib.o" (alias dir prefix joined with importee path).
    assert_eq!(result, "lib/build/lib.o");
}

#[test]
fn test_dep_output_unqualified_no_rewrite() {
    let (lua, outputs, cs) = setup_lua();
    outputs.lock().unwrap().insert(
        "local_recipe".into(),
        vec!["build/local.o".into()],
    );
    register_dep_output_api(&lua, outputs, cs, BTreeMap::new()).unwrap();
    let result: String = lua
        .load(r#"return cook.dep_output("local_recipe")"#)
        .eval()
        .unwrap();
    assert_eq!(result, "build/local.o");
}

#[test]
fn test_dep_output_sigil_alias_with_dotdot() {
    use std::path::PathBuf;
    let (lua, outputs, cs) = setup_lua();
    outputs.lock().unwrap().insert(
        "core.core_lib".into(),
        vec!["build/core.o".into()],
    );
    let mut alias_dirs = BTreeMap::new();
    alias_dirs.insert("core".to_string(), PathBuf::from("../../core/lib"));

    register_dep_output_api(&lua, outputs, cs, alias_dirs).unwrap();
    let result: String = lua
        .load(r#"return cook.dep_output("core.core_lib")"#)
        .eval()
        .unwrap();
    assert_eq!(result, "../../core/lib/build/core.o");
}
```

(Note: `setup_lua` needs updating to return the new `Arc<Mutex<>>` shape — done in Task 6.1.)

- [ ] **Step 3: Run tests to verify failure.**

Run: `cd /home/alex/dev/cook/cli && cargo test -p cook-register test_dep_output`
Expected: failures because `register_dep_output_api` doesn't take `alias_dirs` yet.

- [ ] **Step 4: Update `register_dep_output_api` signature** and add path rewriting:

```rust
use std::path::PathBuf;

pub fn register_dep_output_api(
    lua: &Lua,
    terminal_outputs: SharedTerminalOutputs,
    capture_state: SharedCaptureState,
    alias_dirs: BTreeMap<String, PathBuf>,
) -> LuaResult<()> {
    let cook: LuaTable = lua.globals().get("cook")?;

    let alias_dirs = std::sync::Arc::new(alias_dirs);

    // cook.dep_output(name)
    let to = terminal_outputs.clone();
    let cs = capture_state.clone();
    let ad = alias_dirs.clone();
    let dep_output_fn = lua.create_function(move |_, name: String| {
        let store = to.lock().expect("terminal_outputs mutex poisoned");
        let outputs = store.get(&name).ok_or_else(|| {
            mlua::Error::RuntimeError(format!(
                "recipe '{}' has no terminal output (not registered or has no cook steps)",
                name
            ))
        })?;
        {
            let mut state = cs.borrow_mut();
            if !state.step_group_dep_refs.contains(&name) {
                state.step_group_dep_refs.push(name.clone());
            }
        }
        let rewritten = rewrite_paths_for_importer(&name, outputs, &ad);
        Ok(rewritten.join(" "))
    })?;
    cook.set("dep_output", dep_output_fn)?;

    // cook.dep_output_list(name)
    let to2 = terminal_outputs.clone();
    let cs2 = capture_state.clone();
    let ad2 = alias_dirs.clone();
    let dep_output_list_fn = lua.create_function(move |lua, name: String| {
        let store = to2.lock().expect("terminal_outputs mutex poisoned");
        let outputs = store.get(&name).ok_or_else(|| {
            mlua::Error::RuntimeError(format!(
                "recipe '{}' has no terminal output (not registered or has no cook steps)",
                name
            ))
        })?;
        {
            let mut state = cs2.borrow_mut();
            if !state.step_group_dep_refs.contains(&name) {
                state.step_group_dep_refs.push(name.clone());
            }
        }
        let rewritten = rewrite_paths_for_importer(&name, outputs, &ad2);
        let table = lua.create_table()?;
        for (i, path) in rewritten.iter().enumerate() {
            table.set(i + 1, path.as_str())?;
        }
        Ok(table)
    })?;
    cook.set("dep_output_list", dep_output_list_fn)?;

    Ok(())
}

/// If `name` has the form `alias.recipe`, rewrite each importee path by
/// joining with `alias_dirs[alias]` (which is the importer-relative path to
/// the alias's directory). Same-Cookfile names (no `alias.` prefix) pass
/// through unchanged.
fn rewrite_paths_for_importer(
    name: &str,
    outputs: &[String],
    alias_dirs: &BTreeMap<String, PathBuf>,
) -> Vec<String> {
    if let Some(dot) = name.find('.') {
        let alias = &name[..dot];
        if let Some(alias_dir) = alias_dirs.get(alias) {
            return outputs
                .iter()
                .map(|p| {
                    alias_dir
                        .join(p)
                        .to_string_lossy()
                        .replace(std::path::MAIN_SEPARATOR, "/")
                })
                .collect();
        }
    }
    outputs.to_vec()
}
```

- [ ] **Step 5: Update the call site** in `engine.rs:98`:

```rust
        crate::dep_output_api::register_dep_output_api(
            &lua,
            self.terminal_outputs.clone(),
            capture_state.clone(),
            self.alias_dirs.clone(),
        )?;
```

Add `alias_dirs: BTreeMap<String, PathBuf>` to `Registry`:

```rust
pub struct Registry {
    working_dir: PathBuf,
    env_vars: Rc<RefCell<HashMap<String, String>>>,
    export_store: SharedExportStore,
    terminal_outputs: SharedTerminalOutputs,
    selected_config: Option<String>,
    qualified_prefix: String,
    alias_dirs: BTreeMap<String, PathBuf>,
}
```

`Registry::new` defaults to empty; add `with_alias_dirs`:

```rust
    pub fn with_alias_dirs(mut self, alias_dirs: BTreeMap<String, PathBuf>) -> Self {
        self.alias_dirs = alias_dirs;
        self
    }
```

- [ ] **Step 6: Update `pipeline.rs`** to pass alias_dirs into Registry construction:

```rust
        let registry = cook_register::Registry::new(loaded.dir.clone(), import_env)
            .with_selected_config(config.map(|s| s.to_string()))
            .with_shared_terminal_outputs(shared_outputs.clone())
            .with_qualified_prefix(prefix.clone())
            .with_alias_dirs(alias_dirs);
```

(Same for the root Registry with `root_alias_dirs`.)

- [ ] **Step 7: Update tests** that directly call `register_dep_output_api` (in `dep_output_api.rs::tests`) to pass an empty (or populated) `alias_dirs`. The test cases written in Step 2 already pass the right shape; existing tests need an empty `BTreeMap::new()`.

- [ ] **Step 8: Run cook-register tests.**

Run: `cd /home/alex/dev/cook/cli && cargo test -p cook-register`
Expected: all tests pass.

- [ ] **Step 9: Commit**

```bash
cd /home/alex/dev/cook
git add cli/crates/cook-register/ cli/crates/cook-cli/src/pipeline.rs
git commit -m "register(§7.3): cook.dep_output rewrites paths via alias_dirs (importer-relative)"
```

---

## Phase 8: End-to-end verification

### Task 8.1: Extend `cache_benchmarks/verify.sh` with scenarios 14–17

**Files:**
- Modify: `examples/cache_benchmarks/verify.sh`

- [ ] **Step 1: Read the current verify.sh** to understand the scenario format.

```bash
sed -n '1,100p' /home/alex/dev/cook/examples/cache_benchmarks/verify.sh
```

- [ ] **Step 2: Append scenarios 14–17** to verify.sh.

Scenario 14 (cross-Cookfile body subst): assert that demo's StepEntry input_paths includes `lib/build/lib.o` after the first run.

Scenario 15 (dep input drift across import): touch `lib/src/lib.c`; assert lib.lib_build rebuilds AND demo rebuilds with `InputChanged("lib/build/lib.o")`.

Scenario 16 (workspace-root inference from deep subdir): `cd lib && cook lib_build`; assert root resolves to the parent (cookfile_path stored in cache reflects `lib/Cookfile`, not `Cookfile`).

Scenario 17 (`.cookroot` overrides inference): touch `.cookroot` at parent; rerun; assert root unchanged.

The exact bash for each scenario follows the existing pattern in verify.sh — copy the structure of scenario 10 and adapt.

- [ ] **Step 3: Run.**

Run: `bash /home/alex/dev/cook/examples/cache_benchmarks/verify.sh`
Expected: 17/17 PASS.

- [ ] **Step 4: Commit**

```bash
cd /home/alex/dev/cook
git add examples/cache_benchmarks/verify.sh
git commit -m "verify(cache_benchmarks): scenarios 14-17 — cross-Cookfile subst + drift + inference"
```

### Task 8.2: New `cache_benchmarks_sigil/` fixture

**Files:**
- Create: `examples/cache_benchmarks_sigil/Cookfile`
- Create: `examples/cache_benchmarks_sigil/.cookroot`
- Create: `examples/cache_benchmarks_sigil/core/lib/Cookfile`
- Create: `examples/cache_benchmarks_sigil/core/lib/src/core.c`
- Create: `examples/cache_benchmarks_sigil/apps/web/Cookfile`
- Create: `examples/cache_benchmarks_sigil/apps/web/src/app.c`
- Create: `examples/cache_benchmarks_sigil/verify.sh`

- [ ] **Step 1: Create the directory structure.**

```bash
mkdir -p /home/alex/dev/cook/examples/cache_benchmarks_sigil/core/lib/src
mkdir -p /home/alex/dev/cook/examples/cache_benchmarks_sigil/apps/web/src
touch /home/alex/dev/cook/examples/cache_benchmarks_sigil/.cookroot
```

- [ ] **Step 2: Write `examples/cache_benchmarks_sigil/Cookfile`:**

```
import core //core/lib
import web ./apps/web

recipe top: web.web_app
```

- [ ] **Step 3: Write `examples/cache_benchmarks_sigil/core/lib/Cookfile`:**

```
recipe core_lib
    cook "build/core.o" using { gcc -c src/core.c -o {out} }
```

- [ ] **Step 4: Write `examples/cache_benchmarks_sigil/core/lib/src/core.c`:**

```c
int core_value(void) { return 42; }
```

- [ ] **Step 5: Write `examples/cache_benchmarks_sigil/apps/web/Cookfile`:**

```
import core //core/lib

recipe web_app
    cook "build/web" using { gcc -o {out} src/app.c {core.core_lib} }
```

- [ ] **Step 6: Write `examples/cache_benchmarks_sigil/apps/web/src/app.c`:**

```c
int core_value(void);
int main(void) { return core_value() == 42 ? 0 : 1; }
```

- [ ] **Step 7: Write `verify.sh`** (skeleton — adapt the structure of `cache_benchmarks/verify.sh`):

```bash
#!/usr/bin/env bash
set -e
HERE="$(cd "$(dirname "$0")" && pwd)"
COOK="$(cd "$HERE/../../cli" && cargo build --release -q && pwd)/target/release/cook"

cd "$HERE"
rm -rf core/lib/.cook apps/web/.cook .cook build core/lib/build apps/web/build

echo "[1/4] First build: cooks core_lib then web_app"
"$COOK" -f Cookfile top
test -f core/lib/build/core.o
test -f apps/web/build/web

echo "[2/4] Second build: both should hit cache"
out=$("$COOK" -f Cookfile top 2>&1)
echo "$out" | grep -q "cached" || { echo "expected 'cached' in output"; exit 1; }

echo "[3/4] Touch core src; web should rebuild via cross-Cookfile dep input drift"
sleep 1
touch core/lib/src/core.c
"$COOK" -f Cookfile top

echo "[4/4] Sigil resolution from a deep subdir: cook from apps/web"
cd apps/web
"$COOK" web_app

echo "ALL PASS"
```

- [ ] **Step 8: chmod and run.**

```bash
chmod +x /home/alex/dev/cook/examples/cache_benchmarks_sigil/verify.sh
bash /home/alex/dev/cook/examples/cache_benchmarks_sigil/verify.sh
```

Expected: ALL PASS.

- [ ] **Step 9: Commit**

```bash
cd /home/alex/dev/cook
git add examples/cache_benchmarks_sigil/
git commit -m "fixture(cache_benchmarks_sigil): // sigil import + diamond + deep-subdir invocation"
```

### Task 8.3: Conformance tests for grammar

**Files:**
- Modify (or Create): `cli/crates/cook-lang/tests/conformance/imports.rs` (or wherever conformance tests live)

- [ ] **Step 1: Locate the conformance harness layout.**

```bash
find /home/alex/dev/cook/cli/crates/cook-lang/tests -type f | head -10
ls /home/alex/dev/cook/cli/crates/cook-lang/tests/
```

- [ ] **Step 2: Add conformance cases** for each rejection diagnostic. The exact format matches existing cases (likely `.cook` files paired with expected diagnostic snippets, or inline test functions). Examples:

```rust
// in cli/crates/cook-lang/tests/conformance.rs (or appropriate file)

#[test]
fn conformance_import_dotdot_rejected() {
    assert_parse_rejects(
        "import bad ../sibling\nrecipe \"x\"\n",
        "'..' segments are not permitted",
    );
}

#[test]
fn conformance_import_absolute_rejected() {
    assert_parse_rejects(
        "import bad /tmp/x\nrecipe \"x\"\n",
        "absolute paths are not permitted",
    );
}

#[test]
fn conformance_import_sigil_dotdot_rejected() {
    assert_parse_rejects(
        "import bad //../escape\nrecipe \"x\"\n",
        "'..' segments are not permitted after '//'",
    );
}

#[test]
fn conformance_import_sigil_accepted() {
    let cookfile = cook_lang::parse("import core //core/lib\nrecipe \"x\"\n").unwrap();
    assert_eq!(cookfile.imports.len(), 1);
    matches!(cookfile.imports[0].path, cook_lang::ast::ImportPath::Sigil(_));
}
```

If the harness uses external `.cook` snippets, mimic that pattern instead.

- [ ] **Step 3: Run the conformance harness.**

Run: `cd /home/alex/dev/cook/cli && cargo test -p cook-lang --test conformance`
Expected: all tests pass, including the new ones.

- [ ] **Step 4: Commit**

```bash
cd /home/alex/dev/cook
git add cli/crates/cook-lang/tests/
git commit -m "conformance(§7.2): import path grammar — dotdot/absolute/sigil cases"
```

### Task 8.4: Final integration sweep

**Files:** (no changes; verification step)

- [ ] **Step 1: Full workspace build + test.**

Run: `cd /home/alex/dev/cook/cli && cargo build && cargo test`
Expected: clean.

- [ ] **Step 2: Standard build.**

Run: `cd /home/alex/dev/cook/standard && pnpm build`
Expected: clean.

- [ ] **Step 3: Both verify.sh fixtures.**

Run:
```bash
bash /home/alex/dev/cook/examples/cache_benchmarks/verify.sh
bash /home/alex/dev/cook/examples/cache_benchmarks_sigil/verify.sh
```
Expected: both report ALL PASS.

- [ ] **Step 4: Pre-commit hook check.** Confirm all spec-touching commits in this branch landed alongside their code counterparts (the spec-first hook should have prevented violations during the work, but verify):

```bash
cd /home/alex/dev/cook
git log --oneline 0521063..HEAD  # all commits since the addendum's last commit
```

Expected: visible spec commits in Phase 0 before code-touching commits in Phase 1+.

- [ ] **Step 5: Final summary commit (optional).**

If anything cosmetic (CHANGELOG entry, README update) is warranted, land it now. Otherwise this task is a no-op pass/fail gate.

---

## Self-review notes

(For the planner — these are spec-coverage checkpoints, not engineer instructions.)

- §3.1 tree+sigil shape → Phases 1, 3.
- §3.1.2 root determination (rules 1-5) → Tasks 2.1–2.5.
- §3.1.4 diagnostics → Tasks 1.2, 1.3, 8.3.
- §3.1.5 diamond + tree-cant-diamond → Task 3.1, 3.2.
- §3.2 shared terminal_outputs → Phase 6.
- §3.3 importer-side rewriting → Phase 7.
- §3.4 codegen union → Phase 5.
- §3.5 invariants → Tasks 8.1, 8.2 (verified end-to-end).
- §3.6 per-iteration granularity → preserved by not changing cache_key derivation.
- §4 algorithms → Tasks 2.3, 2.4, 2.5, 4.1, 7.1.
- §5 spec amendments → Phase 0.
- §6 implementation surface → maps onto the file table at the top.
- §7 test plan → Phase 8.
- §8 backward compat → noted; existing `..`/absolute imports break is intentional, surfaces during Phase 1 conformance run.
- §9 open questions → none blocking.
