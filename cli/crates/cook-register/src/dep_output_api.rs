use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use mlua::prelude::*;

use crate::SharedCaptureState;

/// Shared storage for terminal outputs of registered recipes, keyed by
/// **fully-qualified** recipe name (e.g., `"lib.lib_build"` or just
/// `"build"` for root-Cookfile recipes). Hoisted to workspace scope so all
/// Registries write to and read from the same map.
pub type SharedTerminalOutputs = Arc<Mutex<BTreeMap<String, Vec<String>>>>;

/// Register `cook.dep_output(name)` and `cook.dep_output_list(name)` on the cook table.
///
/// - `cook.dep_output(name)` returns the terminal outputs of recipe `name` as a space-joined string.
/// - `cook.dep_output_list(name)` returns the terminal outputs of recipe `name` as a Lua table.
///
/// Both functions record a dep edge in `capture_state.dep_edges` for fine-grained DAG wiring.
///
/// `alias_dirs` maps alias names (e.g. `"lib"`) to the importer-relative path of the alias's
/// directory (e.g. `PathBuf::from("lib")`). When a qualified name like `"lib.lib_build"` is
/// looked up, each output path from the importee is rewritten by prepending the alias's dir.
///
/// `qualified_prefix` is the workspace-global prefix of the Cookfile that owns this registry
/// (e.g. `"queue"` when this registry is for `libs/queue` loaded under alias `queue`, or `""`
/// for the entry-point Cookfile). Lua bodies use **local** alias names
/// (`"proto.proto_lib"` in `libs/queue`), but recipes are stored in `terminal_outputs` under
/// **globally-qualified** names (`"queue.proto.proto_lib"` when the entry is
/// `apps/server` → `//libs/queue` → `//libs/proto`). At lookup, the local name is prefixed
/// with `qualified_prefix` to produce the global key. The dep ref recorded into
/// `step_group_dep_refs` is the global key, so DAG edges line up with `recipe_leaves`.
/// Path rewriting via `alias_dirs` keeps using the **local** name — the alias is the local
/// name's first dot-component.
pub fn register_dep_output_api(
    lua: &Lua,
    terminal_outputs: SharedTerminalOutputs,
    capture_state: SharedCaptureState,
    alias_dirs: BTreeMap<String, PathBuf>,
    qualified_prefix: String,
) -> LuaResult<()> {
    let cook: LuaTable = lua.globals().get("cook")?;

    let alias_dirs = Arc::new(alias_dirs);
    let qualified_prefix = Arc::new(qualified_prefix);

    // cook.dep_output(name) → space-joined string
    // Accumulates dep ref in step_group_dep_refs; actual edge recording
    // happens in cook.add_unit() which attaches the ref to the correct unit.
    let to = terminal_outputs.clone();
    let cs = capture_state.clone();
    let ad = alias_dirs.clone();
    let qp = qualified_prefix.clone();
    let dep_output_fn = lua.create_function(move |_, name: String| {
        let global_key = qualify(&qp, &name);
        let store = to.lock().expect("terminal_outputs mutex poisoned");
        let outputs = store.get(&global_key).ok_or_else(|| {
            mlua::Error::RuntimeError(format!(
                "recipe '{}' has no terminal output (not registered or has no cook steps)",
                name
            ))
        })?;
        let rewritten = rewrite_paths_for_importer(&name, outputs, &ad);
        {
            let mut state = cs.borrow_mut();
            if !state.step_group_dep_refs.contains(&global_key) {
                state.step_group_dep_refs.push(global_key.clone());
            }
            for p in &rewritten {
                if !state.step_group_dep_input_paths.contains(p) {
                    state.step_group_dep_input_paths.push(p.clone());
                }
            }
        }
        Ok(rewritten.join(" "))
    })?;
    cook.set("dep_output", dep_output_fn)?;

    // cook.dep_output_list(name) → Lua table
    // Same accumulation pattern as dep_output.
    let to2 = terminal_outputs.clone();
    let cs2 = capture_state.clone();
    let ad2 = alias_dirs.clone();
    let qp2 = qualified_prefix.clone();
    let dep_output_list_fn = lua.create_function(move |lua, name: String| {
        let global_key = qualify(&qp2, &name);
        let store = to2.lock().expect("terminal_outputs mutex poisoned");
        let outputs = store.get(&global_key).ok_or_else(|| {
            mlua::Error::RuntimeError(format!(
                "recipe '{}' has no terminal output (not registered or has no cook steps)",
                name
            ))
        })?;
        let rewritten = rewrite_paths_for_importer(&name, outputs, &ad2);
        {
            let mut state = cs2.borrow_mut();
            if !state.step_group_dep_refs.contains(&global_key) {
                state.step_group_dep_refs.push(global_key.clone());
            }
            for p in &rewritten {
                if !state.step_group_dep_input_paths.contains(p) {
                    state.step_group_dep_input_paths.push(p.clone());
                }
            }
        }
        let table = lua.create_table()?;
        for (i, path) in rewritten.iter().enumerate() {
            table.set(i + 1, path.as_str())?;
        }
        Ok(table)
    })?;
    cook.set("dep_output_list", dep_output_list_fn)?;

    Ok(())
}

fn qualify(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        name.to_string()
    } else {
        format!("{}.{}", prefix, name)
    }
}

/// If `name` has the form `alias.recipe`, rewrite each importee path by
/// joining with `alias_dirs[alias]` (the importer-relative path to the alias's
/// directory). Same-Cookfile names (no `alias.` prefix) pass through unchanged.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CaptureState;
    use std::cell::RefCell;
    use std::rc::Rc;

    fn setup_lua() -> (Lua, SharedTerminalOutputs, SharedCaptureState) {
        let lua = Lua::new();
        lua.globals().set("cook", lua.create_table().unwrap()).unwrap();
        let terminal_outputs: SharedTerminalOutputs = Arc::new(Mutex::new(BTreeMap::new()));
        let capture_state: SharedCaptureState = Rc::new(RefCell::new(CaptureState::new()));
        (lua, terminal_outputs, capture_state)
    }

    #[test]
    fn test_dep_output_returns_space_joined() {
        let (lua, outputs, cs) = setup_lua();
        outputs.lock().unwrap().insert(
            "protos".into(),
            vec!["gen/foo.pb.o".into(), "gen/bar.pb.o".into()],
        );
        register_dep_output_api(&lua, outputs, cs, BTreeMap::new(), String::new()).unwrap();
        let result: String = lua
            .load(r#"return cook.dep_output("protos")"#)
            .eval()
            .unwrap();
        assert_eq!(result, "gen/foo.pb.o gen/bar.pb.o");
    }

    #[test]
    fn test_dep_output_list_returns_table() {
        let (lua, outputs, cs) = setup_lua();
        outputs
            .lock().unwrap()
            .insert("libmath".into(), vec!["build/lib/libmath.a".into()]);
        register_dep_output_api(&lua, outputs, cs, BTreeMap::new(), String::new()).unwrap();
        let result: Vec<String> = lua
            .load(r#"return cook.dep_output_list("libmath")"#)
            .eval()
            .unwrap();
        assert_eq!(result, vec!["build/lib/libmath.a"]);
    }

    #[test]
    fn test_dep_output_unknown_recipe_errors() {
        let (lua, outputs, cs) = setup_lua();
        register_dep_output_api(&lua, outputs, cs, BTreeMap::new(), String::new()).unwrap();
        let result = lua
            .load(r#"return cook.dep_output("nonexistent")"#)
            .eval::<String>();
        assert!(result.is_err());
    }

    #[test]
    fn test_dep_output_accumulates_dep_ref() {
        let (lua, outputs, cs) = setup_lua();
        outputs
            .lock().unwrap()
            .insert("libmath".into(), vec!["libmath.a".into()]);
        register_dep_output_api(&lua, outputs, cs.clone(), BTreeMap::new(), String::new()).unwrap();
        lua.load(r#"cook.dep_output("libmath")"#).exec().unwrap();
        // dep_output accumulates in step_group_dep_refs, not dep_edges directly.
        // Actual edge recording happens in cook.add_unit().
        let state = cs.borrow();
        assert_eq!(state.step_group_dep_refs, vec!["libmath".to_string()]);
        // No direct dep_edges yet — add_unit would create them.
        assert!(state.dep_edges.is_empty());
    }

    #[test]
    fn test_dep_output_deduplicates_refs() {
        let (lua, outputs, cs) = setup_lua();
        outputs
            .lock().unwrap()
            .insert("libmath".into(), vec!["libmath.a".into()]);
        register_dep_output_api(&lua, outputs, cs.clone(), BTreeMap::new(), String::new()).unwrap();
        lua.load(r#"
            cook.dep_output("libmath")
            cook.dep_output("libmath")
        "#).exec().unwrap();
        let state = cs.borrow();
        // Should not duplicate
        assert_eq!(state.step_group_dep_refs, vec!["libmath".to_string()]);
    }

    #[test]
    fn test_dep_output_rewrites_qualified_paths_with_alias_dir() {
        let (lua, outputs, cs) = setup_lua();
        outputs.lock().unwrap().insert(
            "lib.lib_build".into(),
            vec!["build/lib.o".into()],
        );
        let mut alias_dirs = BTreeMap::new();
        alias_dirs.insert("lib".to_string(), PathBuf::from("lib"));

        register_dep_output_api(&lua, outputs, cs, alias_dirs, String::new()).unwrap();
        let result: String = lua
            .load(r#"return cook.dep_output("lib.lib_build")"#)
            .eval()
            .unwrap();
        assert_eq!(result, "lib/build/lib.o");
    }

    #[test]
    fn test_dep_output_unqualified_no_rewrite() {
        let (lua, outputs, cs) = setup_lua();
        outputs.lock().unwrap().insert(
            "local_recipe".into(),
            vec!["build/local.o".into()],
        );
        register_dep_output_api(&lua, outputs, cs, BTreeMap::new(), String::new()).unwrap();
        let result: String = lua
            .load(r#"return cook.dep_output("local_recipe")"#)
            .eval()
            .unwrap();
        assert_eq!(result, "build/local.o");
    }

    #[test]
    fn test_dep_output_sigil_alias_with_dotdot() {
        let (lua, outputs, cs) = setup_lua();
        outputs.lock().unwrap().insert(
            "core.core_lib".into(),
            vec!["build/core.o".into()],
        );
        let mut alias_dirs = BTreeMap::new();
        alias_dirs.insert("core".to_string(), PathBuf::from("../../core/lib"));

        register_dep_output_api(&lua, outputs, cs, alias_dirs, String::new()).unwrap();
        let result: String = lua
            .load(r#"return cook.dep_output("core.core_lib")"#)
            .eval()
            .unwrap();
        assert_eq!(result, "../../core/lib/build/core.o");
    }

    #[test]
    fn test_dep_output_list_rewrites_qualified_paths() {
        let (lua, outputs, cs) = setup_lua();
        outputs.lock().unwrap().insert(
            "lib.lib_build".into(),
            vec!["build/foo.o".into(), "build/bar.o".into()],
        );
        let mut alias_dirs = BTreeMap::new();
        alias_dirs.insert("lib".to_string(), PathBuf::from("lib"));

        register_dep_output_api(&lua, outputs, cs, alias_dirs, String::new()).unwrap();
        let result: Vec<String> = lua
            .load(r#"return cook.dep_output_list("lib.lib_build")"#)
            .eval()
            .unwrap();
        assert_eq!(result, vec!["lib/build/foo.o", "lib/build/bar.o"]);
    }

    /// Transitive sigil case: when `apps/server` invokes a recipe whose chain is
    /// `server → //libs/queue → //libs/proto`, queue's registry has
    /// `qualified_prefix = "queue"`. Queue's Lua calls `cook.dep_output("proto.proto_lib")`
    /// using its **local** alias, but proto is stored in `terminal_outputs` under the
    /// **global** key `"queue.proto.proto_lib"`. The lookup must translate.
    #[test]
    fn test_dep_output_translates_local_to_global_via_qualified_prefix() {
        let (lua, outputs, cs) = setup_lua();
        outputs.lock().unwrap().insert(
            "queue.proto.proto_lib".into(),
            vec!["build/proto.bin".into()],
        );
        let mut alias_dirs = BTreeMap::new();
        alias_dirs.insert("proto".to_string(), PathBuf::from("../proto"));

        register_dep_output_api(&lua, outputs, cs.clone(), alias_dirs, "queue".to_string())
            .unwrap();
        let result: String = lua
            .load(r#"return cook.dep_output("proto.proto_lib")"#)
            .eval()
            .unwrap();
        assert_eq!(result, "../proto/build/proto.bin");
        // The dep ref recorded must be the GLOBAL key so DAG edge wiring lines up
        // with recipe_leaves (which uses globally-qualified names).
        let state = cs.borrow();
        assert_eq!(state.step_group_dep_refs, vec!["queue.proto.proto_lib".to_string()]);
    }

    #[test]
    fn test_dep_output_empty_qualified_prefix_no_translation() {
        let (lua, outputs, cs) = setup_lua();
        outputs.lock().unwrap().insert(
            "proto.proto_lib".into(),
            vec!["build/proto.bin".into()],
        );
        let mut alias_dirs = BTreeMap::new();
        alias_dirs.insert("proto".to_string(), PathBuf::from("../proto"));

        register_dep_output_api(&lua, outputs, cs.clone(), alias_dirs, String::new())
            .unwrap();
        let result: String = lua
            .load(r#"return cook.dep_output("proto.proto_lib")"#)
            .eval()
            .unwrap();
        assert_eq!(result, "../proto/build/proto.bin");
        let state = cs.borrow();
        assert_eq!(state.step_group_dep_refs, vec!["proto.proto_lib".to_string()]);
    }
}
