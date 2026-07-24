//! cook.require_env runtime helper per CS-0033 §3.2 step 4.
//!
//! After config-block evaluation completes, the engine calls
//! `EnvKeyset::freeze` to capture the set of declared env-var names. From
//! that point forward, `cook.require_env(name)` raises a Lua error if
//! `name` is not in the captured set; otherwise it returns the env value
//! (which may be the empty string).

use mlua::prelude::*;
use std::cell::RefCell;
use std::collections::BTreeSet;
use std::rc::Rc;

/// Per-Lua-state storage for the frozen env keyset.
#[derive(Default, Clone)]
pub struct EnvKeyset {
    inner: Rc<RefCell<Option<BTreeSet<String>>>>,
}

impl EnvKeyset {
    pub fn new() -> Self {
        Self::default()
    }

    /// Capture the current `cook.env` table's keyset as the declared set.
    ///
    /// Idempotent under union: subsequent calls add new keys to the set.
    /// Config blocks may execute multiple times under config presets, but the
    /// declared set is the union across all runs.
    pub fn freeze(&self, env_table: &LuaTable) -> mlua::Result<()> {
        let mut existing = self.inner.borrow_mut();
        let mut set = existing.take().unwrap_or_default();
        for pair in env_table.clone().pairs::<String, LuaValue>() {
            let (key, _) = pair?;
            set.insert(key);
        }
        *existing = Some(set);
        Ok(())
    }

    pub fn contains(&self, key: &str) -> bool {
        self.inner
            .borrow()
            .as_ref()
            .map(|s| s.contains(key))
            .unwrap_or(false)
    }

    pub fn declared_list(&self) -> Vec<String> {
        self.inner
            .borrow()
            .as_ref()
            .map(|s| s.iter().cloned().collect())
            .unwrap_or_default()
    }
}

/// Case-insensitive Levenshtein distance.
fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<u8> = a.to_ascii_uppercase().into_bytes();
    let b: Vec<u8> = b.to_ascii_uppercase().into_bytes();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut cur = Vec::with_capacity(b.len() + 1);
        cur.push(i + 1);
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            cur.push((prev[j] + cost).min(prev[j + 1] + 1).min(cur[j] + 1));
        }
        prev = cur;
    }
    prev[b.len()]
}

/// The `n` declared names closest to `name` by edit distance (ties broken
/// lexicographically for determinism).
///
/// Shared with `cook.require_recipe`'s unknown-name diagnostic (Standard
/// §22.8, CS-0144), which suggests the closest registered recipe names the
/// same way this suggests the closest declared env vars.
pub(crate) fn closest_declared(name: &str, declared: &[String], n: usize) -> Vec<String> {
    let mut scored: Vec<(usize, &String)> =
        declared.iter().map(|d| (edit_distance(name, d), d)).collect();
    scored.sort_by(|x, y| x.0.cmp(&y.0).then_with(|| x.1.cmp(y.1)));
    scored.into_iter().take(n).map(|(_, d)| d.clone()).collect()
}

/// Install `cook.require_env(name)` on the given `cook` table.
///
/// The function checks `name` against the frozen keyset. If `name` is in the
/// set, it returns `cook.env[name]` (possibly the empty string). If `name` is
/// not in the set, it raises a `RuntimeError` with a diagnostic that suggests
/// the closest declared names by edit distance (never the full declared/host
/// set — that can include the entire host environment) and the recommended
/// declaration form.
pub fn install_require_env(
    lua: &Lua,
    cook_table: &LuaTable,
    keyset: EnvKeyset,
) -> mlua::Result<()> {
    let env_table: LuaTable = cook_table.get("env")?;
    let env_clone = env_table.clone();
    let ks = keyset.clone();
    let f = lua.create_function(move |_, name: String| -> mlua::Result<LuaValue> {
        if !ks.contains(&name) {
            let declared = ks.declared_list();
            let msg = if declared.is_empty() {
                format!(
                    "placeholder $<{}>: env var '{}' was not declared in any config block; \
                     declare it with `var.{} = host.env(\"{}\", \"\")` (or similar) in a config block",
                    name, name, name, name
                )
            } else {
                let closest = closest_declared(&name, &declared, 3);
                format!(
                    "placeholder $<{}>: env var '{}' was not declared. Closest declared names: {}. \
                     Add `var.{} = ...` to a config block.",
                    name,
                    name,
                    closest.join(", "),
                    name
                )
            };
            return Err(mlua::Error::RuntimeError(msg));
        }
        env_clone.get::<LuaValue>(name)
    })?;
    cook_table.set("require_env", f)?;
    Ok(())
}

#[cfg(test)]
#[path = "tests/env_api_tests.rs"]
mod tests;
