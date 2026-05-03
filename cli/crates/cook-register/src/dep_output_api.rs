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
/// `qualified_prefix` is the workspace-global prefix of the Cookfile that owns this registry,
/// used to resolve **same-Cookfile** name references (e.g. a `dep_output("local_recipe")` call
/// inside `libs/queue`'s Lua, where queue's prefix is `"queue"`, looks up `"queue.local_recipe"`).
///
/// `alias_qualified_prefixes` maps each local alias to its **importee's canonical workspace
/// prefix**. This is what `cook.dep_output("alias.recipe")` uses to resolve cross-Cookfile
/// references. Diamond imports make this distinct from `qualified_prefix`: when both
/// `apps/cli` and `apps/server → libs/queue` reach `libs/proto`, proto has one canonical
/// storage prefix (e.g. `"server.queue.proto"` if that path won the prefix-assignment race),
/// and *both* importers' local alias `"proto"` must resolve to that same canonical prefix.
/// Prepending the calling registry's own `qualified_prefix` would yield `"cli.proto"` from
/// cli's POV — wrong. The map is computed once per Registry by pipeline.rs and supplies the
/// importee's *canonical* prefix for every direct alias.
///
/// Path rewriting via `alias_dirs` keeps using the **local** alias unchanged — the alias is
/// the local name's first dot-component, regardless of the importee's qualified prefix.
pub fn register_dep_output_api(
    lua: &Lua,
    terminal_outputs: SharedTerminalOutputs,
    capture_state: SharedCaptureState,
    alias_dirs: BTreeMap<String, PathBuf>,
    qualified_prefix: String,
    alias_qualified_prefixes: BTreeMap<String, String>,
) -> LuaResult<()> {
    let cook: LuaTable = lua.globals().get("cook")?;

    let alias_dirs = Arc::new(alias_dirs);
    let qualified_prefix = Arc::new(qualified_prefix);
    let alias_qualified_prefixes = Arc::new(alias_qualified_prefixes);

    // cook.dep_output(name) → space-joined string
    // Accumulates dep ref in step_group_dep_refs; actual edge recording
    // happens in cook.add_unit() which attaches the ref to the correct unit.
    let to = terminal_outputs.clone();
    let cs = capture_state.clone();
    let ad = alias_dirs.clone();
    let qp = qualified_prefix.clone();
    let aqp = alias_qualified_prefixes.clone();
    let dep_output_fn = lua.create_function(move |_, name: String| {
        let global_key = resolve_global_key(&name, &qp, &aqp);
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
    let aqp2 = alias_qualified_prefixes.clone();
    let dep_output_list_fn = lua.create_function(move |lua, name: String| {
        let global_key = resolve_global_key(&name, &qp2, &aqp2);
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

/// Translate a Lua-visible name (`"local_recipe"` or `"alias.recipe"`) into the
/// workspace-global storage key in `terminal_outputs`.
///
/// - `"alias.recipe"` where `alias` is in `alias_qualified_prefixes`: the global key is
///   `<importee_prefix>.<recipe>` (where `importee_prefix` is the alias's canonical
///   workspace prefix; if empty, the global key is just `<recipe>`).
/// - Any other shape (no dot, or a dot whose left side isn't a known import alias): treated
///   as a same-Cookfile reference. The global key is `<self_prefix>.<name>` (or just
///   `<name>` when `self_prefix` is empty).
fn resolve_global_key(
    name: &str,
    self_prefix: &str,
    alias_qualified_prefixes: &BTreeMap<String, String>,
) -> String {
    if let Some((alias, sub)) = name.split_once('.') {
        if let Some(importee_prefix) = alias_qualified_prefixes.get(alias) {
            return if importee_prefix.is_empty() {
                sub.to_string()
            } else {
                format!("{}.{}", importee_prefix, sub)
            };
        }
    }
    if self_prefix.is_empty() {
        name.to_string()
    } else {
        format!("{}.{}", self_prefix, name)
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
        register_dep_output_api(&lua, outputs, cs, BTreeMap::new(), String::new(), BTreeMap::new()).unwrap();
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
        register_dep_output_api(&lua, outputs, cs, BTreeMap::new(), String::new(), BTreeMap::new()).unwrap();
        let result: Vec<String> = lua
            .load(r#"return cook.dep_output_list("libmath")"#)
            .eval()
            .unwrap();
        assert_eq!(result, vec!["build/lib/libmath.a"]);
    }

    #[test]
    fn test_dep_output_unknown_recipe_errors() {
        let (lua, outputs, cs) = setup_lua();
        register_dep_output_api(&lua, outputs, cs, BTreeMap::new(), String::new(), BTreeMap::new()).unwrap();
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
        register_dep_output_api(&lua, outputs, cs.clone(), BTreeMap::new(), String::new(), BTreeMap::new()).unwrap();
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
        register_dep_output_api(&lua, outputs, cs.clone(), BTreeMap::new(), String::new(), BTreeMap::new()).unwrap();
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

        register_dep_output_api(&lua, outputs, cs, alias_dirs, String::new(), BTreeMap::new()).unwrap();
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
        register_dep_output_api(&lua, outputs, cs, BTreeMap::new(), String::new(), BTreeMap::new()).unwrap();
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

        register_dep_output_api(&lua, outputs, cs, alias_dirs, String::new(), BTreeMap::new()).unwrap();
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

        register_dep_output_api(&lua, outputs, cs, alias_dirs, String::new(), BTreeMap::new()).unwrap();
        let result: Vec<String> = lua
            .load(r#"return cook.dep_output_list("lib.lib_build")"#)
            .eval()
            .unwrap();
        assert_eq!(result, vec!["lib/build/foo.o", "lib/build/bar.o"]);
    }

    /// Transitive sigil case: when `apps/server` invokes a recipe whose chain is
    /// `server → //libs/queue → //libs/proto`, queue's registry knows that its local
    /// alias `"proto"` resolves to the canonical importee prefix `"server.queue.proto"`.
    /// Queue's Lua calls `cook.dep_output("proto.proto_lib")` and the lookup must
    /// reach `"server.queue.proto.proto_lib"`.
    #[test]
    fn test_dep_output_resolves_via_alias_qualified_prefix() {
        let (lua, outputs, cs) = setup_lua();
        outputs.lock().unwrap().insert(
            "server.queue.proto.proto_lib".into(),
            vec!["build/proto.bin".into()],
        );
        let mut alias_dirs = BTreeMap::new();
        alias_dirs.insert("proto".to_string(), PathBuf::from("../proto"));
        let mut alias_qp = BTreeMap::new();
        alias_qp.insert("proto".to_string(), "server.queue.proto".to_string());

        register_dep_output_api(
            &lua,
            outputs,
            cs.clone(),
            alias_dirs,
            "server.queue".to_string(),
            alias_qp,
        )
        .unwrap();
        let result: String = lua
            .load(r#"return cook.dep_output("proto.proto_lib")"#)
            .eval()
            .unwrap();
        assert_eq!(result, "../proto/build/proto.bin");
        // The dep ref recorded must be the GLOBAL key so DAG edge wiring lines up
        // with recipe_leaves (which uses globally-qualified names).
        let state = cs.borrow();
        assert_eq!(
            state.step_group_dep_refs,
            vec!["server.queue.proto.proto_lib".to_string()]
        );
    }

    /// Diamond case: `apps/cli` and `apps/server → libs/queue` both reach `libs/proto`.
    /// proto has ONE canonical storage prefix (e.g. `"server.queue.proto"`). cli's local
    /// alias `"proto"` MUST also resolve to the same canonical prefix — not to
    /// `"cli.proto.proto_lib"` (which doesn't exist). The alias_qualified_prefixes map
    /// is what makes this work: pipeline.rs supplies cli's map as
    /// `{"proto" → "server.queue.proto"}`, the canonical importee prefix.
    #[test]
    fn test_dep_output_diamond_resolves_to_canonical_importee_prefix() {
        let (lua, outputs, cs) = setup_lua();
        // proto's canonical storage key (from server's chain winning find_full_prefix).
        outputs.lock().unwrap().insert(
            "server.queue.proto.proto_lib".into(),
            vec!["build/proto.bin".into()],
        );
        let mut alias_dirs = BTreeMap::new();
        // cli's importer-relative path to proto: ../../libs/proto.
        alias_dirs.insert("proto".to_string(), PathBuf::from("../../libs/proto"));
        let mut alias_qp = BTreeMap::new();
        // CLI's alias "proto" → proto's canonical workspace prefix.
        alias_qp.insert("proto".to_string(), "server.queue.proto".to_string());

        // CLI's own qualified_prefix is "cli". Without the alias-map indirection,
        // CS-0028's prefix-prepend would look up "cli.proto.proto_lib" → fail.
        register_dep_output_api(
            &lua,
            outputs,
            cs.clone(),
            alias_dirs,
            "cli".to_string(),
            alias_qp,
        )
        .unwrap();
        let result: String = lua
            .load(r#"return cook.dep_output("proto.proto_lib")"#)
            .eval()
            .unwrap();
        assert_eq!(result, "../../libs/proto/build/proto.bin");
        let state = cs.borrow();
        assert_eq!(
            state.step_group_dep_refs,
            vec!["server.queue.proto.proto_lib".to_string()]
        );
    }

    /// Same-Cookfile reference (no dot) falls back to self-prefix qualification.
    /// A registry with `qualified_prefix = "queue"` calling
    /// `cook.dep_output("local_recipe")` must look up `"queue.local_recipe"`.
    #[test]
    fn test_dep_output_same_cookfile_uses_self_prefix() {
        let (lua, outputs, cs) = setup_lua();
        outputs.lock().unwrap().insert(
            "queue.local_recipe".into(),
            vec!["build/local.bin".into()],
        );

        register_dep_output_api(
            &lua,
            outputs,
            cs.clone(),
            BTreeMap::new(),
            "queue".to_string(),
            BTreeMap::new(),
        )
        .unwrap();
        let result: String = lua
            .load(r#"return cook.dep_output("local_recipe")"#)
            .eval()
            .unwrap();
        assert_eq!(result, "build/local.bin");
        let state = cs.borrow();
        assert_eq!(state.step_group_dep_refs, vec!["queue.local_recipe".to_string()]);
    }

    /// Empty self-prefix and empty alias map (entry-point Cookfile, no imports):
    /// the local name is the global key directly.
    #[test]
    fn test_dep_output_empty_qualified_prefix_no_translation() {
        let (lua, outputs, cs) = setup_lua();
        outputs.lock().unwrap().insert(
            "local_recipe".into(),
            vec!["build/local.bin".into()],
        );

        register_dep_output_api(
            &lua,
            outputs,
            cs.clone(),
            BTreeMap::new(),
            String::new(),
            BTreeMap::new(),
        )
        .unwrap();
        let result: String = lua
            .load(r#"return cook.dep_output("local_recipe")"#)
            .eval()
            .unwrap();
        assert_eq!(result, "build/local.bin");
        let state = cs.borrow();
        assert_eq!(state.step_group_dep_refs, vec!["local_recipe".to_string()]);
    }
}
