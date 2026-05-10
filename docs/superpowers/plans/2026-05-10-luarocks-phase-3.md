# SHI-176 Phase 3 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship the user-facing `cook modules` CLI subsystem on top of Phase 2's bundled LuaRocks: `cook.toml [modules]` parsing, full-closure `cook.lock` lockfile, `~/.cook/bin/luarocks` subprocess driver, runtime `package.path`/`package.cpath` extension (with the Standard §7 amendment co-landing in the same PR), `rocks.usecook.com` defaults wired in, and a `cook_smoke` throwaway rock published to validate the pipeline end-to-end.

**Architecture:** Eight vertical slices land on the integration branch `feature/luarocks-modules`. Execution is sequential through subagent-driven-development: Task 1 (M3.1) defines `ManifestModules`/`ManifestRegistry`; Task 2 (M3.2) imports them; Task 3 (M3.3) imports both; Task 4 (M3.5+M3.6 co-PR) is independent of the Rust slices and can dispatch any time after Task 1 (it touches `cook-luaotp` + Standard, not the modules tree); Task 5 (M3.7+M3.8) fills in `ManifestRegistry::default()` (left as a stub by Task 1) and authors the cook_smoke rock; Task 6 (M3.4 clap surface) integrates Tasks 1–3 and 5 into the CLI dispatch; Task 7 runs the cumulative acceptance gate. `cook pull` stays intact through Phase 3; Phase 4 cuts it over. Underscore separators for blessed module rocks (`cook_smoke`, never `cook-smoke`).

**Tech Stack:** Rust 1.x (cook-cli, cook-luaotp), `clap = "4"` derive, `toml = "0.8"`, `serde = "1"`, `sha2 = "0.10"`, `mlua` (consumed transitively), POSIX shell (`luarocks` launcher from Phase 2), Lua 5.4 (`cook_smoke.lua` source). Standard prose lives in MDX under `standard/src/content/docs/`.

**References:**
- Spec: `docs/superpowers/specs/2026-05-10-luarocks-phase-3-design.md`
- Runtime mechanics primer: `docs/architecture/dynamic-linking-and-rocks.md`
- Parent specs: `2026-05-08-luarocks-modules-design.md`, `2026-05-09-luarocks-modules-decomposition-design.md`, `2026-05-09-luarocks-phase-2-design.md`

---

## Pre-task setup (one-time, before any slice)

- [ ] **S0.1: Sync the integration branch to main**

Phase 2 merged to `main` (commit `ec32ad9`). The local `feature/luarocks-modules` branch is behind. Refresh it so Phase 3 work starts from the latest tip.

```bash
cd /home/alex/dev/cook
git fetch
git checkout main
git pull
git branch -D feature/luarocks-modules || true
git push origin --delete feature/luarocks-modules || true
git switch -c feature/luarocks-modules
git push -u origin feature/luarocks-modules
```

Expected: `feature/luarocks-modules` is at `main`'s HEAD on both local and origin.

- [ ] **S0.2: Scaffold `cli/crates/cook-cli/src/modules/`**

Three of the four Wave-A Rust slices share `cli/crates/cook-cli/src/modules/`. Pre-creating the facade + stub files prevents `mod.rs`-level merge conflicts when slices land out of order. Each Wave-A slice fills in its own file; nothing wires through to `cli.rs` or `main.rs` yet — that is M3.4's territory.

Create `cli/crates/cook-cli/src/modules/mod.rs`:

```rust
//! `cook modules` — manifest, lockfile, and LuaRocks subprocess driver.
//!
//! Phase 3 surface (SHI-176). Each submodule is owned by one slice:
//!   - `manifest`  — M3.1: cook.toml `[modules]` and `[registry].indexes` parsing.
//!   - `lockfile`  — M3.2: cook.lock format, integrity verification, closure introspection.
//!   - `driver`    — M3.3: `~/.cook/bin/luarocks` subprocess wrapper.
//!   - `cli`       — M3.4: clap subcommand wiring; consumes the three above.
//!
//! Shared invariant: `BTreeMap`/`BTreeSet` for any serialised collection
//! (deterministic output per project conventions).

pub mod driver;
pub mod lockfile;
pub mod manifest;
```

Create stub `cli/crates/cook-cli/src/modules/manifest.rs`:

```rust
//! M3.1 stub. Owned by Task 1.
```

Create stub `cli/crates/cook-cli/src/modules/lockfile.rs`:

```rust
//! M3.2 stub. Owned by Task 2.
```

Create stub `cli/crates/cook-cli/src/modules/driver.rs`:

```rust
//! M3.3 stub. Owned by Task 3.
```

- [ ] **S0.3: Wire the new module into `cli/crates/cook-cli/src/lib.rs`**

The library surface is what Phase 3's integration tests will reach into.

Edit `cli/crates/cook-cli/src/lib.rs`. Find:

```rust
pub mod pull;
```

Replace with:

```rust
pub mod modules;
pub mod pull;
```

- [ ] **S0.4: Verify the scaffolding builds**

```bash
cd /home/alex/dev/cook/cli && cargo build -p cook-cli 2>&1 | tail -5
```

Expected: clean build, no warnings about the new modules. The stub files have no public types yet, so nothing else needs to change.

- [ ] **S0.5: Commit the scaffold**

```bash
cd /home/alex/dev/cook
git add cli/crates/cook-cli/src/modules/ cli/crates/cook-cli/src/lib.rs
git commit -m "scaffold(phase3): cook-cli modules/ subtree (M3.1-M3.4 ownership stubs)"
```

---

## Task 1 (M3.1): `cook.toml [modules]` + `[registry].indexes` parsing

**Files:**
- Modify: `cli/crates/cook-cli/src/modules/manifest.rs` (replace stub)
- Modify: `cli/crates/cook-cli/Cargo.toml` (no new deps; `toml` and `serde` are already listed)

**Slice boundary:** Owns `manifest.rs`. Does NOT touch `lockfile.rs` (Task 2), `driver.rs` (Task 3), `cli.rs` (Task 6). Leaves `ManifestRegistry::default()` returning `Vec::new()` — Task 5 fills in the rocks.usecook.com defaults. Does NOT touch `cli/crates/cook-cli/src/pull/` (Phase 4 territory).

### TDD: parser tests first

- [ ] **T1.1: Replace the stub with module skeleton + types**

Replace the contents of `cli/crates/cook-cli/src/modules/manifest.rs`:

```rust
//! M3.1 — `cook.toml` `[modules]` and `[registry].indexes` parsing.
//!
//! `[modules]` is a flat TOML table mapping rock names to luarocks version
//! constraints. Cook does not invent constraint grammar — values pass through
//! to luarocks verbatim. Rock names use luarocks's allowed character set
//! (`[A-Za-z][A-Za-z0-9_.\-]*`); cook uses underscore-separated names for
//! its blessed `cook_*` modules so they are valid Lua identifiers and bare
//! TOML keys.
//!
//! `[registry].indexes` is the new Phase 3 array distinct from the legacy
//! Phase 1 `[registry].url` (which `cook pull` still consumes). Empty or
//! missing `indexes` falls through to `ManifestRegistry::default()`, which
//! M3.7 fills in with `["https://rocks.usecook.com", "https://luarocks.org"]`.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ManifestModules {
    pub modules: BTreeMap<String, String>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ManifestRegistry {
    pub indexes: Vec<String>,
}

impl ManifestRegistry {
    /// Default index list when `[registry].indexes` is missing or empty.
    /// M3.7 (Task 5) populates the constants. Until then, returns an empty
    /// vec — callers fall back to passing `--server` flags from CLI args.
    pub fn default() -> Self {
        // Task 5 replaces this with:
        //   Self {
        //       indexes: vec![
        //           "https://rocks.usecook.com".to_string(),
        //           "https://luarocks.org".to_string(),
        //       ],
        //   }
        Self { indexes: Vec::new() }
    }
}

#[derive(Deserialize)]
struct CookToml {
    #[serde(default)]
    registry: Option<RegistryRaw>,
    #[serde(default)]
    modules: Option<BTreeMap<String, String>>,
}

#[derive(Deserialize)]
struct RegistryRaw {
    // `url` is the Phase 1 legacy field consumed by `cook pull`. We deserialize
    // and discard it here so a cook.toml that has both forms still parses.
    #[allow(dead_code)]
    url: Option<String>,
    #[serde(default)]
    indexes: Option<Vec<String>>,
}

pub fn parse_cook_toml(path: &Path) -> Result<(ManifestModules, ManifestRegistry)> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("read {}", path.display()))?;
    let parsed: CookToml = toml::from_str(&raw)
        .with_context(|| format!("parse {}", path.display()))?;
    let modules = ManifestModules {
        modules: parsed.modules.unwrap_or_default(),
    };
    let registry = match parsed.registry {
        None => ManifestRegistry::default(),
        Some(r) => match r.indexes {
            None | Some(_) if r.indexes.as_ref().is_none_or(|v| v.is_empty()) => {
                ManifestRegistry::default()
            }
            Some(indexes) => ManifestRegistry { indexes },
        },
    };
    Ok((modules, registry))
}

#[cfg(test)]
mod tests;
```

- [ ] **T1.2: Write the failing parser tests**

Create `cli/crates/cook-cli/src/modules/manifest_tests.rs` is **NOT** the tests file — `#[cfg(test)] mod tests;` looks for `manifest/tests.rs` or inline. Use the inline form. Append to `manifest.rs` (replacing the `#[cfg(test)] mod tests;` declaration with the inline block):

Replace the trailing line of `manifest.rs`:

```rust
#[cfg(test)]
mod tests;
```

With:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_cook_toml(contents: &str) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().expect("tempfile");
        f.write_all(contents.as_bytes()).expect("write");
        f
    }

    #[test]
    fn empty_file_yields_empty_manifest_and_default_registry() {
        let f = write_cook_toml("");
        let (m, r) = parse_cook_toml(f.path()).expect("parse");
        assert!(m.modules.is_empty());
        assert_eq!(r, ManifestRegistry::default());
    }

    #[test]
    fn modules_only() {
        let f = write_cook_toml(
            r#"
[modules]
cook_smoke  = "*"
"lua-cjson" = "2.1.*"
argparse    = ">=0.7"
"#,
        );
        let (m, r) = parse_cook_toml(f.path()).expect("parse");
        assert_eq!(m.modules.get("cook_smoke").map(String::as_str), Some("*"));
        assert_eq!(m.modules.get("lua-cjson").map(String::as_str), Some("2.1.*"));
        assert_eq!(m.modules.get("argparse").map(String::as_str), Some(">=0.7"));
        assert_eq!(r, ManifestRegistry::default());
    }

    #[test]
    fn registry_indexes_array() {
        let f = write_cook_toml(
            r#"
[registry]
indexes = ["https://rocks.usecook.com", "https://luarocks.org"]
"#,
        );
        let (_m, r) = parse_cook_toml(f.path()).expect("parse");
        assert_eq!(
            r.indexes,
            vec![
                "https://rocks.usecook.com".to_string(),
                "https://luarocks.org".to_string(),
            ]
        );
    }

    #[test]
    fn empty_indexes_falls_through_to_default() {
        let f = write_cook_toml(
            r#"
[registry]
indexes = []
"#,
        );
        let (_m, r) = parse_cook_toml(f.path()).expect("parse");
        assert_eq!(r, ManifestRegistry::default());
    }

    #[test]
    fn legacy_url_present_alongside_indexes() {
        // Phase 3 leaves `[registry].url` untouched; Phase 4 collapses these.
        // `cook modules` ignores `url` (only `cook pull` consumes it).
        let f = write_cook_toml(
            r#"
[registry]
url = "https://example.test/legacy"
indexes = ["https://rocks.usecook.com"]
"#,
        );
        let (_m, r) = parse_cook_toml(f.path()).expect("parse");
        assert_eq!(r.indexes, vec!["https://rocks.usecook.com".to_string()]);
    }

    #[test]
    fn malformed_toml_errors() {
        let f = write_cook_toml("[modules\n");
        let err = parse_cook_toml(f.path()).expect_err("must fail");
        assert!(format!("{:#}", err).contains("parse"));
    }

    #[test]
    fn non_string_constraint_rejected() {
        // `cook_smoke = 1` would deserialize as integer; we want strings only.
        let f = write_cook_toml(
            r#"
[modules]
cook_smoke = 1
"#,
        );
        assert!(parse_cook_toml(f.path()).is_err());
    }

    #[test]
    fn constraint_round_trip_byte_identical() {
        // Whatever the user wrote ends up byte-identical in the BTreeMap.
        let f = write_cook_toml(
            r#"
[modules]
cook_smoke = ">= 1.0, < 2.0"
"#,
        );
        let (m, _r) = parse_cook_toml(f.path()).expect("parse");
        assert_eq!(
            m.modules.get("cook_smoke").map(String::as_str),
            Some(">= 1.0, < 2.0")
        );
    }
}
```

- [ ] **T1.3: Add `tempfile` as a dev-dep**

Edit `cli/crates/cook-cli/Cargo.toml`. Find the `[dev-dependencies]` block (or add one if missing). Add:

```toml
tempfile = "3"
```

If `[dev-dependencies]` already exists with `tempfile`, skip — it's a frequent dep in this workspace.

- [ ] **T1.4: Run tests to verify they fail**

```bash
cd /home/alex/dev/cook/cli && cargo test -p cook-cli modules::manifest::tests 2>&1 | tail -30
```

Expected: compilation succeeds (the parser code from T1.1 is in place), all 8 tests run. If you get unexpected compile errors, fix them and re-run.

Specifically the `non_string_constraint_rejected` test should now PASS too because TOML deserialization of `cook_smoke = 1` into `BTreeMap<String, String>` errors. **At this point all tests should already be passing** — we wrote the implementation and tests in the same step.

- [ ] **T1.5: Run tests once more to confirm all pass**

```bash
cd /home/alex/dev/cook/cli && cargo test -p cook-cli modules::manifest 2>&1 | tail -15
```

Expected: `test result: ok. 8 passed; 0 failed`.

- [ ] **T1.6: Verify lints clean**

```bash
cd /home/alex/dev/cook/cli && cargo clippy -p cook-cli --tests -- -D warnings 2>&1 | tail -10
```

Expected: no warnings. If clippy complains about the `is_none_or` line not being stable, replace the match arm with a clearer form:

```rust
        Some(r) => {
            let indexes = r.indexes.unwrap_or_default();
            if indexes.is_empty() {
                ManifestRegistry::default()
            } else {
                ManifestRegistry { indexes }
            }
        }
```

Re-run T1.5 to confirm tests still pass.

- [ ] **T1.7: Commit**

```bash
cd /home/alex/dev/cook
git add cli/crates/cook-cli/src/modules/manifest.rs cli/crates/cook-cli/Cargo.toml
git commit -m "feat(phase3): M3.1 — cook.toml [modules] + [registry].indexes parsing"
```

---

## Task 2 (M3.2): `cook.lock` format + read/write + closure introspection

**Files:**
- Modify: `cli/crates/cook-cli/src/modules/lockfile.rs` (replace stub)
- Create: `cli/crates/cook-cli/tests/fixtures/lockfile/rock_tree/` (synthetic luarocks tree for introspection tests)
- Create: `cli/crates/cook-cli/tests/fixtures/lockfile/cache/` (synthetic cache dir with known-hash .src.rock)

**Slice boundary:** Owns `lockfile.rs` and the lockfile-test fixtures. Does NOT touch `manifest.rs` (Task 1) or `driver.rs` (Task 3) — depends on `ManifestModules` from Task 1 only as a public type.

### TDD: round-trip + integrity tests first

- [ ] **T2.1: Replace the stub with the lockfile module**

Replace the contents of `cli/crates/cook-cli/src/modules/lockfile.rs`:

```rust
//! M3.2 — `cook.lock` format, integrity verification, closure introspection.
//!
//! `cook.lock` is the deterministic state for `cook modules install`.
//! Generated by every state-changing invocation; consumed by plain
//! `cook modules install` to enforce reproducibility. The closure includes
//! both top-level (`direct = true`) and transitive (`direct = false`) rocks —
//! a full closure, matching Cargo / pnpm / Bundler.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::modules::manifest::ManifestModules;

const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lockfile {
    pub schema: u32,
    #[serde(rename = "module", default)]
    pub modules: Vec<LockedModule>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockedModule {
    pub name: String,
    pub version: String,
    pub source: String,
    pub integrity: String,
    pub direct: bool,
}

impl Lockfile {
    pub fn new(modules: Vec<LockedModule>) -> Self {
        Self {
            schema: SCHEMA_VERSION,
            modules,
        }
    }
}

pub fn read(path: &Path) -> Result<Lockfile> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("read {}", path.display()))?;
    let parsed: Lockfile = toml::from_str(&raw)
        .with_context(|| format!("parse {}", path.display()))?;
    if parsed.schema > SCHEMA_VERSION {
        return Err(anyhow!(
            "cook.lock schema version {} is newer than this cook supports (max {}); upgrade cook",
            parsed.schema,
            SCHEMA_VERSION
        ));
    }
    Ok(parsed)
}

pub fn write(path: &Path, lock: &Lockfile) -> Result<()> {
    let contents = toml::to_string_pretty(lock)
        .with_context(|| "serialize cook.lock")?;
    std::fs::write(path, contents)
        .with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

/// Verify the on-disk source rock matches the integrity in the lockfile.
/// `cache_dir` is `cook_modules/lib/luarocks/cache/` — the directory luarocks
/// caches downloaded source rocks into.
pub fn verify_integrity(locked: &LockedModule, cache_dir: &Path) -> Result<()> {
    let filename = format!("{}-{}.src.rock", locked.name, locked.version);
    let path = cache_dir.join(&filename);
    let bytes = std::fs::read(&path)
        .with_context(|| format!("read {}", path.display()))?;
    let mut h = Sha256::new();
    h.update(&bytes);
    let actual = format!("sha256-{}", B64.encode(h.finalize()));
    if actual != locked.integrity {
        return Err(anyhow!(
            "integrity mismatch for {}-{}: lockfile expects {}, on-disk computes {}",
            locked.name,
            locked.version,
            locked.integrity,
            actual
        ));
    }
    Ok(())
}

/// Walk `cook_modules/lib/luarocks/rocks-5.4/<name>/<version>/` and produce a
/// Lockfile pinning every rock present. `direct = true` for rocks named in the
/// manifest, else `false`.
///
/// Source URLs come from each rock's `<name>-<version>.rockspec` file (the
/// `source.url` field). Integrity hashes come from
/// `cook_modules/lib/luarocks/cache/<name>-<version>.src.rock`.
pub fn introspect_closure(modules_dir: &Path, manifest: &ManifestModules) -> Result<Lockfile> {
    let rocks_root = modules_dir.join("lib/luarocks/rocks-5.4");
    let cache_dir = modules_dir.join("lib/luarocks/cache");
    let mut out = Vec::new();
    if !rocks_root.is_dir() {
        return Ok(Lockfile::new(out));
    }
    let mut by_name: BTreeMap<String, BTreeMap<String, PathBuf>> = BTreeMap::new();
    for name_entry in std::fs::read_dir(&rocks_root)
        .with_context(|| format!("read {}", rocks_root.display()))?
    {
        let name_entry = name_entry?;
        if !name_entry.file_type()?.is_dir() {
            continue;
        }
        let name = name_entry.file_name().to_string_lossy().into_owned();
        for version_entry in std::fs::read_dir(name_entry.path())? {
            let version_entry = version_entry?;
            if !version_entry.file_type()?.is_dir() {
                continue;
            }
            let version = version_entry.file_name().to_string_lossy().into_owned();
            by_name
                .entry(name.clone())
                .or_default()
                .insert(version, version_entry.path());
        }
    }
    for (name, versions) in by_name {
        // Pick the highest version present (luarocks may keep older versions).
        // Sorted lex order is good enough here — semver-precise picking is
        // luarocks's job at install time.
        let (version, dir) = versions.into_iter().next_back().unwrap();
        let rockspec_path = dir.join(format!("{}-{}.rockspec", name, version));
        let source_url = parse_rockspec_source_url(&rockspec_path)
            .with_context(|| format!("read source.url from {}", rockspec_path.display()))?;
        let integrity = compute_integrity(&cache_dir, &name, &version)
            .with_context(|| format!("compute integrity for {}-{}", name, version))?;
        let direct = manifest.modules.contains_key(&name);
        out.push(LockedModule {
            name,
            version,
            source: source_url,
            integrity,
            direct,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(Lockfile::new(out))
}

fn parse_rockspec_source_url(path: &Path) -> Result<String> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("read {}", path.display()))?;
    // Rockspecs are Lua, not TOML. We do a minimal text scrape for the
    // `source = { url = "..." }` field; full Lua eval would be overkill.
    for line in raw.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("url") {
            let rest = rest.trim_start();
            if let Some(rest) = rest.strip_prefix('=') {
                let rest = rest.trim();
                let rest = rest.trim_start_matches('"').trim_end_matches(',').trim();
                let rest = rest.trim_end_matches('"');
                return Ok(rest.to_string());
            }
        }
    }
    Err(anyhow!(
        "rockspec at {} has no top-level `url = \"...\"` line",
        path.display()
    ))
}

fn compute_integrity(cache_dir: &Path, name: &str, version: &str) -> Result<String> {
    let path = cache_dir.join(format!("{}-{}.src.rock", name, version));
    if !path.exists() {
        // No cached source rock — luarocks installed from a local path or
        // a pre-existing tree. Mark as unknown integrity; the lockfile is
        // honest about gaps.
        return Ok("sha256-unknown".to_string());
    }
    let bytes = std::fs::read(&path)
        .with_context(|| format!("read {}", path.display()))?;
    let mut h = Sha256::new();
    h.update(&bytes);
    Ok(format!("sha256-{}", B64.encode(h.finalize())))
}

#[cfg(test)]
mod tests;
```

- [ ] **T2.2: Add `base64` to cook-cli dependencies**

Edit `cli/crates/cook-cli/Cargo.toml`. In the `[dependencies]` block add:

```toml
base64 = "0.22"
```

(Skip if already present.)

- [ ] **T2.3: Build to confirm types compile**

```bash
cd /home/alex/dev/cook/cli && cargo build -p cook-cli 2>&1 | tail -8
```

Expected: clean build. Fix any errors before proceeding.

- [ ] **T2.4: Create the test fixtures**

```bash
cd /home/alex/dev/cook
mkdir -p cli/crates/cook-cli/tests/fixtures/lockfile/rock_tree/lib/luarocks/rocks-5.4/cook_smoke/0.1.0-1
mkdir -p cli/crates/cook-cli/tests/fixtures/lockfile/rock_tree/lib/luarocks/rocks-5.4/luafilesystem/1.8.0-1
mkdir -p cli/crates/cook-cli/tests/fixtures/lockfile/rock_tree/lib/luarocks/cache
```

Write `cli/crates/cook-cli/tests/fixtures/lockfile/rock_tree/lib/luarocks/rocks-5.4/cook_smoke/0.1.0-1/cook_smoke-0.1.0-1.rockspec`:

```lua
package = "cook_smoke"
version = "0.1.0-1"
source = {
   url = "https://rocks.usecook.com/cook_smoke-0.1.0-1.src.rock",
}
description = {
   summary = "Phase 3 acceptance fixture",
   license = "MIT",
}
dependencies = { "lua >= 5.4" }
build = { type = "builtin", modules = { cook_smoke = "cook_smoke.lua" } }
```

Write `cli/crates/cook-cli/tests/fixtures/lockfile/rock_tree/lib/luarocks/rocks-5.4/luafilesystem/1.8.0-1/luafilesystem-1.8.0-1.rockspec`:

```lua
package = "luafilesystem"
version = "1.8.0-1"
source = {
   url = "https://luarocks.org/manifests/hisham/luafilesystem-1.8.0-1.src.rock",
}
description = { summary = "lfs", license = "MIT" }
dependencies = { "lua >= 5.1" }
build = { type = "builtin", modules = { lfs = "lfs.c" } }
```

Write deterministic-content fixture rocks (so SHA-256 is stable across machines):

```bash
echo "stub-cook_smoke-source-rock" > cli/crates/cook-cli/tests/fixtures/lockfile/rock_tree/lib/luarocks/cache/cook_smoke-0.1.0-1.src.rock
echo "stub-luafilesystem-source-rock" > cli/crates/cook-cli/tests/fixtures/lockfile/rock_tree/lib/luarocks/cache/luafilesystem-1.8.0-1.src.rock
```

- [ ] **T2.5: Compute the expected fixture hashes**

```bash
echo -n "Computing reference SHA-256 base64 hashes for fixtures..."
COOK_SMOKE_HASH=$(sha256sum cli/crates/cook-cli/tests/fixtures/lockfile/rock_tree/lib/luarocks/cache/cook_smoke-0.1.0-1.src.rock | awk '{print $1}' | xxd -r -p | base64)
LFS_HASH=$(sha256sum cli/crates/cook-cli/tests/fixtures/lockfile/rock_tree/lib/luarocks/cache/luafilesystem-1.8.0-1.src.rock | awk '{print $1}' | xxd -r -p | base64)
echo
echo "cook_smoke:     sha256-$COOK_SMOKE_HASH"
echo "luafilesystem:  sha256-$LFS_HASH"
```

Record both hashes — they go into the test assertions in T2.6.

- [ ] **T2.6: Write the inline tests**

Replace the trailing `#[cfg(test)] mod tests;` line of `lockfile.rs` with:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::path::PathBuf;

    fn fixture_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/lockfile/rock_tree")
    }

    fn sample_lockfile() -> Lockfile {
        Lockfile::new(vec![
            LockedModule {
                name: "cook_smoke".into(),
                version: "0.1.0-1".into(),
                source: "https://rocks.usecook.com/cook_smoke-0.1.0-1.src.rock".into(),
                integrity: "sha256-1f3d".into(),
                direct: true,
            },
            LockedModule {
                name: "luafilesystem".into(),
                version: "1.8.0-1".into(),
                source: "https://luarocks.org/manifests/hisham/luafilesystem-1.8.0-1.src.rock".into(),
                integrity: "sha256-9b2c".into(),
                direct: false,
            },
        ])
    }

    #[test]
    fn round_trip_serialization() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("cook.lock");
        let lock = sample_lockfile();
        write(&path, &lock).expect("write");
        let read_back = read(&path).expect("read");
        assert_eq!(read_back, lock);
    }

    #[test]
    fn schema_mismatch_rejected() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("cook.lock");
        let mut f = std::fs::File::create(&path).expect("create");
        writeln!(f, "schema = 99").expect("write");
        let err = read(&path).expect_err("must fail");
        assert!(format!("{:#}", err).contains("schema version 99"));
    }

    #[test]
    fn integrity_match_ok() {
        // Use the fixture cache + a hash matching its on-disk content.
        // Compute the expected hash inline so the test is self-checking.
        let cache = fixture_root().join("lib/luarocks/cache");
        let bytes = std::fs::read(cache.join("cook_smoke-0.1.0-1.src.rock"))
            .expect("read fixture");
        let mut h = Sha256::new();
        h.update(&bytes);
        let expected = format!("sha256-{}", B64.encode(h.finalize()));
        let locked = LockedModule {
            name: "cook_smoke".into(),
            version: "0.1.0-1".into(),
            source: "https://rocks.usecook.com/cook_smoke-0.1.0-1.src.rock".into(),
            integrity: expected,
            direct: true,
        };
        verify_integrity(&locked, &cache).expect("integrity ok");
    }

    #[test]
    fn integrity_mismatch_errors_with_both_hashes() {
        let cache = fixture_root().join("lib/luarocks/cache");
        let locked = LockedModule {
            name: "cook_smoke".into(),
            version: "0.1.0-1".into(),
            source: "https://example/cook_smoke.src.rock".into(),
            integrity: "sha256-wrong".into(),
            direct: true,
        };
        let err = verify_integrity(&locked, &cache).expect_err("must fail");
        let msg = format!("{:#}", err);
        assert!(msg.contains("cook_smoke"));
        assert!(msg.contains("expects"));
        assert!(msg.contains("computes"));
    }

    #[test]
    fn introspect_closure_marks_direct_correctly() {
        let mut manifest = ManifestModules::default();
        manifest.modules.insert("cook_smoke".into(), "*".into());
        // luafilesystem is NOT in the manifest -> direct = false.
        let modules_dir = fixture_root();
        let lock = introspect_closure(&modules_dir, &manifest).expect("introspect");
        assert_eq!(lock.modules.len(), 2);
        let by_name: std::collections::HashMap<&str, &LockedModule> = lock
            .modules
            .iter()
            .map(|m| (m.name.as_str(), m))
            .collect();
        assert!(by_name["cook_smoke"].direct);
        assert!(!by_name["luafilesystem"].direct);
        assert_eq!(by_name["cook_smoke"].version, "0.1.0-1");
        assert_eq!(
            by_name["cook_smoke"].source,
            "https://rocks.usecook.com/cook_smoke-0.1.0-1.src.rock"
        );
    }

    #[test]
    fn introspect_empty_tree_yields_empty_lockfile() {
        let dir = tempfile::tempdir().expect("tempdir");
        let lock = introspect_closure(dir.path(), &ManifestModules::default())
            .expect("introspect");
        assert!(lock.modules.is_empty());
        assert_eq!(lock.schema, SCHEMA_VERSION);
    }
}
```

- [ ] **T2.7: Run the lockfile tests**

```bash
cd /home/alex/dev/cook/cli && cargo test -p cook-cli modules::lockfile 2>&1 | tail -15
```

Expected: `test result: ok. 6 passed; 0 failed`.

If `round_trip_serialization` fails on the `BTreeMap`-vs-`Vec` issue (toml serializing a `Vec<LockedModule>` with `serde(rename = "module")` produces an array-of-tables), inspect the produced TOML:

```bash
cd /home/alex/dev/cook/cli && cargo test -p cook-cli modules::lockfile::tests::round_trip_serialization -- --nocapture 2>&1 | tail -20
```

The expected on-disk shape is `[[module]]` headers per locked module, matching the spec.

- [ ] **T2.8: Verify lints clean**

```bash
cd /home/alex/dev/cook/cli && cargo clippy -p cook-cli --tests -- -D warnings 2>&1 | tail -10
```

Expected: no warnings.

- [ ] **T2.9: Commit**

```bash
cd /home/alex/dev/cook
git add cli/crates/cook-cli/src/modules/lockfile.rs \
        cli/crates/cook-cli/Cargo.toml \
        cli/crates/cook-cli/tests/fixtures/lockfile/
git commit -m "feat(phase3): M3.2 — cook.lock format + integrity + closure introspection"
```

---

## Task 3 (M3.3): LuaRocks subprocess driver

**Files:**
- Modify: `cli/crates/cook-cli/src/modules/driver.rs` (replace stub)
- Create: `cli/crates/cook-cli/tests/fixtures/driver/fake-luarocks.sh` (records argv for golden tests)

**Slice boundary:** Owns `driver.rs` and the fake-luarocks fixture. Does NOT touch `manifest.rs` (Task 1) or `lockfile.rs` (Task 2) — depends on `LockedModule` and `ManifestModules` from those tasks only as public types. Online integration testing happens in Task 6's clap-surface integration tests, not here.

### TDD: argv-shape golden tests first

- [ ] **T3.1: Replace the stub with the driver module**

Replace the contents of `cli/crates/cook-cli/src/modules/driver.rs`:

```rust
//! M3.3 — `~/.cook/bin/luarocks` subprocess wrapper.
//!
//! The driver wraps every state-changing or read-only luarocks invocation
//! cook needs. Every call passes `--tree <project>/cook_modules` so rocks
//! land in the project's tree, never in a user-global luarocks tree.
//! Index precedence is realised by passing `--server <url>` repeatedly in
//! left-to-right order.
//!
//! Error handling is passthrough: on non-zero exit, the driver returns an
//! `anyhow::Error` whose Display contains argv + captured stdout + captured
//! stderr (each capped at 8 KiB to match `cook.exec`'s SHI-188 truncation).
//! No structured parsing of luarocks output.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use anyhow::{anyhow, Context, Result};

use crate::modules::lockfile::LockedModule;

const STREAM_CAP_BYTES: usize = 8 * 1024;

#[derive(Debug, Clone)]
pub struct RocksDriver {
    prefix: PathBuf,
    indexes: Vec<String>,
    project_dir: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledRock {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHit {
    pub name: String,
    pub version: String,
    pub index: String,
}

impl RocksDriver {
    pub fn new(prefix: PathBuf, indexes: Vec<String>, project_dir: PathBuf) -> Self {
        Self {
            prefix,
            indexes,
            project_dir,
        }
    }

    pub fn binary(&self) -> PathBuf {
        self.prefix.join("bin/luarocks")
    }

    pub fn tree_arg(&self) -> PathBuf {
        self.project_dir.join("cook_modules")
    }

    /// Build the base argv prefix used by every invocation.
    pub fn base_argv(&self) -> Vec<String> {
        let mut v = vec![
            "--tree".to_string(),
            self.tree_arg().to_string_lossy().into_owned(),
        ];
        for idx in &self.indexes {
            v.push("--server".to_string());
            v.push(idx.clone());
        }
        v
    }

    pub fn install(&self, name: &str, constraint: &str) -> Result<()> {
        let mut argv = vec!["install".to_string()];
        argv.extend(self.base_argv());
        argv.push(name.to_string());
        if !constraint.is_empty() && constraint != "*" {
            argv.push(constraint.to_string());
        }
        self.run(&argv)?;
        Ok(())
    }

    pub fn install_locked(&self, locked: &LockedModule) -> Result<()> {
        // Bypass luarocks's resolver — install directly from the pinned URL.
        let mut argv = vec!["install".to_string()];
        argv.extend(self.base_argv());
        argv.push(locked.source.clone());
        self.run(&argv)?;
        Ok(())
    }

    pub fn remove(&self, name: &str) -> Result<()> {
        let mut argv = vec!["remove".to_string()];
        argv.extend(self.base_argv());
        argv.push(name.to_string());
        self.run(&argv)?;
        Ok(())
    }

    pub fn search(&self, query: &str) -> Result<Vec<SearchHit>> {
        let mut argv = vec!["search".to_string()];
        argv.extend(self.base_argv());
        argv.push(query.to_string());
        let out = self.run(&argv)?;
        Ok(parse_search_output(&out.stdout, &self.indexes))
    }

    pub fn list_installed(&self) -> Result<Vec<InstalledRock>> {
        let mut argv = vec!["list".to_string()];
        argv.extend(self.base_argv());
        argv.push("--porcelain".to_string());
        let out = self.run(&argv)?;
        Ok(parse_list_output(&out.stdout))
    }

    /// Run the luarocks binary with the given argv, return captured Output.
    /// On non-zero exit, return a passthrough error.
    fn run(&self, argv: &[String]) -> Result<Output> {
        let bin = self.binary();
        let out = Command::new(&bin)
            .args(argv)
            .output()
            .with_context(|| format!("spawn {}", bin.display()))?;
        if !out.status.success() {
            let argv_quoted = argv
                .iter()
                .map(|a| {
                    if a.contains(' ') {
                        format!("'{}'", a)
                    } else {
                        a.clone()
                    }
                })
                .collect::<Vec<_>>()
                .join(" ");
            return Err(anyhow!(
                "luarocks failed: {} {}\n--- stdout ---\n{}\n--- stderr ---\n{}\n--- exit {} ---",
                bin.display(),
                argv_quoted,
                truncate_stream(&out.stdout),
                truncate_stream(&out.stderr),
                out.status.code().map(|c| c.to_string()).unwrap_or_else(|| "signal".into()),
            ));
        }
        Ok(out)
    }
}

fn truncate_stream(bytes: &[u8]) -> String {
    let s = String::from_utf8_lossy(bytes);
    if s.len() <= STREAM_CAP_BYTES {
        return s.into_owned();
    }
    let truncated = &s[..STREAM_CAP_BYTES];
    format!(
        "{}... ({} more bytes)",
        truncated,
        s.len() - STREAM_CAP_BYTES
    )
}

fn parse_list_output(stdout: &[u8]) -> Vec<InstalledRock> {
    // luarocks --porcelain `list` output: lines of the form `name\tversion\t...`.
    String::from_utf8_lossy(stdout)
        .lines()
        .filter_map(|line| {
            let mut cols = line.split('\t');
            let name = cols.next()?;
            let version = cols.next()?;
            if name.is_empty() {
                return None;
            }
            Some(InstalledRock {
                name: name.to_string(),
                version: version.to_string(),
            })
        })
        .collect()
}

fn parse_search_output(stdout: &[u8], indexes: &[String]) -> Vec<SearchHit> {
    // luarocks `search` output isn't perfectly stable; we extract `name (version)`
    // pairs and tag them with the first configured index (best effort).
    // Structured search semantics is not a Phase 3 goal; the user sees luarocks's
    // own output too via the parent command stdout.
    let s = String::from_utf8_lossy(stdout);
    let default_index = indexes.first().cloned().unwrap_or_default();
    let mut hits = Vec::new();
    for line in s.lines() {
        let trimmed = line.trim();
        if let Some(idx) = trimmed.find('(') {
            let name = trimmed[..idx].trim();
            let rest = &trimmed[idx + 1..];
            if let Some(end) = rest.find(')') {
                let version = rest[..end].trim();
                if !name.is_empty() && !version.is_empty() {
                    hits.push(SearchHit {
                        name: name.to_string(),
                        version: version.to_string(),
                        index: default_index.clone(),
                    });
                }
            }
        }
    }
    hits
}

#[cfg(test)]
mod tests;
```

- [ ] **T3.2: Build to confirm types compile**

```bash
cd /home/alex/dev/cook/cli && cargo build -p cook-cli 2>&1 | tail -8
```

Expected: clean build.

- [ ] **T3.3: Create the fake-luarocks fixture**

Create `cli/crates/cook-cli/tests/fixtures/driver/fake-luarocks.sh`:

```bash
#!/bin/sh
# Fake luarocks for golden argv tests. Records the argv it was invoked with
# to a file pointed to by $FAKE_LUAROCKS_LOG, then exits with $FAKE_LUAROCKS_EXIT
# (default 0). Stdout is whatever $FAKE_LUAROCKS_STDOUT contains.

if [ -n "$FAKE_LUAROCKS_LOG" ]; then
    echo "argv:" > "$FAKE_LUAROCKS_LOG"
    for a in "$@"; do
        printf '  %s\n' "$a" >> "$FAKE_LUAROCKS_LOG"
    done
fi

if [ -n "$FAKE_LUAROCKS_STDOUT" ]; then
    printf '%s\n' "$FAKE_LUAROCKS_STDOUT"
fi

if [ -n "$FAKE_LUAROCKS_STDERR" ]; then
    printf '%s\n' "$FAKE_LUAROCKS_STDERR" >&2
fi

exit "${FAKE_LUAROCKS_EXIT:-0}"
```

```bash
chmod +x cli/crates/cook-cli/tests/fixtures/driver/fake-luarocks.sh
```

- [ ] **T3.4: Write the inline driver tests**

Replace the trailing `#[cfg(test)] mod tests;` line of `driver.rs` with:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fake_prefix() -> tempfile::TempDir {
        // Set up a fake $prefix where bin/luarocks is a symlink to the
        // tests/fixtures/driver/fake-luarocks.sh script.
        let tmp = tempfile::tempdir().expect("tempdir");
        let bin = tmp.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let fake = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/driver/fake-luarocks.sh");
        std::os::unix::fs::symlink(&fake, bin.join("luarocks")).expect("symlink");
        tmp
    }

    fn read_argv_log(path: &Path) -> Vec<String> {
        let raw = std::fs::read_to_string(path).expect("read log");
        raw.lines()
            .skip(1) // skip "argv:" header
            .map(|l| l.trim().to_string())
            .collect()
    }

    #[test]
    fn install_argv_includes_tree_and_servers() {
        let prefix = fake_prefix();
        let project = tempfile::tempdir().expect("project");
        let log = project.path().join("argv.log");
        std::env::set_var("FAKE_LUAROCKS_LOG", &log);
        std::env::set_var("FAKE_LUAROCKS_EXIT", "0");

        let driver = RocksDriver::new(
            prefix.path().to_path_buf(),
            vec![
                "https://rocks.usecook.com".to_string(),
                "https://luarocks.org".to_string(),
            ],
            project.path().to_path_buf(),
        );
        driver.install("cook_smoke", "*").expect("install");

        let argv = read_argv_log(&log);
        assert_eq!(argv[0], "install");
        assert!(argv.iter().any(|a| a == "--tree"));
        let tree_idx = argv.iter().position(|a| a == "--tree").unwrap();
        assert!(argv[tree_idx + 1].ends_with("cook_modules"));
        let server_args: Vec<&String> = argv
            .iter()
            .enumerate()
            .filter(|(i, _)| i > &0 && argv[i - 1] == "--server")
            .map(|(_, v)| v)
            .collect();
        assert_eq!(
            server_args,
            vec![
                &"https://rocks.usecook.com".to_string(),
                &"https://luarocks.org".to_string(),
            ]
        );
        assert_eq!(argv.last().unwrap(), "cook_smoke");
        // Constraint "*" omitted from argv (passes through as no-constraint).
        assert!(!argv.iter().any(|a| a == "*"));
    }

    #[test]
    fn install_with_explicit_constraint_passes_through() {
        let prefix = fake_prefix();
        let project = tempfile::tempdir().expect("project");
        let log = project.path().join("argv.log");
        std::env::set_var("FAKE_LUAROCKS_LOG", &log);

        let driver = RocksDriver::new(
            prefix.path().to_path_buf(),
            Vec::new(),
            project.path().to_path_buf(),
        );
        driver.install("argparse", ">=0.7").expect("install");
        let argv = read_argv_log(&log);
        assert_eq!(argv[0], "install");
        assert!(argv.iter().any(|a| a == "argparse"));
        assert!(argv.iter().any(|a| a == ">=0.7"));
    }

    #[test]
    fn install_locked_uses_pinned_url() {
        let prefix = fake_prefix();
        let project = tempfile::tempdir().expect("project");
        let log = project.path().join("argv.log");
        std::env::set_var("FAKE_LUAROCKS_LOG", &log);

        let driver = RocksDriver::new(
            prefix.path().to_path_buf(),
            vec!["https://example".into()],
            project.path().to_path_buf(),
        );
        let locked = LockedModule {
            name: "cook_smoke".into(),
            version: "0.1.0-1".into(),
            source: "https://rocks.usecook.com/cook_smoke-0.1.0-1.src.rock".into(),
            integrity: "sha256-x".into(),
            direct: true,
        };
        driver.install_locked(&locked).expect("install_locked");
        let argv = read_argv_log(&log);
        assert_eq!(argv.last().unwrap(), &locked.source);
    }

    #[test]
    fn nonzero_exit_passes_through_argv_stdout_stderr() {
        let prefix = fake_prefix();
        let project = tempfile::tempdir().expect("project");
        let log = project.path().join("argv.log");
        std::env::set_var("FAKE_LUAROCKS_LOG", &log);
        std::env::set_var("FAKE_LUAROCKS_EXIT", "7");
        std::env::set_var("FAKE_LUAROCKS_STDOUT", "stdout-marker");
        std::env::set_var("FAKE_LUAROCKS_STDERR", "stderr-marker");

        let driver = RocksDriver::new(
            prefix.path().to_path_buf(),
            Vec::new(),
            project.path().to_path_buf(),
        );
        let err = driver.remove("cook_smoke").expect_err("must fail");
        let msg = format!("{:#}", err);
        assert!(msg.contains("luarocks failed"));
        assert!(msg.contains("stdout-marker"));
        assert!(msg.contains("stderr-marker"));
        assert!(msg.contains("exit 7"));
        // Cleanup env so other tests don't see the failure setup.
        std::env::remove_var("FAKE_LUAROCKS_EXIT");
        std::env::remove_var("FAKE_LUAROCKS_STDOUT");
        std::env::remove_var("FAKE_LUAROCKS_STDERR");
    }

    #[test]
    fn parse_list_output_extracts_porcelain() {
        let stdout = b"cook_smoke\t0.1.0-1\tinstalled\nargparse\t0.7.1-1\tinstalled\n";
        let rocks = parse_list_output(stdout);
        assert_eq!(rocks.len(), 2);
        assert_eq!(rocks[0].name, "cook_smoke");
        assert_eq!(rocks[0].version, "0.1.0-1");
        assert_eq!(rocks[1].name, "argparse");
        assert_eq!(rocks[1].version, "0.7.1-1");
    }

    #[test]
    fn parse_search_output_extracts_name_version() {
        let stdout = b"cook_smoke (0.1.0-1)\nlua-cjson (2.1.0.10-1)\n";
        let hits = parse_search_output(stdout, &["https://rocks.usecook.com".into()]);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].name, "cook_smoke");
        assert_eq!(hits[0].version, "0.1.0-1");
        assert_eq!(hits[0].index, "https://rocks.usecook.com");
    }

    #[test]
    fn truncate_stream_caps_long_output() {
        let big = vec![b'A'; STREAM_CAP_BYTES + 100];
        let truncated = truncate_stream(&big);
        assert!(truncated.contains("100 more bytes"));
        assert!(truncated.len() < big.len());
    }
}
```

- [ ] **T3.5: Run the driver tests**

```bash
cd /home/alex/dev/cook/cli && cargo test -p cook-cli modules::driver 2>&1 | tail -15
```

Expected: `test result: ok. 7 passed; 0 failed`.

If a test fails because it can't `chmod +x` the fixture (some checkout configurations strip the bit), re-run T3.3's `chmod` and the test setup. The shebang script needs the executable bit on disk.

⚠️ Cross-test env var leakage: tests share `std::env`. The `nonzero_exit_passes_through_argv_stdout_stderr` test sets `FAKE_LUAROCKS_EXIT=7` and cleans it up at the end; if cargo runs tests in parallel and another test reads the env var mid-flight, it can flake. If you see flakes, gate driver tests with `--test-threads=1`:

```bash
cd /home/alex/dev/cook/cli && cargo test -p cook-cli modules::driver -- --test-threads=1
```

If the flake is reproducible, refactor: pass env via a `Driver` field instead of process env. Document the choice in the test-helper comment if you change it.

- [ ] **T3.6: Verify lints clean**

```bash
cd /home/alex/dev/cook/cli && cargo clippy -p cook-cli --tests -- -D warnings 2>&1 | tail -10
```

Expected: no warnings.

- [ ] **T3.7: Commit**

```bash
cd /home/alex/dev/cook
git add cli/crates/cook-cli/src/modules/driver.rs \
        cli/crates/cook-cli/tests/fixtures/driver/
git commit -m "feat(phase3): M3.3 — LuaRocks subprocess driver with argv golden tests"
```

---

## Task 4 (M3.5 + M3.6 co-PR): Runtime `package.path` / `package.cpath` extension + Standard §7

**Files:**
- Modify: `cli/crates/cook-luaotp/src/pool.rs:494-522` (rename + grow `refresh_package_path`)
- Modify: `Cookfile` and `cook_modules/luarocks_phase2.lua` (delete the manual cpath prepends in `chore "gate-m2"`)
- Move: `tests/fixtures/c-ext-hello/` → `cli/crates/cook-engine/tests/fixtures/c-rock/`
- Modify: `standard/src/content/docs/07-cross-cookfile-composition.mdx` (new normative clause + example update)
- Modify: `standard/src/content/docs/appendix/B-rationale.mdx` (rationale paragraph)
- Modify: `standard/src/content/docs/appendix/D-conformance-summary.mdx` (new CS entry)
- Create: `standard/conformance/positive/<NNN>-rocks-share-lua-resolution/{Cookfile,manifest.json,...}` (new positive case)

**Slice boundary:** This is a co-PR — the runtime change and the Standard amendment land in one commit. `COOK_STANDARD_BYPASS=1` is **never** used; the pre-commit hook is the backstop. Owns the gate-m2 chore-body cleanup as part of fixture relocation. Does not edit `manifest.rs`/`lockfile.rs`/`driver.rs` (Tasks 1–3) and does not edit `cli.rs` (Task 6).

### TDD: extend the existing pool tests first

- [ ] **T4.1: Survey the existing call sites**

```bash
cd /home/alex/dev/cook
grep -rn 'refresh_package_path' cli/ 2>&1 | head -20
```

Expected: at least one definition at `cli/crates/cook-luaotp/src/pool.rs:498` and one or more call sites in the worker loop. Note all call sites — every one is renamed in T4.4.

- [ ] **T4.2: Read the function under change**

Read `cli/crates/cook-luaotp/src/pool.rs:494-522`. Confirm the existing function:

- Stashes `_cook_original_path` once.
- Sets `package.path` to `<cm>/?.lua;<cm>/?/init.lua;<orig>`.
- Returns `mlua::Result<()>`.

- [ ] **T4.3: Replace `refresh_package_path` with `refresh_package_search_paths`**

Edit `cli/crates/cook-luaotp/src/pool.rs`. Replace the existing function (lines roughly 494–522) with:

```rust
/// Refresh `package.path` and `package.cpath` for the upcoming work unit so
/// `require("foo")` finds rocks under `<cwd>/cook_modules/`. Called per-unit
/// from the worker loop because `cwd` is per-Cookfile and each body unit may
/// come from a different one.
///
/// Search-path order (Standard §7):
///
///   package.path:
///     <cwd>/cook_modules/?.lua                          hand-vendored, single file
///     <cwd>/cook_modules/?/init.lua                     hand-vendored, dir module
///     <cwd>/cook_modules/share/lua/5.4/?.lua            LuaRocks pure Lua
///     <cwd>/cook_modules/share/lua/5.4/?/init.lua       LuaRocks pure Lua
///     <original>
///
///   package.cpath:
///     <cwd>/cook_modules/?.<so-ext>                     hand-vendored, top level
///     <cwd>/cook_modules/lib/lua/5.4/?.<so-ext>         LuaRocks-installed C
///     <original>
///
/// `<so-ext>` is `.so` on Linux/macOS (Lua's loader convention; LuaRocks emits
/// `.so` on macOS too) and `.dll` on Windows. The original suffixes are stashed
/// once so per-unit refresh is idempotent across calls.
fn refresh_package_search_paths(lua: &mlua::Lua, cwd: &PathBuf) -> mlua::Result<()> {
    let cook_modules = cwd.join("cook_modules");
    let pkg: mlua::Table = match lua.globals().get::<mlua::Value>("package")? {
        mlua::Value::Table(t) => t,
        _ => return Ok(()),
    };

    // Stash originals on first call so subsequent calls don't grow the suffix.
    let original_path: String = match pkg.get::<mlua::Value>("_cook_original_path")? {
        mlua::Value::String(s) => s.to_str()?.to_string(),
        _ => {
            let cur: String = pkg.get::<String>("path").unwrap_or_default();
            pkg.set("_cook_original_path", cur.clone())?;
            cur
        }
    };
    let original_cpath: String = match pkg.get::<mlua::Value>("_cook_original_cpath")? {
        mlua::Value::String(s) => s.to_str()?.to_string(),
        _ => {
            let cur: String = pkg.get::<String>("cpath").unwrap_or_default();
            pkg.set("_cook_original_cpath", cur.clone())?;
            cur
        }
    };

    let cm = cook_modules.display().to_string();
    let so_ext = if cfg!(target_os = "windows") { "dll" } else { "so" };

    let new_path = format!(
        "{cm}/?.lua;{cm}/?/init.lua;{cm}/share/lua/5.4/?.lua;{cm}/share/lua/5.4/?/init.lua;{orig}",
        cm = cm,
        orig = original_path,
    );
    let new_cpath = format!(
        "{cm}/?.{ext};{cm}/lib/lua/5.4/?.{ext};{orig}",
        cm = cm,
        ext = so_ext,
        orig = original_cpath,
    );

    pkg.set("path", new_path)?;
    pkg.set("cpath", new_cpath)?;
    Ok(())
}
```

- [ ] **T4.4: Update every call site of the old name**

```bash
cd /home/alex/dev/cook
grep -rn 'refresh_package_path' cli/ standard/ tests/ 2>&1 | grep -v 'target/'
```

For each remaining match outside `pool.rs`, replace `refresh_package_path` with `refresh_package_search_paths` (use `sed -i` if multiple files). Re-run the grep until empty.

```bash
grep -rn 'refresh_package_path\b' cli/ 2>&1 | grep -v 'target/' | head
```

Expected: empty output.

- [ ] **T4.5: Build to confirm**

```bash
cd /home/alex/dev/cook/cli && cargo build -p cook-luaotp 2>&1 | tail -5
```

Expected: clean build.

- [ ] **T4.6: Add a unit test for the cpath shape**

Find the existing test module in `cli/crates/cook-luaotp/src/pool.rs` (the `#[cfg(test)] mod tests` block; if there isn't one yet, add one at the file's end). Add:

```rust
#[cfg(test)]
mod search_path_tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn refresh_sets_path_and_cpath_with_rock_tree_entries() {
        let lua = mlua::Lua::new();
        let cwd = PathBuf::from("/tmp/fake-project");
        refresh_package_search_paths(&lua, &cwd).expect("refresh");
        let pkg: mlua::Table = lua.globals().get("package").unwrap();
        let path: String = pkg.get("path").unwrap();
        let cpath: String = pkg.get("cpath").unwrap();

        assert!(path.contains("/tmp/fake-project/cook_modules/?.lua"));
        assert!(path.contains("/tmp/fake-project/cook_modules/?/init.lua"));
        assert!(path.contains("/tmp/fake-project/cook_modules/share/lua/5.4/?.lua"));
        assert!(path.contains("/tmp/fake-project/cook_modules/share/lua/5.4/?/init.lua"));

        assert!(cpath.contains("/tmp/fake-project/cook_modules/?."));
        assert!(cpath.contains("/tmp/fake-project/cook_modules/lib/lua/5.4/?."));
    }

    #[test]
    fn refresh_is_idempotent() {
        let lua = mlua::Lua::new();
        let cwd = PathBuf::from("/tmp/fake-project");
        refresh_package_search_paths(&lua, &cwd).expect("first");
        let pkg: mlua::Table = lua.globals().get("package").unwrap();
        let first: String = pkg.get("path").unwrap();
        refresh_package_search_paths(&lua, &cwd).expect("second");
        let second: String = pkg.get("path").unwrap();
        assert_eq!(first, second, "path must not grow on repeated refresh");
    }
}
```

- [ ] **T4.7: Run the new tests**

```bash
cd /home/alex/dev/cook/cli && cargo test -p cook-luaotp pool::search_path_tests 2>&1 | tail -10
```

Expected: 2 passed.

### Fixture relocation + gate-m2 cleanup

- [ ] **T4.8: Move the C-extension fixture**

```bash
cd /home/alex/dev/cook
mkdir -p cli/crates/cook-engine/tests/fixtures/c-rock
git mv tests/fixtures/c-ext-hello/* cli/crates/cook-engine/tests/fixtures/c-rock/
rmdir tests/fixtures/c-ext-hello 2>/dev/null || true
```

- [ ] **T4.9: Update fixture references in gate-m2**

Read `cook_modules/luarocks_phase2.lua` and find any reference to `tests/fixtures/c-ext-hello/`. Replace with `cli/crates/cook-engine/tests/fixtures/c-rock/`. Same for `Cookfile-a.lua` / `Cookfile-b.lua` if they reference the old path.

```bash
cd /home/alex/dev/cook
grep -rn 'c-ext-hello' . --include='*.lua' --include='Cookfile' --include='*.toml' 2>&1 | grep -v target/
```

Replace every match.

- [ ] **T4.10: Delete the manual cpath prepends in gate-m2 fixtures**

The Phase 2 gate-m2 chore generates two test Cookfiles (`Cookfile-a.lua`, `Cookfile-b.lua`) inside `target/gate-m2/proj/`. Each starts with:

```lua
package.cpath = "<absolute-tmpdir>/proj/cook_modules/?.so;" .. package.cpath
```

(or the `lib/lua/5.4/` variant for Part B). Find the chore body that emits these Cookfiles in `cook_modules/luarocks_phase2.lua`:

```bash
grep -n 'package.cpath' cook_modules/luarocks_phase2.lua
```

Delete the `package.cpath = ...` line from each generated Cookfile. The runtime now configures cpath; the manual prepend was the Phase 2 workaround.

After the deletions, the generated `Cookfile-a.lua` should start straight with:

```lua
local m = require("cook_hello")
print(m.value())
```

And `Cookfile-b.lua` should start with:

```lua
local cjson = require("cjson")
local s = cjson.encode({ hello = "world" })
print(s)
local t = cjson.decode(s)
assert(t.hello == "world", "round-trip failed")
print("round-trip ok")
```

- [ ] **T4.11: Confirm gate-m2 still passes**

```bash
cd /home/alex/dev/cook
cargo build --release -p cook-cli 2>&1 | tail -3
cook chore gate-m2 2>&1 | tail -20
```

Expected: both Part A (prints `42`) and Part B (`{"hello":"world"}` and `round-trip ok`) succeed without the manual cpath prepends. If they fail, the runtime change in T4.3 didn't propagate — re-check the call sites updated in T4.4.

### Standard §7 amendment

- [ ] **T4.12: Locate the §7 anchor**

Read `standard/src/content/docs/07-cross-cookfile-composition.mdx`, focusing on lines 65–75 (the existing locality clause at line 68). The new clause inserts immediately after that sentence.

- [ ] **T4.13: Add the new normative clause**

Find the existing line (around 68):

```
For an execute-phase work unit, "the calling work unit's source Cookfile" (§{lua.cook-load-module}) is the Cookfile that contained the recipe whose body unit is being evaluated. A conforming implementation MUST resolve `cook.load_module(name)` and `require(name)` against that Cookfile's `cook_modules/` directory and MUST NOT fall back to a sibling Cookfile's `cook_modules/`.
```

Insert immediately after it (a new paragraph):

```
A conforming implementation MUST configure `package.path` and `package.cpath` for an execute-phase work unit such that the calling Cookfile's `cook_modules/` is searched in the following order: (1) `cook_modules/?.lua` and `cook_modules/?/init.lua` (hand-vendored, top level); (2) `cook_modules/share/lua/<lua-version>/?.lua` and `cook_modules/share/lua/<lua-version>/?/init.lua` (LuaRocks-installed pure Lua); for native modules: (a) `cook_modules/?.<so-ext>` (hand-vendored, top level); (b) `cook_modules/lib/lua/<lua-version>/?.<so-ext>` (LuaRocks-installed). `<lua-version>` is the embedded Lua's MAJOR.MINOR (currently `5.4`). `<so-ext>` is the platform's loadable-module extension (`.so` on Linux/macOS, `.dll` on Windows).
```

- [ ] **T4.14: Add the App. B rationale paragraph**

Read `standard/src/content/docs/appendix/B-rationale.mdx` (or the equivalent — confirm the file exists; the path from the spec's references). Find the section discussing `cook_modules/` (search for "cook_modules" or the locality rationale).

Append a new paragraph at the appropriate spot:

```
The §7 search-path order adopts the LuaRocks tree layout (`share/lua/<ver>/`, `lib/lua/<ver>/`) as the on-disk shape so cook can interoperate with luarocks.org rockspecs without forking layout policy. Hand-vendored top-level files retain highest priority so authors can override a rock-installed version by dropping a file at `cook_modules/<name>.lua`; the LuaRocks tree is searched second.
```

- [ ] **T4.15: Add the App. D conformance-summary entry**

Read `standard/src/content/docs/appendix/D-conformance-summary.mdx`. Locate the highest existing CS entry number (search for `CS-0`). Use the next available number for the new entry.

Append:

```
- **CS-XXXX** — Search-path order within `cook_modules/` (§{modules.search-path-order}). MUST configure `package.path` and `package.cpath` per the search-path order in §7.
```

(Replace `XXXX` with the next available number, zero-padded to 4 digits.)

The §-anchor `{modules.search-path-order}` corresponds to the new clause's heading anchor; if §7's amendment doesn't have an explicit anchor yet, add one as a heading or annotation per the existing §7 anchor pattern.

- [ ] **T4.16: Add a positive conformance case**

Find the highest-numbered positive case under `standard/conformance/positive/`:

```bash
ls standard/conformance/positive/ | tail -3
```

Use the next number. Create the new case directory:

```bash
NEW_NUM=$(printf '%03d' $(($(ls standard/conformance/positive/ | grep -E '^[0-9]+' | sort -n | tail -1 | cut -c1-3) + 1)))
mkdir -p standard/conformance/positive/${NEW_NUM}-rocks-share-lua-resolution
```

In that new directory, write:

`Cookfile`:

```cook
use rockmod

recipe smoke
    > local m = require("rockmod")
    > print(m.value)
end
```

`cook_modules/share/lua/5.4/rockmod.lua`:

```lua
return { value = "rocks-tree-resolution-ok" }
```

`expected/recipes/smoke.txt` (or whatever expected-output file the conformance harness uses — match the convention of an adjacent positive case):

```
rocks-tree-resolution-ok
```

`notes.md`:

```markdown
# rocks-share-lua-resolution

Verifies §7 search-path order: `require("rockmod")` resolves
`cook_modules/share/lua/5.4/rockmod.lua` (the LuaRocks-style path, not
the hand-vendored top-level) when the top-level location is empty.
```

Match the file conventions of an adjacent positive case (e.g., `017-use-from-execute/`) for any framework-specific files (`harness.json`, etc.).

- [ ] **T4.17: Run the conformance harness**

```bash
cd /home/alex/dev/cook
cargo test -p cook-lang --test conformance 2>&1 | tail -10
```

Expected: all positive cases pass, including the new one. If the new case fails because it can't find the harness convention, inspect `017-use-from-execute/` and copy its file shape exactly.

- [ ] **T4.18: Pre-commit hook check**

The pre-commit hook is paranoid about Standard updates landing alongside language-surface code. Stage everything and run the hook in dry-run mode:

```bash
cd /home/alex/dev/cook
git add cli/crates/cook-luaotp/src/pool.rs \
        standard/src/content/docs/07-cross-cookfile-composition.mdx \
        standard/src/content/docs/appendix/B-rationale.mdx \
        standard/src/content/docs/appendix/D-conformance-summary.mdx \
        standard/conformance/positive/${NEW_NUM}-rocks-share-lua-resolution/ \
        cli/crates/cook-engine/tests/fixtures/c-rock/ \
        cook_modules/luarocks_phase2.lua \
        Cookfile

# Stage the deletion of the old fixture path
git rm -r tests/fixtures/c-ext-hello 2>/dev/null || true

git status -s | head
.githooks/pre-commit
```

If the hook fails citing a missing Standard pair, double-check the Standard MDX edits are staged. **Do NOT** set `COOK_STANDARD_BYPASS=1`. Fix the missing piece instead.

- [ ] **T4.19: Run the full test suite**

```bash
cd /home/alex/dev/cook/cli && cargo test 2>&1 | tail -20
```

Expected: all tests pass, including the new pool tests, the conformance suite, and the Phase 2 gate-m2 test (if it's part of the workspace `cargo test`).

- [ ] **T4.20: Commit (single PR — co-PR rule)**

```bash
cd /home/alex/dev/cook
git commit -m "$(cat <<'EOF'
feat(phase3): M3.5+M3.6 — runtime package.{path,cpath} + Standard §7

M3.5: rename refresh_package_path → refresh_package_search_paths in
cook-luaotp::pool. Grow it to set package.cpath alongside package.path so
LuaRocks-installed rocks under cook_modules/share/lua/5.4/ and
cook_modules/lib/lua/5.4/ resolve via bare require(). Per-unit refresh
stays idempotent via _cook_original_{path,cpath} stashing.

M3.6: Standard §7 grows a new normative search-path-order clause; App. B
rationale explains the LuaRocks-tree adoption; App. D adds CS-XXXX. New
positive conformance case under standard/conformance/positive/.

Phase 2 fixture relocation: tests/fixtures/c-ext-hello/ →
cli/crates/cook-engine/tests/fixtures/c-rock/. The manual package.cpath
prepends in chore "gate-m2"'s generated Cookfiles are deleted; the
runtime now does that work.

Co-PR per spec-first rule (feedback_spec_first_no_bypass.md).
COOK_STANDARD_BYPASS unused.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 5 (M3.7 + M3.8): rocks.usecook.com defaults + `cook_smoke` rock publish

**Files:**
- Modify: `cli/crates/cook-cli/src/modules/manifest.rs` (fill in `ManifestRegistry::default()`)
- Create: `rocks/cook_smoke/cook_smoke-0.1.0-1.rockspec`
- Create: `rocks/cook_smoke/cook_smoke.lua`
- Create: `rocks/cook_smoke/README.md`
- Manual step (not a code change): publish to rocks.usecook.com per SHI-180's hosting procedure.

**Slice boundary:** Owns the `default()` impl in `manifest.rs` (Task 1 left it as a stub returning `Vec::new()`) and the new `rocks/cook_smoke/` directory. Does NOT touch any other field on `ManifestRegistry` and does NOT touch parsing logic. Does NOT touch `pull/` or `cook.toml [registry].url`.

### Code change

- [ ] **T5.1: Update `ManifestRegistry::default()`**

Edit `cli/crates/cook-cli/src/modules/manifest.rs`. Find:

```rust
    pub fn default() -> Self {
        // Task 5 replaces this with:
        //   Self {
        //       indexes: vec![
        //           "https://rocks.usecook.com".to_string(),
        //           "https://luarocks.org".to_string(),
        //       ],
        //   }
        Self { indexes: Vec::new() }
    }
```

Replace with:

```rust
    pub fn default() -> Self {
        Self {
            indexes: vec![
                "https://rocks.usecook.com".to_string(),
                "https://luarocks.org".to_string(),
            ],
        }
    }
```

- [ ] **T5.2: Update the manifest tests' expectations**

The tests in Task 1 asserted `r == ManifestRegistry::default()` for cases where `indexes` was missing or empty. They'll keep passing — `default()` simply changed value. But add an explicit test that the constants are what we documented.

Edit the inline `tests` module in `cli/crates/cook-cli/src/modules/manifest.rs`. Add:

```rust
    #[test]
    fn default_registry_has_documented_indexes() {
        let r = ManifestRegistry::default();
        assert_eq!(
            r.indexes,
            vec![
                "https://rocks.usecook.com".to_string(),
                "https://luarocks.org".to_string(),
            ]
        );
    }
```

- [ ] **T5.3: Run tests**

```bash
cd /home/alex/dev/cook/cli && cargo test -p cook-cli modules::manifest 2>&1 | tail -10
```

Expected: 9 passed (the 8 existing + the new `default_registry_has_documented_indexes`).

### `cook_smoke` rock authoring

- [ ] **T5.4: Create the `rocks/cook_smoke/` directory**

```bash
cd /home/alex/dev/cook
mkdir -p rocks/cook_smoke
```

- [ ] **T5.5: Write the rockspec**

Create `rocks/cook_smoke/cook_smoke-0.1.0-1.rockspec`:

```lua
package = "cook_smoke"
version = "0.1.0-1"
source = {
   url = "https://rocks.usecook.com/cook_smoke-0.1.0-1.src.rock",
}
description = {
   summary = "Phase 3 acceptance fixture for cook modules pipeline",
   detailed = [[
      Throwaway rock used by SHI-176 Phase 3 to validate the cook modules
      install pipeline against rocks.usecook.com end-to-end. Exposes a
      single function: cook_smoke.value() returns 42.
   ]],
   homepage = "https://github.com/lioralabs/cook",
   license = "MIT",
   maintainer = "Liora Labs <code@lioralabs.dev>",
}
dependencies = {
   "lua >= 5.4",
}
build = {
   type = "builtin",
   modules = {
      cook_smoke = "cook_smoke.lua",
   },
}
```

- [ ] **T5.6: Write the Lua source**

Create `rocks/cook_smoke/cook_smoke.lua`:

```lua
-- cook_smoke: Phase 3 acceptance fixture rock for SHI-176.
-- Throwaway: published to rocks.usecook.com to verify the install
-- pipeline end-to-end. Not a stable API; do not depend on this from
-- production Cookfiles.

local M = {}

function M.value()
    return 42
end

return M
```

- [ ] **T5.7: Write the README**

Create `rocks/cook_smoke/README.md`:

```markdown
# cook_smoke

Phase 3 acceptance fixture for SHI-176. Published to `rocks.usecook.com`
so cook's modules-install pipeline has a real rock to exercise end-to-end.

This rock is **not stable**. It exposes one function (`cook_smoke.value()`
returns 42) and exists solely to validate that:

- `cook modules install cook_smoke` resolves against `rocks.usecook.com`.
- The resulting `cook_modules/share/lua/5.4/cook_smoke.lua` loads via
  the §7 runtime resolution.
- `cook.lock` round-trips with `cook_smoke` pinned at the published version.

Do not import `cook_smoke` from a real Cookfile. It will be deleted or
rewritten without notice.

## Publish procedure

See SHI-180 for the rocks.usecook.com upload mechanism. Quick reference:

```sh
~/.cook/bin/luarocks pack rocks/cook_smoke/cook_smoke-0.1.0-1.rockspec
# upload cook_smoke-0.1.0-1.src.rock per SHI-180
```
```

- [ ] **T5.8: Pack the rock locally to verify the rockspec is valid**

```bash
cd /home/alex/dev/cook/rocks/cook_smoke
~/.cook/bin/luarocks pack cook_smoke-0.1.0-1.rockspec
ls -la cook_smoke-0.1.0-1.src.rock
```

Expected: a `cook_smoke-0.1.0-1.src.rock` file (a tarball) is produced. If `luarocks pack` errors, fix the rockspec syntax.

- [ ] **T5.9: Manual upload to rocks.usecook.com**

Read SHI-180 for the upload mechanism (rsync to a static-file host, `gh release upload`, or Gitea Pages). Execute the upload and update the index manifest. Verify the rock is fetchable:

```bash
curl -fsSL https://rocks.usecook.com/manifest 2>&1 | grep cook_smoke || echo "MANIFEST MISSING cook_smoke"
curl -fsI https://rocks.usecook.com/cook_smoke-0.1.0-1.src.rock 2>&1 | head -3
```

Expected: the manifest lists `cook_smoke`, and the .src.rock returns HTTP 200.

⚠️ This step requires network egress and write access to the rocks.usecook.com hosting. If you do not have access, stop and surface the blocker — do not commit code that references an unpublished rock.

- [ ] **T5.10: Smoke-test the install from a fresh project**

```bash
cd /home/alex/dev/cook
mkdir -p /tmp/phase3-m3.8-smoke && cd /tmp/phase3-m3.8-smoke
cat > cook.toml <<'EOF'
[modules]
cook_smoke = "*"
EOF
~/.cook/bin/luarocks install cook_smoke --tree cook_modules \
   --server https://rocks.usecook.com \
   --server https://luarocks.org \
   2>&1 | tail -5
ls cook_modules/share/lua/5.4/
```

Expected: `cook_smoke.lua` present at `cook_modules/share/lua/5.4/cook_smoke.lua`. (We use luarocks directly here because the `cook modules install` clap surface is M3.4 / Task 6 — not yet wired.)

- [ ] **T5.11: Cleanup the smoke-test tree**

```bash
rm -rf /tmp/phase3-m3.8-smoke
```

- [ ] **T5.12: Stage and commit**

The committed code change is the `default()` constants and the rockspec/source files. The publish step is a manual side effect, not committed.

```bash
cd /home/alex/dev/cook
git add cli/crates/cook-cli/src/modules/manifest.rs rocks/cook_smoke/
# Don't commit the local cook_smoke-0.1.0-1.src.rock pack output
git rm --cached rocks/cook_smoke/cook_smoke-0.1.0-1.src.rock 2>/dev/null || true
echo 'rocks/*/*-*.src.rock' >> .gitignore
git add .gitignore
git commit -m "feat(phase3): M3.7+M3.8 — rocks.usecook.com defaults + cook_smoke fixture rock"
```

(After commit, a `.src.rock` may still exist on disk locally; it is gitignored and stays out of future commits.)

---

## Task 6 (M3.4): `cook modules` clap subcommand surface

**Files:**
- Create: `cli/crates/cook-cli/src/modules/cli.rs` (new file: subcommand args + dispatch)
- Modify: `cli/crates/cook-cli/src/modules/mod.rs` (add `pub mod cli;` + `pub fn run`)
- Modify: `cli/crates/cook-cli/src/cli.rs` (add `Cmd::Modules(ModulesArgs)` variant)
- Modify: `cli/crates/cook-cli/src/main.rs` (dispatch `Cmd::Modules`)
- Create: `cli/crates/cook-cli/tests/modules_integration.rs` (integration tests)
- Create: `cli/crates/cook-cli/tests/fixtures/phase3-online/{cook.toml,Cookfile}` (online integration fixture)

**Slice boundary:** Wave B. Depends on Tasks 1, 2, 3 having merged. Owns the clap dispatch wiring and integration tests. Does NOT edit `manifest.rs`/`lockfile.rs`/`driver.rs` — only consumes their public types.

### TDD: clap-shape tests first

- [ ] **T6.1: Add `Cmd::Modules` to the parent clap enum**

Read `cli/crates/cook-cli/src/cli.rs`. Find the `Cmd` enum (around line 69). Add a new variant after `Pull`:

```rust
    /// Manage cook modules — install, remove, update, list, search rocks.
    Modules(crate::modules::cli::ModulesArgs),
```

Then verify the import path. The reference is `crate::modules::cli::ModulesArgs`; the module exists thanks to S0.2.

- [ ] **T6.2: Write the modules-cli args + dispatch**

Create `cli/crates/cook-cli/src/modules/cli.rs`:

```rust
//! M3.4 — `cook modules` clap subcommand surface.
//!
//! Wires `install`, `remove`, `update`, `list`, `search` into the cook
//! binary's subcommand dispatch (mirroring `cook pull`'s shape — see
//! cli/crates/cook-cli/src/main.rs and cli/crates/cook-cli/src/pull/).
//!
//! This module is the orchestration layer: it reads cook.toml via M3.1,
//! reads/writes cook.lock via M3.2, drives ~/.cook/bin/luarocks via M3.3.

use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};
use clap::{Args, Subcommand};

use crate::modules::driver::RocksDriver;
use crate::modules::lockfile::{self, Lockfile};
use crate::modules::manifest::{self, ManifestModules, ManifestRegistry};

#[derive(Args, Debug, Clone)]
pub struct ModulesArgs {
    #[command(subcommand)]
    pub cmd: ModulesCmd,

    /// One-shot prefix to `[registry].indexes` for this invocation.
    #[arg(long = "registry", global = true)]
    pub registry: Option<String>,

    /// Error on any prompt instead of asking.
    #[arg(long = "non-interactive", global = true)]
    pub non_interactive: bool,

    /// Non-interactive TOFU consent (CI).
    #[arg(long = "accept-trust", global = true)]
    pub accept_trust: bool,
}

#[derive(Subcommand, Debug, Clone)]
pub enum ModulesCmd {
    /// Realise cook.toml + cook.lock into ./cook_modules. With names, add and install them.
    Install {
        /// Optional rock names. With no args, installs the locked closure.
        names: Vec<String>,
    },
    /// Drop modules from cook.toml and prune cook_modules.
    Remove {
        names: Vec<String>,
    },
    /// Bump every dep within manifest constraints, or one named dep.
    Update {
        /// Optional rock name. With no arg, updates every dep.
        name: Option<String>,
    },
    /// Read cook.lock; print installed rocks.
    List,
    /// Search configured indexes for matching rocks.
    Search { query: String },
}

/// Public entry. Returns the process exit code.
pub fn run(args: ModulesArgs) -> i32 {
    match run_inner(args) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("cook modules: {e:#}");
            1
        }
    }
}

fn run_inner(args: ModulesArgs) -> Result<()> {
    let project_dir = std::env::current_dir().context("cwd")?;
    let cook_toml = project_dir.join("cook.toml");
    let lockfile_path = project_dir.join("cook.lock");

    let (manifest, registry) = if cook_toml.exists() {
        manifest::parse_cook_toml(&cook_toml)?
    } else {
        (ManifestModules::default(), ManifestRegistry::default())
    };

    let mut indexes = registry.indexes.clone();
    if indexes.is_empty() {
        indexes = ManifestRegistry::default().indexes;
    }
    if let Some(override_url) = args.registry.clone() {
        indexes.insert(0, override_url);
    }

    let prefix = cook_prefix()?;
    let driver = RocksDriver::new(prefix, indexes, project_dir.clone());

    match args.cmd {
        ModulesCmd::Install { names } if names.is_empty() => {
            install_locked_closure(&driver, &manifest, &lockfile_path)
        }
        ModulesCmd::Install { names } => {
            install_named(&driver, &manifest, &cook_toml, &lockfile_path, &names)
        }
        ModulesCmd::Remove { names } => {
            remove_named(&driver, &manifest, &cook_toml, &lockfile_path, &names)
        }
        ModulesCmd::Update { name } => {
            update_one_or_all(&driver, &manifest, &lockfile_path, name)
        }
        ModulesCmd::List => list_installed(&lockfile_path),
        ModulesCmd::Search { query } => {
            for hit in driver.search(&query)? {
                println!("{}\t{}\t{}", hit.name, hit.version, hit.index);
            }
            Ok(())
        }
    }
}

fn cook_prefix() -> Result<PathBuf> {
    // ~/.cook/ — same convention as Phase 1's install layout.
    let home = dirs::home_dir().ok_or_else(|| anyhow!("HOME not set"))?;
    Ok(home.join(".cook"))
}

fn install_locked_closure(
    driver: &RocksDriver,
    manifest: &ManifestModules,
    lockfile_path: &std::path::Path,
) -> Result<()> {
    if !lockfile_path.exists() {
        // No lockfile and no positional args: do a fresh install of the manifest.
        for (name, constraint) in &manifest.modules {
            driver.install(name, constraint)?;
        }
        let lock = lockfile::introspect_closure(
            &lockfile_path
                .parent()
                .unwrap_or(std::path::Path::new("."))
                .join("cook_modules"),
            manifest,
        )?;
        lockfile::write(lockfile_path, &lock)?;
        return Ok(());
    }
    let lock = lockfile::read(lockfile_path)?;
    validate_lockfile_consistent(&lock, manifest)?;
    for locked in &lock.modules {
        driver.install_locked(locked)?;
    }
    Ok(())
}

fn install_named(
    driver: &RocksDriver,
    manifest: &ManifestModules,
    cook_toml: &std::path::Path,
    lockfile_path: &std::path::Path,
    names: &[String],
) -> Result<()> {
    let mut updated = manifest.clone();
    for name in names {
        let (rock, constraint) = parse_name_at_version(name);
        updated.modules.insert(rock.clone(), constraint.clone());
        driver.install(&rock, &constraint)?;
    }
    write_manifest_modules(cook_toml, &updated)?;
    let lock = lockfile::introspect_closure(
        &cook_toml
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .join("cook_modules"),
        &updated,
    )?;
    lockfile::write(lockfile_path, &lock)?;
    Ok(())
}

fn remove_named(
    driver: &RocksDriver,
    manifest: &ManifestModules,
    cook_toml: &std::path::Path,
    lockfile_path: &std::path::Path,
    names: &[String],
) -> Result<()> {
    let mut updated = manifest.clone();
    for name in names {
        updated.modules.remove(name);
        driver.remove(name)?;
    }
    write_manifest_modules(cook_toml, &updated)?;
    let lock = lockfile::introspect_closure(
        &cook_toml
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .join("cook_modules"),
        &updated,
    )?;
    lockfile::write(lockfile_path, &lock)?;
    Ok(())
}

fn update_one_or_all(
    driver: &RocksDriver,
    manifest: &ManifestModules,
    lockfile_path: &std::path::Path,
    name: Option<String>,
) -> Result<()> {
    let names: Vec<String> = match name {
        Some(n) => vec![n],
        None => manifest.modules.keys().cloned().collect(),
    };
    for n in &names {
        let constraint = manifest.modules.get(n).cloned().unwrap_or_else(|| "*".into());
        driver.install(n, &constraint)?;
    }
    let lock = lockfile::introspect_closure(
        &lockfile_path
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .join("cook_modules"),
        manifest,
    )?;
    lockfile::write(lockfile_path, &lock)?;
    Ok(())
}

fn list_installed(lockfile_path: &std::path::Path) -> Result<()> {
    if !lockfile_path.exists() {
        eprintln!("no cook.lock found; nothing installed");
        return Ok(());
    }
    let lock = lockfile::read(lockfile_path)?;
    for m in &lock.modules {
        let kind = if m.direct { "direct" } else { "transitive" };
        println!("{}\t{}\t{}", m.name, m.version, kind);
    }
    Ok(())
}

fn validate_lockfile_consistent(lock: &Lockfile, manifest: &ManifestModules) -> Result<()> {
    let direct_in_lock: std::collections::BTreeSet<&str> = lock
        .modules
        .iter()
        .filter(|m| m.direct)
        .map(|m| m.name.as_str())
        .collect();
    let manifest_names: std::collections::BTreeSet<&str> =
        manifest.modules.keys().map(String::as_str).collect();
    if direct_in_lock != manifest_names {
        return Err(anyhow!(
            "cook.lock direct deps ({:?}) disagree with [modules] ({:?}); \
             run `cook modules install <name>` or `cook modules update`",
            direct_in_lock,
            manifest_names,
        ));
    }
    Ok(())
}

fn parse_name_at_version(spec: &str) -> (String, String) {
    if let Some((name, ver)) = spec.split_once('@') {
        (name.to_string(), ver.to_string())
    } else {
        (spec.to_string(), "*".to_string())
    }
}

fn write_manifest_modules(path: &std::path::Path, manifest: &ManifestModules) -> Result<()> {
    let mut existing = if path.exists() {
        std::fs::read_to_string(path).context("read cook.toml")?
    } else {
        String::new()
    };
    // Preserve [registry] block; replace [modules] section.
    if let Some(pos) = existing.find("[modules]") {
        // Trim from [modules] to next [...] or EOF.
        let after = &existing[pos..];
        let end_rel = after[1..]
            .find("\n[")
            .map(|p| pos + 1 + p)
            .unwrap_or(existing.len());
        existing.replace_range(pos..end_rel, "");
    }
    let mut block = String::from("[modules]\n");
    for (name, constraint) in &manifest.modules {
        let key = if name.contains('-') || name.contains('.') {
            format!("\"{}\"", name)
        } else {
            name.clone()
        };
        block.push_str(&format!("{} = \"{}\"\n", key, constraint));
    }
    if !existing.is_empty() && !existing.ends_with('\n') {
        existing.push('\n');
    }
    existing.push_str(&block);
    std::fs::write(path, existing).context("write cook.toml")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_name_at_version_separates_name_and_constraint() {
        assert_eq!(
            parse_name_at_version("cook_smoke@0.1.0-1"),
            ("cook_smoke".into(), "0.1.0-1".into())
        );
        assert_eq!(
            parse_name_at_version("cook_smoke"),
            ("cook_smoke".into(), "*".into())
        );
    }

    #[test]
    fn validate_lockfile_consistent_passes_on_match() {
        let mut manifest = ManifestModules::default();
        manifest.modules.insert("cook_smoke".into(), "*".into());
        let lock = Lockfile::new(vec![lockfile::LockedModule {
            name: "cook_smoke".into(),
            version: "0.1.0-1".into(),
            source: "https://rocks.usecook.com/cook_smoke-0.1.0-1.src.rock".into(),
            integrity: "sha256-x".into(),
            direct: true,
        }]);
        validate_lockfile_consistent(&lock, &manifest).expect("ok");
    }

    #[test]
    fn validate_lockfile_consistent_errors_on_drift() {
        let mut manifest = ManifestModules::default();
        manifest.modules.insert("cook_smoke".into(), "*".into());
        let lock = Lockfile::new(Vec::new());
        let err = validate_lockfile_consistent(&lock, &manifest).expect_err("must fail");
        assert!(format!("{:#}", err).contains("disagree"));
    }
}
```

- [ ] **T6.3: Re-export the module-cli surface from `modules/mod.rs`**

Edit `cli/crates/cook-cli/src/modules/mod.rs`. Replace its contents with:

```rust
//! `cook modules` — manifest, lockfile, and LuaRocks subprocess driver.

pub mod cli;
pub mod driver;
pub mod lockfile;
pub mod manifest;

pub use cli::{run, ModulesArgs};
```

- [ ] **T6.4: Wire dispatch in `main.rs`**

Edit `cli/crates/cook-cli/src/main.rs`. Find the `Cmd::Pull(args) => ...` line. Add immediately before or after:

```rust
        Some(Cmd::Modules(args)) => std::process::exit(modules::run(args)),
```

Then ensure `modules` is in the `use` declarations at the top. Find the existing `use cook_cli::pull` (or `mod pull`) — match the existing pattern. Add:

```rust
use cook_cli::modules;
```

(or `mod modules;` if the file uses that style — match.)

- [ ] **T6.5: Build to confirm dispatch compiles**

```bash
cd /home/alex/dev/cook/cli && cargo build -p cook-cli 2>&1 | tail -10
```

Expected: clean build.

- [ ] **T6.6: Smoke-test the help text**

```bash
cd /home/alex/dev/cook/cli && cargo run -p cook-cli -- modules --help 2>&1 | tail -20
```

Expected: clap shows the `install`, `remove`, `update`, `list`, `search` subcommands and the `--registry`, `--non-interactive`, `--accept-trust` global flags.

- [ ] **T6.7: Run the unit tests**

```bash
cd /home/alex/dev/cook/cli && cargo test -p cook-cli modules::cli 2>&1 | tail -10
```

Expected: 3 passed (parse_name_at_version, validate_lockfile_consistent_passes_on_match, validate_lockfile_consistent_errors_on_drift).

### Integration tests

- [ ] **T6.8: Create the online fixture**

```bash
cd /home/alex/dev/cook
mkdir -p cli/crates/cook-cli/tests/fixtures/phase3-online
```

Create `cli/crates/cook-cli/tests/fixtures/phase3-online/cook.toml`:

```toml
[registry]
indexes = ["https://rocks.usecook.com", "https://luarocks.org"]

[modules]
cook_smoke = "*"
```

Create `cli/crates/cook-cli/tests/fixtures/phase3-online/Cookfile`:

```cook
use cook_smoke

recipe smoke
    > local s = require("cook_smoke")
    > print(s.value())
end
```

- [ ] **T6.9: Write the integration test**

Create `cli/crates/cook-cli/tests/modules_integration.rs`:

```rust
//! M3.4 integration tests. Two flavors:
//!   - Offline tests: drive the clap surface against the real RocksDriver
//!     pointed at a fake-luarocks shim that records argv (no network).
//!   - Online tests: gated on `cook chore gate-m2` having been run, install
//!     cook_smoke from rocks.usecook.com via the bundled luarocks.
//!
//! Online tests require network egress + a populated rocks.usecook.com.
//! Run with `--ignored` to enable: `cargo test -p cook-cli --test modules_integration -- --ignored`.

use std::path::{Path, PathBuf};
use std::process::Command;

fn cook_binary() -> PathBuf {
    // Built by cargo before tests run.
    PathBuf::from(env!("CARGO_BIN_EXE_cook"))
}

fn fixture_dir(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

#[test]
fn modules_help_lists_subcommands() {
    let out = Command::new(cook_binary())
        .args(["modules", "--help"])
        .output()
        .expect("spawn");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("install"));
    assert!(stdout.contains("remove"));
    assert!(stdout.contains("update"));
    assert!(stdout.contains("list"));
    assert!(stdout.contains("search"));
}

#[test]
fn modules_list_in_empty_project_says_no_lockfile() {
    let dir = tempfile::tempdir().expect("tempdir");
    let out = Command::new(cook_binary())
        .args(["modules", "list"])
        .current_dir(dir.path())
        .output()
        .expect("spawn");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("no cook.lock") || stderr.is_empty());
}

#[test]
#[ignore = "requires ~/.cook/bin/luarocks (gate-m2 must be green) + network egress"]
fn online_install_cook_smoke_from_fixture_project() {
    let src = fixture_dir("phase3-online");
    let dir = tempfile::tempdir().expect("tempdir");
    copy_dir_all(&src, dir.path()).expect("copy fixture");

    let install = Command::new(cook_binary())
        .args(["modules", "install"])
        .current_dir(dir.path())
        .output()
        .expect("spawn install");
    assert!(
        install.status.success(),
        "install failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&install.stdout),
        String::from_utf8_lossy(&install.stderr),
    );

    let installed = dir
        .path()
        .join("cook_modules/share/lua/5.4/cook_smoke.lua");
    assert!(
        installed.exists(),
        "cook_smoke.lua missing at {}",
        installed.display()
    );

    let lock = dir.path().join("cook.lock");
    assert!(lock.exists(), "cook.lock not written");

    let smoke = Command::new(cook_binary())
        .arg("smoke")
        .current_dir(dir.path())
        .output()
        .expect("spawn smoke");
    let stdout = String::from_utf8_lossy(&smoke.stdout);
    assert!(stdout.contains("42"), "smoke recipe should print 42; got: {stdout}");
}

fn copy_dir_all(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let dst_path = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_all(&entry.path(), &dst_path)?;
        } else {
            std::fs::copy(entry.path(), dst_path)?;
        }
    }
    Ok(())
}
```

- [ ] **T6.10: Run the offline integration tests**

```bash
cd /home/alex/dev/cook/cli && cargo test -p cook-cli --test modules_integration 2>&1 | tail -15
```

Expected: 2 passed, 1 ignored (the online test).

- [ ] **T6.11: Run the online integration test (gated)**

This step requires `cook chore gate-m2` to have been run (so `~/.cook/bin/luarocks` exists) and network egress to rocks.usecook.com.

```bash
cd /home/alex/dev/cook/cli && cargo test -p cook-cli --test modules_integration -- --ignored 2>&1 | tail -20
```

Expected: 1 passed (the online test). If this fails because rocks.usecook.com hasn't received the cook_smoke upload from Task 5, surface the blocker — you cannot pass this test until M3.8's manual upload is complete.

If you want to defer the online test (e.g., running on CI without network), document the deferred status in the commit message and open a follow-up task to run it on a network-enabled host.

- [ ] **T6.12: Verify lints clean**

```bash
cd /home/alex/dev/cook/cli && cargo clippy -p cook-cli --tests -- -D warnings 2>&1 | tail -10
```

Expected: no warnings.

- [ ] **T6.13: Commit**

```bash
cd /home/alex/dev/cook
git add cli/crates/cook-cli/src/modules/cli.rs \
        cli/crates/cook-cli/src/modules/mod.rs \
        cli/crates/cook-cli/src/cli.rs \
        cli/crates/cook-cli/src/main.rs \
        cli/crates/cook-cli/tests/modules_integration.rs \
        cli/crates/cook-cli/tests/fixtures/phase3-online/
git commit -m "feat(phase3): M3.4 — cook modules clap subcommand surface + integration tests"
```

---

## Task 7: Phase 3 cumulative acceptance gate

**Files:**
- Create: `cli/crates/cook-cli/tests/fixtures/phase3-acceptance/{cook.toml,Cookfile}` (multi-rock fixture)

**Slice boundary:** Acceptance verification only. Touches no production code; depends on Tasks 1–6 having merged. Runs on linux-x86_64 (locally) and darwin-arm64 (manually on `mini` until SHI-187 lands the runner).

### Build the cumulative fixture

- [ ] **T7.1: Create the acceptance fixture**

```bash
cd /home/alex/dev/cook
mkdir -p cli/crates/cook-cli/tests/fixtures/phase3-acceptance
```

Create `cli/crates/cook-cli/tests/fixtures/phase3-acceptance/cook.toml`:

```toml
[registry]
indexes = ["https://rocks.usecook.com", "https://luarocks.org"]

[modules]
cook_smoke  = "*"
"lua-cjson" = ">=2.1"
argparse    = "*"
```

Create `cli/crates/cook-cli/tests/fixtures/phase3-acceptance/Cookfile`:

```cook
use cook_smoke
use lua-cjson as cjson
use argparse

recipe smoke
    > local s = require("cook_smoke")
    > local out = cjson.encode({hello = "world", value = s.value()})
    > print(out)
    > local p = argparse("smoke", "test")
    > print("argparse loaded:", p ~= nil)
end
```

### Run the gate (Linux)

- [ ] **T7.2: Stage a fresh worktree for the gate**

```bash
cd /home/alex/dev/cook
GATE_DIR=$(mktemp -d -t phase3-gate-XXXX)
cp -r cli/crates/cook-cli/tests/fixtures/phase3-acceptance/. "$GATE_DIR/"
cd "$GATE_DIR"
ls
```

Expected: `cook.toml` and `Cookfile` in place.

- [ ] **T7.3: Step 1 — `cook modules install`**

```bash
~/.cook/bin/cook modules install 2>&1 | tail -5
```

Expected: install succeeds. The output references downloads from rocks.usecook.com and luarocks.org.

- [ ] **T7.4: Step 2 — verify rocks landed at the expected paths**

```bash
ls cook_modules/share/lua/5.4/cook_smoke.lua
ls cook_modules/lib/lua/5.4/cjson.so
ls cook_modules/share/lua/5.4/argparse.lua
```

Expected: all three files exist.

- [ ] **T7.5: Step 3 — verify `cook.lock` shape**

```bash
cat cook.lock | head -40
```

Expected:
- `schema = 1` at the top.
- `cook_smoke`, `lua-cjson`, `argparse` each have a `[[module]]` entry with `direct = true`.
- At least one transitive (e.g., `lpeg` for argparse) has `direct = false`.

- [ ] **T7.6: Step 4 — run the smoke recipe**

```bash
~/.cook/bin/cook smoke 2>&1 | tail -10
```

Expected output contains:
- `"value":42` and `"hello":"world"` (from cjson.encode).
- `argparse loaded: true`.

- [ ] **T7.7: Step 5 — re-running install is a no-op**

```bash
~/.cook/bin/cook modules install 2>&1 | tail -5
```

Expected: install completes quickly with no fresh downloads (the lockfile-replay path; integrity-verifiable cache hits only).

- [ ] **T7.8: Step 6 — `cook modules remove cook_smoke` purges the rock**

```bash
~/.cook/bin/cook modules remove cook_smoke 2>&1 | tail -5
ls cook_modules/share/lua/5.4/cook_smoke.lua 2>&1 || echo "removed (expected)"
grep cook_smoke cook.lock 2>&1 || echo "absent from lockfile (expected)"
cat cook.toml | grep cook_smoke 2>&1 || echo "absent from cook.toml (expected)"
```

Expected: cook_smoke.lua is gone, no cook_smoke in cook.lock, no cook_smoke in cook.toml.

- [ ] **T7.9: Step 7 — conformance harness green**

```bash
cd /home/alex/dev/cook
cargo test -p cook-lang --test conformance 2>&1 | tail -5
```

Expected: all positive cases pass, including the rocks-share-lua-resolution case from Task 4.

### macOS verification (manual on `mini`)

- [ ] **T7.10: Push the integration branch**

```bash
cd /home/alex/dev/cook
git push origin feature/luarocks-modules
```

- [ ] **T7.11: Run the gate on `mini`**

SSH into the macOS build slave (`gilberthouse.story-pike.ts.net` Tailnet — or whatever the host is currently named in your `~/.ssh/config`) and re-run T7.2–T7.8:

```bash
ssh mini
cd ~/dev/cook
git fetch
git checkout feature/luarocks-modules
git pull
cargo build --release -p cook-cli 2>&1 | tail -3
# then repeat T7.2 through T7.8 verbatim
```

Capture the full stdout/stderr to a file:

```bash
GATE_DIR=$(mktemp -d -t phase3-gate-XXXX)
cp -r cli/crates/cook-cli/tests/fixtures/phase3-acceptance/. "$GATE_DIR/"
cd "$GATE_DIR"
{
  echo "=== step 1: cook modules install ==="
  ~/.cook/bin/cook modules install
  echo "=== step 2: file presence ==="
  ls cook_modules/share/lua/5.4/cook_smoke.lua \
      cook_modules/lib/lua/5.4/cjson.so \
      cook_modules/share/lua/5.4/argparse.lua
  echo "=== step 3: cook.lock ==="
  cat cook.lock
  echo "=== step 4: smoke recipe ==="
  ~/.cook/bin/cook smoke
  echo "=== step 5: re-run install ==="
  ~/.cook/bin/cook modules install
  echo "=== step 6: remove cook_smoke ==="
  ~/.cook/bin/cook modules remove cook_smoke
  ls cook_modules/share/lua/5.4/cook_smoke.lua 2>&1 || echo "removed"
} > /tmp/phase3-gate-darwin.log 2>&1
cat /tmp/phase3-gate-darwin.log
```

Post the captured `/tmp/phase3-gate-darwin.log` content as a comment on the M3 cumulative-gate Linear ticket.

- [ ] **T7.12: Cleanup the gate dir**

```bash
rm -rf "$GATE_DIR"
```

- [ ] **T7.13: Commit the acceptance fixture**

```bash
cd /home/alex/dev/cook
git add cli/crates/cook-cli/tests/fixtures/phase3-acceptance/
git commit -m "test(phase3): cumulative acceptance gate fixture (cook_smoke + lua-cjson + argparse)"
```

- [ ] **T7.14: Merge `feature/luarocks-modules` to `main`**

Once linux-x86_64 and darwin-arm64 gates are both green:

```bash
cd /home/alex/dev/cook
git checkout main
git pull
git merge --no-ff feature/luarocks-modules -m "Merge SHI-176 Phase 3 (M3): cook modules CLI"
git push origin main
```

This is the final Phase 3 deliverable. Phase 4 (blessed-module rock migration + `cook pull` deletion) is unblocked.

---

## Self-review checklist

After all tasks complete, verify:

- [ ] **Spec coverage** — every section in `2026-05-10-luarocks-phase-3-design.md` has at least one corresponding task. Specifically: M3.1 (Task 1), M3.2 (Task 2), M3.3 (Task 3), M3.5+M3.6 (Task 4), M3.7+M3.8 (Task 5), M3.4 (Task 6), cumulative gate (Task 7).
- [ ] **Standard co-PR rule** — Task 4's commit contains both `cli/crates/cook-luaotp/src/pool.rs` and `standard/src/content/docs/07-cross-cookfile-composition.mdx`. The pre-commit hook accepted it. `COOK_STANDARD_BYPASS` was never used.
- [ ] **`cook pull` untouched** — `cli/crates/cook-cli/src/pull/` is unchanged across Phase 3. `[registry].url` in cook.toml schema is unchanged.
- [ ] **Underscore naming** — `rocks/cook_smoke/` uses underscore; the rockspec's `package = "cook_smoke"`; no hyphenated cook-* names anywhere.
- [ ] **Worktree hygiene** — every commit cleanly applies on `feature/luarocks-modules` from main; no spurious files (gitignored `.src.rock` packs absent from history).
- [ ] **Gate output** — both linux-x86_64 and darwin-arm64 gate logs are captured and attached to the M3 cumulative-gate ticket.
