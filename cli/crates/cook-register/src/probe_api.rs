//! Register-phase `cook.probe(key, opts)` binding.
//!
//! Installs the `cook.probe` function on the existing `cook` Lua table and
//! accumulates registrations into a [`ProbeRegistry`]. The registry is later
//! drained into [`SessionCaptureState::probes`](crate::SessionCaptureState)
//! after the register pass completes.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use mlua::prelude::*;

use cook_contracts::{CapturedUnit, DepKind, ProbeInputs, ProbeUnit, WorkPayload};

use crate::SharedBodySlot;

/// Per-key registration record: the resolved [`ProbeUnit`] plus the source
/// location where `cook.probe` was called (for duplicate-key diagnostics).
#[derive(Debug)]
pub struct ProbeRegistration {
    pub probe: ProbeUnit,
    pub source_file: String,
    pub source_line: usize,
}

/// Accumulates all `cook.probe(...)` calls made during a single register pass.
#[derive(Debug, Default)]
pub struct ProbeRegistry {
    pub probes: BTreeMap<String, ProbeRegistration>,
}

/// A ref-counted, interior-mutable handle to a [`ProbeRegistry`].
/// `!Send` by design — the register VM is single-threaded.
pub type SharedProbeRegistry = Rc<RefCell<ProbeRegistry>>;

/// Install `cook.probe(key, opts)` on `cook`.
///
/// * `lua`         — the register-phase Lua VM.
/// * `cook`        — the `cook` table already present in `lua.globals()`.
/// * `registry`    — receives each successful `cook.probe` call.
/// * `body_slot`   — the active body capture state. When a body is active
///                   (body-scoped `cook.probe`), each probe is also pushed
///                   as a `CapturedUnit { payload: WorkPayload::Probe { … } }`
///                   onto the body's units vector so the DAG builder schedules
///                   probe work as a consumer of the recipe (CS-0074 Bug 1).
///                   When the body slot is `None` (top-level `cook.probe` per
///                   spec §6 step 4 / §7), the probe is session-scoped: it
///                   lives only in the probe registry and no `CapturedUnit`
///                   is emitted.
/// * `source_file` — the Cookfile name included in duplicate-key diagnostics.
pub fn install_cook_probe(
    lua: &Lua,
    cook: &LuaTable,
    registry: SharedProbeRegistry,
    body_slot: SharedBodySlot,
    source_file: String,
) -> LuaResult<()> {
    let probe_fn = lua.create_function(move |lua, (key, opts): (String, LuaTable)| {
        // 1. Validate key is non-empty (the table typing is already checked by
        //    mlua's argument destructuring — a non-string key raises before here).
        if key.is_empty() {
            return Err(LuaError::runtime("cook.probe: key must be a non-empty string"));
        }

        // 4. Detect call-site line via debug.getinfo(2, "Sl").
        let call_line: usize = lua
            .load("return debug.getinfo(4, 'Sl').currentline")
            .eval::<i64>()
            .unwrap_or(0)
            .max(0) as usize;

        // 2. Read opts.inputs sub-keys (all optional, default empty).
        let inputs = match opts.get::<LuaValue>("inputs") {
            Ok(LuaValue::Table(inp)) => {
                let env = read_string_list(&inp, "env")?;
                let tools = read_string_list(&inp, "tools")?;
                let files = read_string_list(&inp, "files")?;
                let requires = read_string_list(&inp, "requires")?;
                ProbeInputs { env, tools, files, requires }
            }
            Ok(LuaValue::Nil) | Err(_) => ProbeInputs::default(),
            Ok(other) => {
                return Err(LuaError::runtime(format!(
                    "cook.probe: opts.inputs must be a table, got {}",
                    lua_type_name(&other)
                )));
            }
        };

        // 3. Read opts.produce — MUST be a string.
        let produce_source: String = match opts.get::<LuaValue>("produce") {
            Ok(LuaValue::String(s)) => s.to_str()?.to_string(),
            Ok(LuaValue::Function(_)) => {
                return Err(LuaError::runtime(
                    "cook.probe: opts.produce must be a string (Lua source code), got function",
                ));
            }
            Ok(other) => {
                return Err(LuaError::runtime(format!(
                    "cook.probe: opts.produce must be a string (Lua source code), got {}",
                    lua_type_name(&other)
                )));
            }
            Err(_) => {
                return Err(LuaError::runtime(
                    "cook.probe: opts.produce is required and must be a string",
                ));
            }
        };

        // 5. Duplicate-key check.
        let mut reg = registry.borrow_mut();
        if let Some(prev) = reg.probes.get(&key) {
            return Err(LuaError::runtime(format!(
                "probe key '{}' declared at {}:{}; previously declared at {}:{}",
                key,
                source_file,
                call_line,
                prev.source_file,
                prev.source_line,
            )));
        }

        // 6. Insert into registry.
        reg.probes.insert(
            key.clone(),
            ProbeRegistration {
                probe: ProbeUnit {
                    key: key.clone(),
                    produce_source: produce_source.clone(),
                    produce_line: call_line,
                    inputs: inputs.clone(),
                },
                source_file: source_file.clone(),
                source_line: call_line,
            },
        );

        // 7. Body-scoped probes (slot.is_some()) ALSO get a CapturedUnit so
        //    the DAG builder can schedule the probe as a consumer work-unit
        //    within the owning recipe and wire consumer→probe edges
        //    (CS-0074 §22.5.2). Probe-to-probe edges come from
        //    inputs.requires; probe-to-consumer edges are wired in the DAG
        //    builder by reading each consumer unit's `probes` field.
        //
        //    Top-level probes (slot.is_none()) are session-scoped per spec
        //    §6 step 4 / §7: they live only in `probe_registry` (drained
        //    later into `session_state.probes`) and do NOT need to appear
        //    in any recipe body's units vector. We deliberately skip the
        //    body push instead of erroring — `register_cookfile` starts
        //    with `body_slot == None` during top-level Lua load.
        let mut slot = body_slot.borrow_mut();
        if let Some(body) = slot.as_mut() {
            body.units.push(CapturedUnit {
                payload: WorkPayload::Probe {
                    key,
                    produce: produce_source,
                    line: call_line,
                },
                cache_meta: None,
                dep_kind: DepKind::Sequential,
                probes: inputs.requires,
            });
        }

        Ok(())
    })?;

    cook.set("probe", probe_fn)?;
    Ok(())
}

/// Three-color DFS state for cycle detection.
#[derive(Clone, Copy, PartialEq, Eq)]
enum NodeState {
    InProgress,
    Done,
}

impl ProbeRegistry {
    /// Detect cycles in the probe `requires` graph (§22.5.8).
    ///
    /// Returns `Ok(())` when the graph is acyclic, or `Err(msg)` with a
    /// diagnostic of the form `"probe cycle detected: cc:a -> cc:b -> cc:a"`.
    pub fn detect_cycles(&self) -> Result<(), String> {
        let mut state: BTreeMap<&str, NodeState> = BTreeMap::new();
        let mut stack: Vec<&str> = vec![];
        for k in self.probes.keys() {
            if !matches!(state.get(k.as_str()), Some(NodeState::Done)) {
                self.dfs(k, &mut state, &mut stack)?;
            }
        }
        Ok(())
    }

    fn dfs<'a>(
        &'a self,
        node: &'a str,
        state: &mut BTreeMap<&'a str, NodeState>,
        stack: &mut Vec<&'a str>,
    ) -> Result<(), String> {
        state.insert(node, NodeState::InProgress);
        stack.push(node);
        if let Some(reg) = self.probes.get(node) {
            for r in &reg.probe.inputs.requires {
                match state.get(r.as_str()) {
                    Some(NodeState::InProgress) => {
                        // Cycle detected — trim stack to where `r` first appears.
                        let start = stack.iter().position(|&n| n == r.as_str()).unwrap_or(0);
                        let mut path: Vec<&str> = stack[start..].to_vec();
                        path.push(r.as_str());
                        return Err(format!(
                            "probe cycle detected: {}",
                            path.join(" -> ")
                        ));
                    }
                    Some(NodeState::Done) => continue,
                    None => self.dfs(r, state, stack)?,
                }
            }
        }
        stack.pop();
        state.insert(node, NodeState::Done);
        Ok(())
    }
}

/// Read a named key from a Lua table as a `Vec<String>`.  Returns an empty
/// `Vec` when the key is absent or `nil`.
fn read_string_list(tbl: &LuaTable, key: &str) -> LuaResult<Vec<String>> {
    match tbl.get::<LuaValue>(key) {
        Ok(LuaValue::Nil) | Err(_) => Ok(vec![]),
        Ok(LuaValue::Table(t)) => {
            let mut out = Vec::new();
            for v in t.sequence_values::<String>() {
                out.push(v.map_err(|e| {
                    LuaError::runtime(format!(
                        "cook.probe: opts.inputs.{key} must be a list of strings: {e}"
                    ))
                })?);
            }
            Ok(out)
        }
        Ok(other) => Err(LuaError::runtime(format!(
            "cook.probe: opts.inputs.{key} must be a list of strings, got {}",
            lua_type_name(&other)
        ))),
    }
}

fn lua_type_name(v: &LuaValue) -> &'static str {
    match v {
        LuaValue::Nil => "nil",
        LuaValue::Boolean(_) => "boolean",
        LuaValue::Integer(_) => "integer",
        LuaValue::Number(_) => "number",
        LuaValue::String(_) => "string",
        LuaValue::Table(_) => "table",
        LuaValue::Function(_) => "function",
        LuaValue::Thread(_) => "thread",
        LuaValue::UserData(_) => "userdata",
        LuaValue::LightUserData(_) => "lightuserdata",
        _ => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BodyCaptureState;

    fn setup(source_file: &str) -> (Lua, SharedProbeRegistry, SharedBodySlot) {
        let lua = Lua::new();
        let cook = lua.create_table().unwrap();
        lua.globals().set("cook", cook.clone()).unwrap();
        let reg: SharedProbeRegistry = Rc::new(RefCell::new(ProbeRegistry::default()));
        let body_slot: SharedBodySlot =
            Rc::new(RefCell::new(Some(BodyCaptureState::new())));
        install_cook_probe(&lua, &cook, reg.clone(), body_slot.clone(), source_file.to_string()).unwrap();
        (lua, reg, body_slot)
    }

    #[test]
    fn cook_probe_registers_a_unit() {
        let (lua, reg, _cap) = setup("Cookfile");

        lua.load(r#"
            cook.probe("cc:zlib", {
              inputs = { env = {"PKG_CONFIG_PATH"}, tools = {"pkg-config"} },
              produce = "return { found = true }",
            })
        "#)
        .exec()
        .unwrap();

        let r = reg.borrow();
        let p = r.probes.get("cc:zlib").expect("probe registered");
        assert_eq!(p.probe.key, "cc:zlib");
        assert_eq!(p.probe.produce_source, "return { found = true }");
        assert_eq!(p.probe.inputs.env, vec!["PKG_CONFIG_PATH"]);
        assert_eq!(p.probe.inputs.tools, vec!["pkg-config"]);
    }

    #[test]
    fn cook_probe_registers_requires_in_inputs() {
        let (lua, reg, _cap) = setup("Cookfile");

        lua.load(r#"
            cook.probe("cc:libfoo", {
              inputs = { requires = {"cc:compiler"} },
              produce = "return true",
            })
        "#)
        .exec()
        .unwrap();

        let r = reg.borrow();
        let p = r.probes.get("cc:libfoo").expect("probe registered");
        assert_eq!(p.probe.inputs.requires, vec!["cc:compiler"]);
    }

    #[test]
    fn cook_probe_empty_inputs_table_is_ok() {
        let (lua, reg, _cap) = setup("Cookfile");

        lua.load(r#"
            cook.probe("cc:simple", {
              inputs = {},
              produce = "return 1",
            })
        "#)
        .exec()
        .unwrap();

        let r = reg.borrow();
        assert!(r.probes.contains_key("cc:simple"));
    }

    #[test]
    fn cook_probe_omitting_inputs_defaults_to_empty() {
        let (lua, reg, _cap) = setup("Cookfile");

        lua.load(r#"
            cook.probe("cc:noinputs", {
              produce = "return nil",
            })
        "#)
        .exec()
        .unwrap();

        let r = reg.borrow();
        let p = r.probes.get("cc:noinputs").expect("probe registered");
        assert!(p.probe.inputs.env.is_empty());
        assert!(p.probe.inputs.tools.is_empty());
        assert!(p.probe.inputs.files.is_empty());
        assert!(p.probe.inputs.requires.is_empty());
    }

    #[test]
    fn duplicate_probe_key_errors_with_both_locations() {
        let (lua, _reg, _cap) = setup("Cookfile");

        let result = lua
            .load(r#"
            cook.probe("cc:zlib", { inputs = {}, produce = "return 1" })
            cook.probe("cc:zlib", { inputs = {}, produce = "return 2" })
        "#)
            .exec();

        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("probe key 'cc:zlib' declared at"),
            "got: {err}"
        );
        assert!(err.contains("previously declared at"), "got: {err}");
    }

    #[test]
    fn produce_must_be_string_not_function() {
        let (lua, _reg, _cap) = setup("Cookfile");

        let result = lua
            .load(r#"
            cook.probe("cc:zlib", {
              inputs = {},
              produce = function() return 1 end,
            })
        "#)
            .exec();

        let err = result.unwrap_err().to_string();
        assert!(err.contains("must be a string"), "got: {err}");
    }

    #[test]
    fn produce_missing_raises_error() {
        let (lua, _reg, _cap) = setup("Cookfile");

        let result = lua
            .load(r#"cook.probe("k", { inputs = {} })"#)
            .exec();

        assert!(result.is_err(), "missing produce must raise an error");
        let err = result.unwrap_err().to_string();
        assert!(err.contains("produce"), "error should mention 'produce'; got: {err}");
    }

    #[test]
    fn empty_key_raises_error() {
        let (lua, _reg, _cap) = setup("Cookfile");

        let result = lua
            .load(r#"cook.probe("", { inputs = {}, produce = "return 1" })"#)
            .exec();

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("non-empty"), "got: {err}");
    }

    #[test]
    fn multiple_distinct_probes_all_registered() {
        let (lua, reg, _cap) = setup("Cookfile");

        lua.load(r#"
            cook.probe("cc:zlib",  { inputs = {}, produce = "return 1" })
            cook.probe("cc:openssl", { inputs = {}, produce = "return 2" })
            cook.probe("cc:lua", { inputs = {}, produce = "return 3" })
        "#)
        .exec()
        .unwrap();

        let r = reg.borrow();
        assert_eq!(r.probes.len(), 3);
        assert!(r.probes.contains_key("cc:zlib"));
        assert!(r.probes.contains_key("cc:openssl"));
        assert!(r.probes.contains_key("cc:lua"));
    }
}
