use mlua::prelude::*;

use cook_contracts::{CapturedUnit, DepKind, WorkPayload};

use crate::SharedBodySlot;

/// Uniform register-phase type error for a `cook.add_test` field (CS-0127):
/// a wrong-typed field is a hard error naming the field, the expected type,
/// and the received Lua type — never silently coerced to its default. Mirrors
/// `unit_api::type_err`.
fn type_err(field: &str, expected: &str, got: &str) -> LuaError {
    LuaError::runtime(format!(
        "cook.add_test: `{field}` must be {expected}, got {got} (Standard \u{00a7}22.4, CS-0127)"
    ))
}

/// Register `cook.add_test(table)` on the cook table.
///
/// cook.add_test captures a test work unit (Standard §22.4: `command`/
/// `lua_code`, `suite`, `line`, `iteration_item`, and `inputs`). Uses
/// DepKind::TestSibling so test failures don't cancel siblings.
pub fn register_test_api(lua: &Lua, body_slot: SharedBodySlot) -> LuaResult<()> {
    let cook: LuaTable = lua.globals().get("cook")?;

    let body_slot_add = body_slot.clone();
    let add_test_fn = lua.create_function(move |_, tbl: LuaTable| {
        // CS-0127 §22.4: `command`, if present, must be a string — never
        // coerced. An empty string is treated as absent (`None`) so the
        // exactly-one check below reports it as missing, not as a supplied
        // value, matching the historical empty-command diagnostic.
        let command: Option<String> = match tbl.get::<LuaValue>("command") {
            Ok(LuaValue::Nil) | Err(_) => None,
            Ok(LuaValue::String(s)) => Some(s.to_string_lossy().to_string()),
            Ok(other) => return Err(type_err("command", "a string", other.type_name())),
        }
        .filter(|s| !s.is_empty());
        // CS-0127 §22.4: `lua_code`, if present, must be a string — never
        // coerced. Empty string treated as absent, as for `command`.
        let lua_code: Option<String> = match tbl.get::<LuaValue>("lua_code") {
            Ok(LuaValue::Nil) | Err(_) => None,
            Ok(LuaValue::String(s)) => Some(s.to_string_lossy().to_string()),
            Ok(other) => return Err(type_err("lua_code", "a string", other.type_name())),
        }
        .filter(|s| !s.is_empty());
        // CS-0127 §22.4: exactly one of `command` / `lua_code` MUST be
        // provided non-empty. Both empty/absent → the "required" arm (message
        // names `command`, the historical field); both present → "got both".
        let (command, lua_code) = match (command, lua_code) {
            (Some(c), None) => (c, None),
            (None, Some(l)) => (String::new(), Some(l)),
            (Some(_), Some(_)) => {
                return Err(mlua::Error::runtime(
                    "cook.add_test: exactly one of `command` or `lua_code` must be provided, got both (Standard \u{00a7}22.4, CS-0127)",
                ))
            }
            _ => {
                return Err(mlua::Error::runtime(
                    "cook.add_test: exactly one of `command` or `lua_code` is required and must be a non-empty string (Standard \u{00a7}22.4, CS-0127)",
                ))
            }
        };

        // CS-0135 §22.4 / §7: `cook.add_test` no longer accepts a `timeout`
        // field (the `test` step's `timeout` modifier was removed), and there
        // is no per-test time bound in v1.0 — a hung test hangs the run, the
        // same as `make` (App. E CS-0135). We therefore pass an effectively
        // unbounded timeout so the executor's kill loop never fires. The
        // `WorkPayload::Test::timeout` field stays populated for the engine
        // (and for the planned 1.x per-test-timeout re-add).
        let timeout: u64 = u64::MAX;

        // CS-0127 §22.4: `suite` defaults to the enclosing recipe's name.
        let suite_name: String = match tbl.get::<LuaValue>("suite") {
            Ok(LuaValue::Nil) | Err(_) => {
                let slot = body_slot_add.borrow();
                let body = slot.as_ref().ok_or_else(|| {
                    mlua::Error::runtime("cook.add_test called outside a recipe body")
                })?;
                body.current_recipe.clone().unwrap_or_default()
            }
            Ok(LuaValue::String(s)) => {
                let sv = s.to_string_lossy().to_string();
                if sv.is_empty() {
                    let slot = body_slot_add.borrow();
                    let body = slot.as_ref().ok_or_else(|| {
                        mlua::Error::runtime("cook.add_test called outside a recipe body")
                    })?;
                    body.current_recipe.clone().unwrap_or_default()
                } else {
                    sv
                }
            }
            Ok(other) => return Err(type_err("suite", "a string", other.type_name())),
        };

        // CS-0135 §22.4: `cook.add_test` no longer accepts a
        // `should_fail` field (the `test` step's `should_fail` modifier
        // was removed). `WorkPayload::Test::should_fail` stays
        // populated for the engine executor's pass/fail inversion,
        // defaulting to the same value the field used to fall back to
        // when absent.
        let should_fail: bool = false;
        // CS-0127 §22.4: `line` must be a non-negative integer — never
        // coerced.
        let line: usize = match tbl.get::<LuaValue>("line") {
            Ok(LuaValue::Nil) | Err(_) => 0,
            Ok(LuaValue::Integer(n)) if n >= 0 => n as usize,
            Ok(other) => return Err(type_err("line", "a non-negative integer", other.type_name())),
        };
        // CS-0127 §22.4: `iteration_item` must be a string — never coerced.
        let iteration_item: Option<String> = match tbl.get::<LuaValue>("iteration_item") {
            Ok(LuaValue::Nil) | Err(_) => None,
            Ok(LuaValue::String(s)) => {
                let sv = s.to_string_lossy().to_string();
                if sv.is_empty() { None } else { Some(sv) }
            }
            Ok(other) => return Err(type_err("iteration_item", "a string", other.type_name())),
        };

        // COOK-84: declared ingredient files for this test (codegen passes the
        // recipe's resolved `ingredients` local). Unioned below with the step
        // group's dep-output paths, mirroring cook.add_unit's cache_input_paths
        // (unit_api.rs). CS-0127: `inputs` must be a table of strings —
        // never coerced (including mlua's implicit number-to-string
        // coercion on elements).
        let inputs: Vec<String> = match tbl.get::<LuaValue>("inputs") {
            Ok(LuaValue::Nil) | Err(_) => vec![],
            Ok(LuaValue::Table(t)) => {
                let mut out = Vec::new();
                for v in t.sequence_values::<LuaValue>() {
                    let v = v.map_err(|e| {
                        LuaError::runtime(format!("cook.add_test: `inputs`: {e}"))
                    })?;
                    match v {
                        LuaValue::String(s) => out.push(s.to_string_lossy().to_string()),
                        other => {
                            return Err(type_err("inputs", "a table of strings", other.type_name()))
                        }
                    }
                }
                out
            }
            Ok(other) => return Err(type_err("inputs", "a table of strings", other.type_name())),
        };

        // CS-0159: opts.seal — the test unit's effective seal set (bare probe
        // keys). Mirrors `cook.add_unit`'s `seal` field exactly: only the KEY
        // set is register-time data; the canonical VALUES fold into the test
        // fingerprint at execute phase, once the sealed probes have run.
        let seal_keys: std::collections::BTreeSet<String> = match tbl.get::<LuaValue>("seal") {
            Ok(LuaValue::Nil) | Err(_) => Default::default(),
            Ok(LuaValue::Table(t)) => {
                let mut out = std::collections::BTreeSet::new();
                for v in t.sequence_values::<LuaValue>() {
                    let v = v.map_err(|e| {
                        LuaError::runtime(format!("cook.add_test: `seal`: {e}"))
                    })?;
                    match v {
                        LuaValue::String(s) => {
                            out.insert(s.to_string_lossy().to_string());
                        }
                        other => {
                            return Err(type_err("seal", "a table of strings", other.type_name()))
                        }
                    }
                }
                out
            }
            Ok(other) => return Err(type_err("seal", "a table of strings", other.type_name())),
        };

        // opts.consumes — glob allowlist narrowing which PREDECESSOR OUTPUTS
        // fold into the ready-time fingerprint (§17.4 step 1). Absent folds
        // all of them, the historical behaviour; a test that states what it
        // reads stops re-keying on dependency artifacts it never opens
        // (sourcemaps being the flagship case). Same table-of-strings
        // discipline as `inputs` / `seal`, never coerced.
        let consumes: Vec<String> = match tbl.get::<LuaValue>("consumes") {
            Ok(LuaValue::Nil) | Err(_) => vec![],
            Ok(LuaValue::Table(t)) => {
                let mut out = Vec::new();
                for v in t.sequence_values::<LuaValue>() {
                    let v = v.map_err(|e| {
                        LuaError::runtime(format!("cook.add_test: `consumes`: {e}"))
                    })?;
                    match v {
                        LuaValue::String(s) => {
                            let sv = s.to_string_lossy().to_string();
                            // Validated with the matcher the engine folds
                            // with, so a pattern accepted here is the
                            // pattern that runs. Rejected at register time
                            // because an unparseable glob matches nothing,
                            // and "matches nothing" on an allowlist points
                            // the under-keying way.
                            if let Err(e) = cook_fingerprint::consumes::validate_pattern(&sv) {
                                return Err(LuaError::runtime(format!(
                                    "cook.add_test: `consumes` entry '{sv}' is not a \
                                     valid glob ({e}); it would match no predecessor \
                                     output, silently widening the cached-pass window"
                                )));
                            }
                            out.push(sv);
                        }
                        other => {
                            return Err(type_err(
                                "consumes",
                                "a table of strings",
                                other.type_name(),
                            ))
                        }
                    }
                }
                out
            }
            Ok(other) => return Err(type_err("consumes", "a table of strings", other.type_name())),
        };

        let mut slot = body_slot_add.borrow_mut();
        let body = slot.as_mut().ok_or_else(|| {
            mlua::Error::runtime("cook.add_test called outside a recipe body")
        })?;
        // CS-0135 §22.4: `cook.add_test` accepts no `name` field (the `test`
        // step's `as` modifier substitutes at codegen time), so
        // `WorkPayload::Test::test_name` is engine-facing only. Derive it as
        // `<recipe>_test<N>` — N the 1-based ordinal of this test unit within
        // the enclosing recipe body — so a target-less test unit carries a
        // stable, non-empty label (progress verb lines previously fell back
        // to `$?`, and `cook logs` showed a blank unit name).
        let test_ordinal = body
            .units
            .iter()
            .filter(|u| matches!(u.payload, WorkPayload::Test { .. }))
            .count()
            + 1;
        let test_name = format!(
            "{}_test{}",
            body.current_recipe.as_deref().unwrap_or(""),
            test_ordinal
        );
        // COOK-84: inputs ∪ step_group_dep_input_paths, deduped, order-preserving.
        let mut input_paths: Vec<String> = inputs;
        for p in &body.step_group_dep_input_paths {
            if !input_paths.contains(p) {
                input_paths.push(p.clone());
            }
        }
        let payload = WorkPayload::Test {
            cmd: command,
            line,
            timeout,
            should_fail,
            suite_name,
            test_name,
            iteration_item,
            lua_code,
            input_paths,
            seal_keys: seal_keys.clone(),
            consumes,
        };
        let dep_kind = if let Some(group_idx) = body.current_group {
            DepKind::TestSibling(group_idx)
        } else {
            DepKind::Sequential
        };
        let unit_idx = body.units.len();
        body.units.push(CapturedUnit {
            payload,
            cache_meta: None,
            dep_kind: dep_kind.clone(),
            // CS-0159: sealed probes are execute-phase determinants — the test
            // must run after them so their values are materialised before the
            // ready-time fingerprint is computed. Same wiring `cook.add_unit`
            // does for a sealing cook unit (unit_api.rs); without it the fold
            // would silently read an empty value for every sealed key.
            probes: seal_keys.into_iter().collect(),
            unit_env_vars: Default::default(),
            member: None,
            output_paths: Vec::new(),
        });
        if let DepKind::TestSibling(gi) = &dep_kind {
            body.step_groups[*gi].push(unit_idx);
        }
        // Mirrors cook.add_unit: every dep ref accumulated in this step_group
        // (e.g. via cook.dep_output("alias.recipe") calls inside the test
        // body) must be wired as a dep edge for this unit, so the wave
        // grouper schedules the upstream recipe before this test runs.
        // Without this, a test body refing a sibling recipe races that
        // sibling under --jobs > 1.
        let dep_refs: Vec<String> = body.step_group_dep_refs.clone();
        for dep_name in dep_refs {
            body.dep_edges.push((unit_idx, dep_name));
        }

        Ok(())
    })?;
    cook.set("add_test", add_test_fn)?;

    Ok(())
}

#[cfg(test)]
#[path = "tests/test_api_tests.rs"]
mod tests;
