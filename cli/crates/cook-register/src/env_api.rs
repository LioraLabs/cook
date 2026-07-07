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
fn closest_declared(name: &str, declared: &[String], n: usize) -> Vec<String> {
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
                     declare it with `env.{} = os.getenv(\"{}\") or \"\"` (or similar) in a config block",
                    name, name, name, name
                )
            } else {
                let closest = closest_declared(&name, &declared, 3);
                format!(
                    "placeholder $<{}>: env var '{}' was not declared. Closest declared names: {}. \
                     Add `env.{} = ...` to a config block.",
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
mod tests {
    use super::*;
    use mlua::Lua;

    fn setup() -> (Lua, LuaTable, EnvKeyset) {
        let lua = Lua::new();
        let cook: LuaTable = lua.create_table().unwrap();
        let env: LuaTable = lua.create_table().unwrap();
        cook.set("env", env).unwrap();
        let ks = EnvKeyset::new();
        install_require_env(&lua, &cook, ks.clone()).unwrap();
        lua.globals().set("cook", cook.clone()).unwrap();
        (lua, cook, ks)
    }

    #[test]
    fn returns_value_for_declared_key() {
        let (lua, cook, ks) = setup();
        let env: LuaTable = cook.get("env").unwrap();
        env.set("HOME", "/home/alex").unwrap();
        ks.freeze(&env).unwrap();
        let v: String = lua
            .load(r#"return cook.require_env("HOME")"#)
            .eval()
            .unwrap();
        assert_eq!(v, "/home/alex");
    }

    #[test]
    fn returns_empty_string_for_declared_but_empty() {
        let (lua, cook, ks) = setup();
        let env: LuaTable = cook.get("env").unwrap();
        env.set("EMPTY", "").unwrap();
        ks.freeze(&env).unwrap();
        let v: String = lua
            .load(r#"return cook.require_env("EMPTY")"#)
            .eval()
            .unwrap();
        assert_eq!(v, "");
    }

    #[test]
    fn errors_for_undeclared_key() {
        let (lua, cook, ks) = setup();
        let env: LuaTable = cook.get("env").unwrap();
        env.set("HOME", "x").unwrap();
        ks.freeze(&env).unwrap();
        let res: mlua::Result<String> =
            lua.load(r#"return cook.require_env("HOEM")"#).eval();
        assert!(res.is_err());
        let msg = format!("{}", res.unwrap_err());
        assert!(msg.contains("HOEM"), "expected HOEM in: {msg}");
        assert!(msg.contains("declared"), "expected 'declared' in: {msg}");
        assert!(msg.contains("HOME"), "expected HOME in: {msg}");
    }

    #[test]
    fn errors_for_undeclared_key_suggests_closest_matches_only() {
        let (lua, cook, ks) = setup();
        let env: LuaTable = cook.get("env").unwrap();
        for name in [
            "HOMEDIR", "HOME", "PATH", "CC", "CXX", "LANG", "SHELL", "TERM", "USER", "PWD",
        ] {
            env.set(name, "x").unwrap();
        }
        ks.freeze(&env).unwrap();
        let res: mlua::Result<String> =
            lua.load(r#"return cook.require_env("HOMDIR")"#).eval();
        assert!(res.is_err());
        let msg = format!("{}", res.unwrap_err());
        assert!(msg.contains("HOMEDIR"), "expected HOMEDIR in: {msg}");
        assert!(
            msg.contains("env.HOMDIR = "),
            "expected 'env.HOMDIR = ' in: {msg}"
        );
        assert!(
            !msg.contains("PATH"),
            "PATH is far from HOMDIR and should not appear in: {msg}"
        );
        assert!(
            !msg.contains("Declared env vars:"),
            "must not dump the full declared/host env set: {msg}"
        );
    }

    #[test]
    fn edit_distance_is_symmetric_and_case_insensitive() {
        assert_eq!(edit_distance("HOME", "HOME"), 0);
        assert_eq!(edit_distance("HOME", "home"), 0);
        assert_eq!(edit_distance("HOMDIR", "HOMEDIR"), 1);
        assert_eq!(
            edit_distance("HOMDIR", "HOMEDIR"),
            edit_distance("HOMEDIR", "HOMDIR")
        );
        assert_eq!(edit_distance("abc", "xyz"), edit_distance("xyz", "abc"));
    }

    #[test]
    fn closest_declared_picks_top_n_by_distance() {
        let declared: Vec<String> = [
            "HOMEDIR", "HOME", "PATH", "CC", "CXX", "LANG", "SHELL", "TERM", "USER", "PWD",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let closest = closest_declared("HOMDIR", &declared, 3);
        assert_eq!(closest.len(), 3);
        assert!(
            closest.contains(&"HOMEDIR".to_string()),
            "expected HOMEDIR among closest: {closest:?}"
        );
        assert!(
            !closest.contains(&"PATH".to_string()),
            "PATH should not be among closest: {closest:?}"
        );
    }

    #[test]
    fn errors_when_no_declarations_at_all() {
        let (lua, cook, ks) = setup();
        let env: LuaTable = cook.get("env").unwrap();
        ks.freeze(&env).unwrap();
        let res: mlua::Result<String> = lua.load(r#"return cook.require_env("X")"#).eval();
        assert!(res.is_err());
        let msg = format!("{}", res.unwrap_err());
        assert!(
            msg.contains("not declared in any config block"),
            "expected 'not declared in any config block' in: {msg}"
        );
    }

    #[test]
    fn post_freeze_write_does_not_make_key_declared() {
        let (lua, cook, ks) = setup();
        let env: LuaTable = cook.get("env").unwrap();
        // Freeze with empty env (no config-block declarations)
        ks.freeze(&env).unwrap();
        // Simulate a recipe-time write to the live env table after the freeze
        env.set("LATE", "value").unwrap();
        // The post-freeze write makes cook.env["LATE"] visible, but require_env
        // must still error — LATE was not in scope at freeze time, so it does
        // not satisfy the "declared" contract from §xref.resolution step 3.
        let res: mlua::Result<String> = lua.load(r#"return cook.require_env("LATE")"#).eval();
        assert!(res.is_err(), "post-freeze write must not declare key");
        let msg = format!("{}", res.unwrap_err());
        assert!(msg.contains("LATE") && msg.contains("not declared"),
            "diagnostic must name LATE and mention it is not declared; got: {}", msg);
    }
}
