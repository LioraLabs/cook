use super::*;

fn make_cookfile(recipes: Vec<Recipe>) -> Cookfile {
    Cookfile {
        config_blocks: vec![],
        recipes,
        chores: vec![],
        uses: vec![],
        imports: vec![],
        register_blocks: vec![],
        top_level_module_calls: vec![],
        probes: vec![],
    }
}

fn make_recipe(name: &str, steps: Vec<Step>) -> Recipe {
    Recipe {
        name: name.to_string(),
        deps: vec![],
        ingredients: vec![],
        excludes: vec![],
        steps,
        line: 1,
    }
}

#[test]
fn test_extract_recipe_names() {
    let cookfile = make_cookfile(vec![
        make_recipe("libmath", vec![]),
        make_recipe("backend", vec![]),
    ]);
    let names = extract_recipe_names(&cookfile);
    assert_eq!(names.len(), 2);
    assert!(names.contains("libmath"));
    assert!(names.contains("backend"));
}

#[test]
fn test_extract_sigil_tokens() {
    let tokens = extract_sigil_tokens("gcc -c $<in> -o $<out> $<libmath>");
    assert_eq!(tokens, vec!["in", "out", "libmath"]);
}

#[test]
fn test_extract_sigil_tokens_with_accessor() {
    let tokens = extract_sigil_tokens("build/$<protos.stem>.o");
    assert_eq!(tokens, vec!["protos.stem"]);
}

#[test]
fn test_parse_dep_token_plain_recipe() {
    let mut names = BTreeSet::new();
    names.insert("libmath".to_string());
    let result = parse_dep_token("libmath", &names);
    assert_eq!(
        result,
        Some(DepRef { recipe_name: "libmath".to_string(), accessor: None })
    );
}

#[test]
fn test_parse_dep_token_with_accessor() {
    let mut names = BTreeSet::new();
    names.insert("protos".to_string());
    let result = parse_dep_token("protos.stem", &names);
    assert_eq!(
        result,
        Some(DepRef {
            recipe_name: "protos".to_string(),
            accessor: Some("stem".to_string()),
        })
    );
}

#[test]
fn test_parse_dep_token_dotted_recipe_name() {
    // "backend.build" is itself a recipe name — should match with no accessor
    let mut names = BTreeSet::new();
    names.insert("backend.build".to_string());
    let result = parse_dep_token("backend.build", &names);
    assert_eq!(
        result,
        Some(DepRef { recipe_name: "backend.build".to_string(), accessor: None })
    );
}

#[test]
fn test_parse_dep_token_env_var() {
    let mut names = BTreeSet::new();
    names.insert("libmath".to_string());
    let result = parse_dep_token("CC", &names);
    assert_eq!(result, None);
}

#[test]
fn test_parse_dep_token_builtin_ignored() {
    let names = BTreeSet::new();
    assert_eq!(parse_dep_token("in", &names), None);
    assert_eq!(parse_dep_token("out", &names), None);
    assert_eq!(parse_dep_token("stem", &names), None);
    // "all" is no longer a builtin (COOK-195); this stays None here only
    // because `names` has no recipe called "all" registered — not because
    // "all" is skipped as a builtin.
    assert_eq!(parse_dep_token("all", &names), None);
}

#[test]
fn test_extract_dep_refs_from_cook_step() {
    let mut recipe_names = BTreeSet::new();
    recipe_names.insert("libmath".to_string());
    recipe_names.insert("libstr".to_string());

        let recipe = make_recipe(
            "app",
        vec![Step::Cook {
            step: CookStep {
                outputs: vec![OutputPattern::Quoted("build/app".to_string())],
                body: Some(Body::ShellBlock(
                    vec!["gcc -o $<out> $<in> $<libmath> $<libstr>".to_string()],
                )),
                disposition: Default::default(),
            },
            line: 2,
        }],
    );

    let refs = extract_dep_refs(&recipe, &recipe_names);
    assert_eq!(refs.len(), 2);
    assert!(refs.contains(&DepRef { recipe_name: "libmath".to_string(), accessor: None }));
    assert!(refs.contains(&DepRef { recipe_name: "libstr".to_string(), accessor: None }));
    }

    #[test]
    fn test_extract_dep_refs_from_output_pattern() {
        let mut recipe_names = BTreeSet::new();
        recipe_names.insert("protos".to_string());

    let recipe = make_recipe(
        "app",
        vec![Step::Cook {
            step: CookStep {
                outputs: vec![OutputPattern::Quoted("build/$<protos.stem>.pb.cc".to_string())],
                body: None,
                disposition: Default::default(),
            },
            line: 2,
        }],
    );

    let refs = extract_dep_refs(&recipe, &recipe_names);
    assert_eq!(refs.len(), 1);
    assert!(refs.contains(&DepRef {
        recipe_name: "protos".to_string(),
        accessor: Some("stem".to_string()),
    }));
}

// ── CS-0022 tests ────────────────────────────────────────────────

#[test]
fn cs_0022_in_and_out_are_not_dep_refs() {
    let mut names = BTreeSet::new();
    names.insert("libmath".to_string());

    assert!(parse_dep_token("in.stem", &names).is_none(),
        "in.stem is an own-input accessor, not a dep ref");
    assert!(parse_dep_token("out.dir", &names).is_none(),
            "out.dir is an output accessor, not a dep ref");
    assert!(parse_dep_token("out_1.stem", &names).is_none(),
        "out_1.stem is a multi-output accessor, not a dep ref");
    assert_eq!(
        parse_dep_token("libmath.stem", &names).map(|d| d.recipe_name),
        Some("libmath".to_string()),
        "libmath.stem is a genuine dep ref"
    );
}

#[test]
fn test_extract_recipe_names_with_imports_includes_aliased() {
    use std::collections::BTreeMap;

    let lib_cookfile = make_cookfile(vec![
        make_recipe("lib_build", vec![]),
        make_recipe("lib_test", vec![]),
    ]);
    let main_cookfile = make_cookfile(vec![make_recipe("demo", vec![])]);

    let mut imports_by_alias: BTreeMap<String, &Cookfile> = BTreeMap::new();
    imports_by_alias.insert("lib".to_string(), &lib_cookfile);

    let names = extract_recipe_names_with_imports(&main_cookfile, &imports_by_alias);
    assert!(names.contains("demo"));
    assert!(names.contains("lib.lib_build"));
    assert!(names.contains("lib.lib_test"));
    assert_eq!(names.len(), 3);
}

#[test]
fn test_extract_recipe_names_with_imports_no_imports_equals_local() {
    use std::collections::BTreeMap;

    let cookfile = make_cookfile(vec![make_recipe("a", vec![]), make_recipe("b", vec![])]);
    let imports_by_alias: BTreeMap<String, &Cookfile> = BTreeMap::new();
    let names = extract_recipe_names_with_imports(&cookfile, &imports_by_alias);
    let local = extract_recipe_names(&cookfile);
    assert_eq!(names, local);
}

#[test]
fn parse_dep_token_strips_bracket_index_for_recipe_member() {
    let mut names = BTreeSet::new();
    names.insert("render".to_string());
        // COOK-221 / CS-0137: the per-member spelling is `[in]`.
        assert_eq!(
            parse_dep_token("render[in]", &names),
        Some(DepRef { recipe_name: "render".to_string(), accessor: None })
        );
        // bare recipe still works
        assert_eq!(
            parse_dep_token("render", &names),
        Some(DepRef { recipe_name: "render".to_string(), accessor: None })
        );
        // the removed `[]` spelling contributes no edge (the resolver rejects
        // the placeholder with a did-you-mean before any unit registers)
        assert_eq!(parse_dep_token("render[]", &names), None);
    // `[in]` on a non-recipe is not a dep
    assert_eq!(parse_dep_token("notarecipe[in]", &names), None);
}

#[test]
fn cs_0022_shell_block_dep_ref_extraction() {
    // Shell block with $<NAME> references must be extracted.
    let mut recipe_names = BTreeSet::new();
    recipe_names.insert("libmath".to_string());

    let recipe = make_recipe(
        "app",
        vec![Step::Cook {
            step: CookStep {
                outputs: vec![OutputPattern::Quoted("build/app".to_string())],
                body: Some(Body::ShellBlock(vec![
                    "gcc -o $<out> main.c $<libmath>".to_string(),
                ])),
                disposition: Default::default(),
            },
            line: 2,
        }],
    );

    let refs = extract_dep_refs(&recipe, &recipe_names);
    assert!(
        refs.iter().any(|r| r.recipe_name == "libmath" && r.accessor.is_none()),
        "shell block must contribute its $<libmath> reference to the dep graph; got: {:?}",
        refs
    );
}
