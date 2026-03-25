# DDD Audit TODO — Cook Monorepo

Audit date: 2026-03-20

**Overall:** The 10-crate split is well-executed. 5 findings, none critical.

---

## Priority 1: cook-engine leaks internal crate types (MEDIUM)

`cook-engine/src/run.rs` — `run()` signature exposes `cook_register::Registry` and `cook_luaotp::TestOutput`, forcing cook-cli to depend on internal engine crates.

**Fix:** Have cook-engine accept raw data (working dirs, env vars, lua sources) instead of `Registry` instances. Define a `RegistrySource` struct in cook-engine. Re-export or wrap `TestOutput` in cook-engine's own type.

**Files:** `cli/crates/cook-engine/src/run.rs`, `cli/crates/cook-cli/src/pipeline.rs`

---

## Priority 2: display_name() in cook-contracts (MEDIUM)

`cook-contracts/src/lib.rs:36-51` — `WorkPayload::display_name()` is presentation logic in the shared kernel. Spec says cook-contracts should be behavior-free.

**Fix:** Move to cook-cli as a helper function or extension trait.

**Files:** `cli/crates/cook-contracts/src/lib.rs`, `cli/crates/cook-cli/src/` (new helper)

---

## Priority 3: Unused deps in cook-cli Cargo.toml (LOW)

`cook-cli/Cargo.toml` declares `cook-cache`, `cook-dag`, `cook-luaotp`, `cook-contracts` but never imports them via `use`. Remove and verify build.

**Files:** `cli/crates/cook-cli/Cargo.toml`

---

## Priority 4: cook-register exposes all submodules publicly (LOW)

`cook-register/src/lib.rs:6-16` — All 11 submodules are `pub mod`. Only `Registry`, `register_fs_api`, `register_path_api` are part of the intended API. Internal modules should use `pub(crate) mod`.

**Files:** `cli/crates/cook-register/src/lib.rs`

---

## Priority 5: Duplicate hash_str (LOW — ACCEPT)

Both `cook-register/src/lib.rs:75` and `cook-cache/src/lib.rs:21` define identical `hash_str()`. Accepted trade-off — keeping the crates decoupled is more valuable than DRY for a 1-line function.

**Action:** No change needed.

---

## Verification

```bash
cd ~/dev/cook/cli
cargo build && cargo test
```
