use super::*;

/// Build a workspace-of-one directly (no files besides the tempdir) so
/// the error-mapping tests exercise register_workspace — the sole
/// registration path after the dual-path collapse.
fn workspace_of_one(dir: &Path, lua_source: &str) -> Workspace {
    Workspace {
        root: LoadedCookfile {
            // Intentionally inert placeholder AST: registration consumes
            // only `lua_source`; the parsed Cookfile is never re-lowered.
            cookfile: cook_lang::parse("recipe placeholder\n")
                .expect("placeholder Cookfile parses"),
            lua_source: lua_source.to_string(),
            dir: dir.to_path_buf(),
        },
        imports: BTreeMap::new(),
        namespace_map: Vec::new(),
        workspace_root: dir.to_path_buf(),
        warnings: Vec::new(),
    }
}

#[test]
fn register_workspace_preserves_ingredient_warning_order() {
    let dir = tempfile::tempdir().unwrap();
    let workspace = workspace_of_one(
        dir.path(),
        r#"
                cook.recipe("first", {ingredients = {"a.none"}, excludes = {}}, function() end)
                cook.recipe("second", {ingredients = {"b.none"}, excludes = {}}, function() end)
            "#,
    );
    let registered = register_workspace(
        &workspace,
        None,
        &[],
        RegisterMode::Enumerate,
        None,
        None,
    )
    .unwrap();
    assert_eq!(
        registered.warnings,
        vec![
            "ingredient \"a.none\" matched 0 files (recipe first)",
            "ingredient \"b.none\" matched 0 files (recipe second)",
        ]
    );
}

/// SHI-222 Phase 5 Task 5.6: `register_workspace` must surface
/// `RegisterError::RecipeCollision` as a structured
/// `PipelineError::RecipeCollision { name, sites }` (not as
/// `PipelineError::Other`), so the CLI can render the multi-line
/// per-site diagnostic at emit time (spec §8) and exit with code 3.
#[test]
fn register_workspace_maps_collision_to_typed_variant() {
    let lua_src = r#"
            cook.recipe("build", {requires = {}}, function() end)
            cook.recipe("build", {requires = {}}, function() end)
        "#;
    let tmpdir = tempfile::TempDir::new().unwrap();
    let ws = workspace_of_one(tmpdir.path(), lua_src);
    let result = register_workspace(&ws, None, &[], RegisterMode::Enumerate, None, None);

    match result {
        Ok(_) => panic!("expected PipelineError::RecipeCollision, got Ok"),
        Err(PipelineError::RecipeCollision { name, sites }) => {
            assert_eq!(name, "build");
            assert_eq!(sites.len(), 2, "both register-phase sites are captured");
            // Both are dynamic `cook.recipe(...)` calls — confirms the
            // typed mapping passes the kind through faithfully.
            for s in &sites {
                assert_eq!(s.kind, cook_register::RegistrationSiteKind::Dynamic);
            }
        }
        Err(other) => panic!("expected PipelineError::RecipeCollision, got {other:?}"),
    }
}

/// `RegisterError` variants other than `RecipeCollision` continue to fall
/// through to `PipelineError::Other` (pre-Task-5.6 behavior preserved).
/// Exercises the fallthrough arm of `map_register_error` via a Lua-level
/// error in the cookfile source.
#[test]
fn register_workspace_maps_non_collision_to_other() {
    // Top-level Lua error (undefined function) → RegisterError::Lua →
    // PipelineError::Other.
    let lua_src = "this_function_does_not_exist()\n";
    let tmpdir = tempfile::TempDir::new().unwrap();
    let ws = workspace_of_one(tmpdir.path(), lua_src);
    let result = register_workspace(&ws, None, &[], RegisterMode::Enumerate, None, None);

    match result {
        Ok(_) => panic!("expected PipelineError::Other, got Ok"),
        Err(PipelineError::Other(_)) => {}
        Err(other) => panic!("expected PipelineError::Other, got {other:?}"),
    }
}

/// Workspace-of-one discovery — a recipe registered at
/// register-phase (invisible to static codegen) must be folded into the
/// $<NAME> classification set when the workspace path re-codegens.
#[test]
fn codegen_with_module_recipes_discovers_dynamic_recipe_workspace_of_one() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(
        dir.path().join("Cookfile"),
        "recipe consume\n    cook \"build/out\" { cat $<gen> > $<out> }\n",
    )
    .unwrap();
    let entry = dir.path().join("Cookfile");
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let mut ws = Workspace::load(&entry, &root, &[]).unwrap();
    // Static codegen cannot see `gen` → mis-lowers to require_var.
    assert!(ws.root.lua_source.contains("cook.require_var(\"gen\")"));
    // Simulate a module-registered recipe: append a dynamic registration
    // to the discovery Lua (list_names sees it; bodies never run).
    ws.root.lua_source.push_str(
        "\ncook.recipe(\"gen\", {requires = {}}, function() end)\n",
    );
    codegen_with_module_recipes(&mut ws, None, &[]).unwrap();
    assert!(
        ws.root.lua_source.contains("cook.dep_output(\"gen\")"),
        "expected $<gen> re-lowered to dep_output, got:\n{}",
        ws.root.lua_source
    );
}

/// The discovery pass must also cover IMPORTED members —
/// an importer's `$<alias.recipe>` where `recipe` is module-registered in
/// the importee must re-lower to dep_output on the workspace path.
#[test]
fn codegen_with_module_recipes_discovers_dynamic_recipe_in_importee() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::create_dir_all(dir.path().join("lib")).unwrap();
    std::fs::write(
        dir.path().join("lib/Cookfile"),
        "recipe lib_static\n    cook \"lib.o\" { echo $<out> }\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("Cookfile"),
        "import lib ./lib\nrecipe top\n    cook \"build/top\" { cat $<lib.gen> > $<out> }\n",
    )
    .unwrap();
    std::fs::write(dir.path().join(".cookroot"), "").unwrap();
    let entry = dir.path().join("Cookfile");
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let mut ws = Workspace::load(&entry, &root, &[]).unwrap();
    assert!(ws.root.lua_source.contains("cook.require_var(\"lib.gen\")"));
    // Simulate a module-registered recipe in the importee.
    let lib_canon = std::fs::canonicalize(dir.path().join("lib")).unwrap();
    ws.imports
        .get_mut(&lib_canon)
        .unwrap()
        .lua_source
        .push_str("\ncook.recipe(\"gen\", {requires = {}}, function() end)\n");
    codegen_with_module_recipes(&mut ws, None, &[]).unwrap();
    assert!(
        ws.root.lua_source.contains("cook.dep_output(\"lib.gen\")"),
        "expected $<lib.gen> re-lowered to dep_output, got:\n{}",
        ws.root.lua_source
    );
}

/// §20.2.3 cache-identity invariance: the same member Cookfile must
/// register its units with IDENTICAL `CacheMeta.cookfile_path` and
/// `CacheMeta.recipe_name` whether it is reached as an import of the
/// enclosing workspace root (entry = root/Cookfile, registered under
/// prefix "rust") or as the entry Cookfile itself (workspace-of-one
/// root, prefix ""). The invocation directory must not influence the
/// cache namespace.
#[test]
fn cache_meta_is_invocation_independent_across_entry_points() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(
        dir.path().join("Cookfile"),
        "import rust apps/rust\n\nrecipe check\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.path().join("apps/rust")).unwrap();
    std::fs::write(
        dir.path().join("apps/rust/Cookfile"),
        concat!(
            "recipe build\n",
            "        cook.add_unit({\n",
            "            inputs  = { },\n",
            "            outputs = { \"build/out.txt\" },\n",
            "            command = \"mkdir -p build && echo hi > build/out.txt\",\n",
            "        })\n",
        ),
    )
    .unwrap();
    std::fs::write(dir.path().join(".cookroot"), "").unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();

    // (i) Entry = the workspace root Cookfile; member registers as an
    //     import under prefix "rust".
    let ws_root = Workspace::load(&root.join("Cookfile"), &root, &[]).unwrap();
    let reg_root = register_workspace(&ws_root, None, &[], RegisterMode::Enumerate, None, None).unwrap();

    // (ii) Entry = the member Cookfile itself (invoked inside apps/rust);
    //      it registers as the workspace-of-one root under prefix "".
    let ws_member =
        Workspace::load(&root.join("apps/rust/Cookfile"), &root, &[]).unwrap();
    let reg_member = register_workspace(&ws_member, None, &[], RegisterMode::Enumerate, None, None).unwrap();

    let meta_of = |reg: &RegisteredWorkspace, key: &str| {
        reg.units_by_recipe
            .get(key)
            .unwrap_or_else(|| panic!("recipe '{key}' registered"))
            .units
            .first()
            .unwrap_or_else(|| panic!("recipe '{key}' has a unit"))
            .cache_meta
            .clone()
            .unwrap_or_else(|| panic!("recipe '{key}' unit has cache_meta"))
    };

    let meta_i = meta_of(&reg_root, "rust.build");
    let meta_ii = meta_of(&reg_member, "build");

    assert_eq!(
        meta_i.cookfile_path, meta_ii.cookfile_path,
        "cookfile_path must not depend on the entry point"
    );
    assert_eq!(
        meta_i.recipe_name, meta_ii.recipe_name,
        "recipe_name must not depend on the entry point"
    );
    assert_eq!(meta_i.cookfile_path, "apps/rust/Cookfile");
    assert_eq!(meta_i.recipe_name, "build");
}

/// Nested-import discovery: extras must be qualified with the LOCAL
/// alias of the direct importer (a's `$<b.gen>`), and must NOT leak
/// into members that don't import the discoverer directly (root).
#[test]
fn codegen_with_module_recipes_qualifies_extras_with_local_alias_only() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::create_dir_all(dir.path().join("a/b")).unwrap();
    std::fs::write(
        dir.path().join("a/b/Cookfile"),
        "recipe b_static\n    cook \"b.o\" { echo $<out> }\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("a/Cookfile"),
        "import b ./b\nrecipe mid\n    cook \"mid.o\" { cat $<b.gen> > $<out> }\n",
    )
    .unwrap();
    // Root ALSO references `$<b.gen>` — but root does not import b
    // directly, so its reference must STAY mis-lowered (require_var)
    // after discovery: extras reach direct importers only.
    std::fs::write(
        dir.path().join("Cookfile"),
        "import a ./a\nrecipe top\n    cook \"top.o\" { cat $<a.mid> $<b.gen> > $<out> }\n",
    )
    .unwrap();
    std::fs::write(dir.path().join(".cookroot"), "").unwrap();
    let entry = dir.path().join("Cookfile");
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let mut ws = Workspace::load(&entry, &root, &[]).unwrap();
    let b_canon = std::fs::canonicalize(dir.path().join("a/b")).unwrap();
    ws.imports
        .get_mut(&b_canon)
        .unwrap()
        .lua_source
        .push_str("\ncook.recipe(\"gen\", {requires = {}}, function() end)\n");
    codegen_with_module_recipes(&mut ws, None, &[]).unwrap();
    let a_canon = std::fs::canonicalize(dir.path().join("a")).unwrap();
    let a_lua = &ws.imports.get(&a_canon).unwrap().lua_source;
    assert!(
        a_lua.contains("cook.dep_output(\"b.gen\")"),
        "a's $<b.gen> must re-lower via its LOCAL alias, got:\n{a_lua}"
    );
    assert!(
        ws.root.lua_source.contains("cook.require_var(\"b.gen\")"),
        "root's $<b.gen> must stay mis-lowered (root does not import b \
         directly, so b's extras must not reach its union), got:\n{}",
        ws.root.lua_source
    );
    assert!(
        !ws.root.lua_source.contains("cook.dep_output(\"b.gen\")"),
        "root must not gain a dep_output for b.gen, got:\n{}",
        ws.root.lua_source
    );
}

/// COOK-352: `merge_into` must qualify `RecipeUnits::deps` on the same footing
/// as `recipe_name`, because every consumer downstream compares them against
/// qualified names.
///
/// `run.rs` rebuilds a recipe's coarse barrier set by intersecting `units.deps`
/// with the qualified closure `edges`. While `deps` carried member-LOCAL names,
/// that intersection was empty for every recipe in every workspace — the engine
/// received `deps: []` throughout, and §16.1.2's read-after-write rule could
/// only ever be satisfied through `dep_edges` (a `$<producer>` body reference).
/// It rejected legitimate builds with "recipe 'B' does not require 'A'" while
/// B's header said exactly that.
///
/// `cook-engine`'s own `literal_read_after_write.rs` did not catch it: those
/// tests construct `RecipeUnits` by hand with matching unqualified names, so
/// they exercise the predicate and never `merge_into`.
#[test]
fn register_workspace_qualifies_recipe_units_deps() {
    let dir = tempfile::tempdir().unwrap();
    let member = dir.path().join("mem");
    std::fs::create_dir_all(&member).unwrap();
    std::fs::write(dir.path().join("Cookfile"), "import mem ./mem\n").unwrap();
    std::fs::write(
        member.join("Cookfile"),
        "recipe gen\n    cook \"out/g.c\" { echo g > $<out> }\n\
         \nrecipe use: gen\n    cook \"out/f.o\" { echo f > $<out> }\n",
    )
    .unwrap();

    let workspace = Workspace::load(
        &dir.path().join("Cookfile"),
        dir.path(),
        &[],
    )
    .expect("workspace loads");

    let registered =
        register_workspace(&workspace, None, &[], RegisterMode::Enumerate, None, None)
            .expect("register");

    let use_units = registered
        .units_by_recipe
        .get("mem.use")
        .expect("mem.use registered under its qualified name");

    assert_eq!(
        use_units.deps,
        vec!["mem.gen".to_string()],
        "deps must be workspace-qualified to match `recipe_name` and the \
         closure `edges` they are intersected against; got {:?}",
        use_units.deps
    );
}
