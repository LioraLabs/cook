//! The declared-variable surface: the read-only `var` global and
//! `cook.require_var` (CS-0172, Standard §5.3, §21.4).
//!
//! A Cookfile's `config` blocks write declared variables onto their `var` sink
//! (§5.3.1). This module exposes those values to everything else:
//!
//! - **`var.NAME`** — a read-only global table, in scope for register-phase
//!   Lua (top-level `module_call`s, `register` blocks, recipe bodies, module
//!   bodies). Reads return the value with its declared Lua type, so a
//!   boolean-valued knob reaches a module option as a boolean.
//! - **`cook.require_var("NAME")`** — the `$<NAME>` lowering target. Returns
//!   the value's *string* form, because a placeholder interpolates into a
//!   command; codegen records the key in the unit's determinants (§17.1.2).
//!
//! Both raise on a name no config block declared, suggesting the closest
//! declared names by edit distance. Before CS-0172 this surface was
//! `cook.env` — the process-environment table itself — which made every
//! ambient variable an undeclared build variable and forced string-typed
//! values. The store is now private to the Lua registry: the only handles Lua
//! has are the read-only proxy and `require_var`, so a recipe body cannot
//! mutate a declared value out from under the cache key that consulted it.

use mlua::prelude::*;
use std::cell::RefCell;
use std::collections::BTreeSet;
use std::rc::Rc;

/// Per-Lua-state storage for the frozen declared-variable keyset.
#[derive(Default, Clone)]
pub struct VarKeyset {
    inner: Rc<RefCell<Option<BTreeSet<String>>>>,
}

impl VarKeyset {
    pub fn new() -> Self {
        Self::default()
    }

    /// Capture the current var store's keyset as the declared set.
    ///
    /// Idempotent under union: subsequent calls add new keys to the set.
    /// Config blocks may execute multiple times under config presets, but the
    /// declared set is the union across all runs.
    pub fn freeze(&self, store: &LuaTable) -> mlua::Result<()> {
        let mut existing = self.inner.borrow_mut();
        let mut set = existing.take().unwrap_or_default();
        for pair in store.clone().pairs::<String, LuaValue>() {
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

/// The declared-variable store: the table config blocks write and everything
/// else reads through. Held in the Lua registry, not on a global.
pub fn var_store(lua: &Lua) -> mlua::Result<LuaTable> {
    lua.named_registry_value::<LuaTable>(crate::VAR_STORE_REGISTRY_KEY)
}

/// Render a declared variable's value in the form a `$<NAME>` placeholder
/// interpolates and the cache key hashes (§5.3.1).
///
/// Booleans render `true` / `false`; an integer-valued number renders without
/// a decimal point so `var.jobs = 8` interpolates as `8`, not `8.0`.
pub fn var_to_string(name: &str, value: &LuaValue) -> mlua::Result<String> {
    match value {
        LuaValue::String(s) => Ok(s.to_str()?.to_string()),
        LuaValue::Boolean(b) => Ok(if *b { "true" } else { "false" }.to_string()),
        LuaValue::Integer(i) => Ok(i.to_string()),
        LuaValue::Number(n) => {
            if n.fract() == 0.0 && n.is_finite() && n.abs() < 1e15 {
                Ok(format!("{}", *n as i64))
            } else {
                Ok(format!("{}", n))
            }
        }
        other => Err(mlua::Error::RuntimeError(format!(
            "config var '{name}' is a {}; a declared variable must be a string, \
             a number, or a boolean (Standard §5.3.1). Compose lists and tables \
             in a `register` block instead.",
            other.type_name()
        ))),
    }
}

/// The diagnostic for a read of a name no config block declared.
fn undeclared(name: &str, keyset: &VarKeyset, sigil: bool) -> mlua::Error {
    let declared = keyset.declared_list();
    let site = if sigil {
        format!("placeholder $<{name}>: ")
    } else {
        format!("var.{name}: ")
    };
    let msg = if declared.is_empty() {
        format!(
            "{site}no config block declares '{name}'; declare it with \
             `var.{name} = ...` in a config block (Standard §5.3.1)"
        )
    } else {
        format!(
            "{site}no config block declares '{name}'. Closest declared: {}. \
             Add `var.{name} = ...` to a config block.",
            closest_declared(name, &declared, 3).join(", ")
        )
    };
    mlua::Error::RuntimeError(msg)
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
/// same way this suggests the closest declared variables.
pub(crate) fn closest_declared(name: &str, declared: &[String], n: usize) -> Vec<String> {
    let mut scored: Vec<(usize, &String)> =
        declared.iter().map(|d| (edit_distance(name, d), d)).collect();
    scored.sort_by(|x, y| x.0.cmp(&y.0).then_with(|| x.1.cmp(y.1)));
    scored.into_iter().take(n).map(|(_, d)| d.clone()).collect()
}

/// Install `cook.require_var(name)` and the read-only `var` global.
///
/// `require_var` is what `$<NAME>` lowers to, so it returns the string form
/// (§5.3.1). The `var` global returns the declared Lua type. Both check the
/// frozen keyset first; before the freeze (i.e. during the config pass itself)
/// the keyset is empty and any read raises, which is correct — a config body
/// reads its own sink, not this surface.
pub fn install_var_api(lua: &Lua, cook_table: &LuaTable, keyset: VarKeyset) -> mlua::Result<()> {
    let ks = keyset.clone();
    let require_var = lua.create_function(move |lua, name: String| -> mlua::Result<String> {
        if !ks.contains(&name) {
            return Err(undeclared(&name, &ks, true));
        }
        let value: LuaValue = var_store(lua)?.get(name.clone())?;
        var_to_string(&name, &value)
    })?;
    cook_table.set("require_var", require_var)?;

    // `var` is a proxy: an empty table whose metatable routes reads to the
    // store and refuses writes. Reading through `__index` (rather than handing
    // out the store) is what keeps the declared-name check on every access.
    let proxy = lua.create_table()?;
    let meta = lua.create_table()?;
    let ks_read = keyset.clone();
    meta.set(
        "__index",
        lua.create_function(
            move |lua, (_proxy, name): (LuaValue, String)| -> mlua::Result<LuaValue> {
                if !ks_read.contains(&name) {
                    return Err(undeclared(&name, &ks_read, false));
                }
                var_store(lua)?.get::<LuaValue>(name)
            },
        )?,
    )?;
    meta.set(
        "__newindex",
        lua.create_function(
            move |_, (_proxy, name, _value): (LuaValue, String, LuaValue)| -> mlua::Result<()> {
                Err(mlua::Error::RuntimeError(format!(
                    "var.{name} is read-only outside a config block: a declared \
                     variable's value is a cache determinant, so it cannot be \
                     reassigned once recipes have been registered against it \
                     (Standard §5.3.1). Set it in a `config` block, or pass \
                     `--set {name}=...`."
                )))
            },
        )?,
    )?;
    // Hide the metatable so the guards cannot be lifted off with setmetatable.
    meta.set("__metatable", false)?;
    proxy.set_metatable(Some(meta));
    lua.globals().set("var", proxy)?;

    Ok(())
}

#[cfg(test)]
#[path = "tests/var_api_tests.rs"]
mod tests;
