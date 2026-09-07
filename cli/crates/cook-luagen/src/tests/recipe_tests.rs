use super::*;

fn recipe(name: &str, output: &str, command: &str) -> Recipe {
    Recipe {
        name: name.to_string(),
        description: None,
        deps: vec![],
        inputs: vec![],
        excludes: vec![],
        steps: vec![Step::Cook {
            step: CookStep {
                outputs: vec![output.into()],
                body: Some(Body::ShellBlock(vec![command.to_string()])),
                disposition: Disposition::default(),
            },
            line: 1,
        }],
        line: 1,
    }
}

#[test]
fn colon_qualified_recipe_output_uses_dep_output_and_infers_the_edge() {
    let cookfile = Cookfile {
        config_blocks: vec![],
        recipes: vec![
            recipe("dotnet:build", "build/app.dll", "dotnet build -o $<out>"),
            recipe("aggregate", "build/all.txt", "cat $<dotnet:build> > $<out>"),
        ],
        chores: vec![],
        uses: vec![],
        imports: vec![],
        register_blocks: vec![],
        top_level_module_calls: vec![],
        probes: vec![],
    };
    let names = crate::dep_ref::extract_recipe_names(&cookfile);
    let lua = crate::generate_checked(&cookfile, &names)
        .expect("codegen")
        .0;

    assert!(
        lua.contains(r#"requires = {"dotnet:build"}"#),
        "lua:\n{lua}"
    );
    assert!(
        lua.contains(r#"cook.dep_output("dotnet:build")"#),
        "lua:\n{lua}"
    );
}

#[test]
fn colon_qualified_recipe_accessor_uses_dep_output() {
    let cookfile = Cookfile {
        config_blocks: vec![],
        recipes: vec![recipe(
            "dotnet:build",
            "build/app.dll",
            "dotnet build -o $<out>",
        )],
        chores: vec![Chore {
            name: "package".to_string(),
            description: None,
            params: vec![],
            deps: vec![],
            steps: vec![Step::Shell {
                command: "cat $<dotnet:build.stem>".to_string(),
                line: 1,
                interactive: true,
            }],
            line: 1,
        }],
        uses: vec![],
        imports: vec![],
        register_blocks: vec![],
        top_level_module_calls: vec![],
        probes: vec![],
    };
    let names = crate::dep_ref::extract_recipe_names(&cookfile);
    let lua = crate::generate_checked(&cookfile, &names)
        .expect("codegen")
        .0;

    assert!(
        lua.contains(r#"path.stem(cook.dep_output("dotnet:build"))"#),
        "lua:\n{lua}"
    );
}

#[test]
fn builtin_placeholder_outranks_same_named_recipe() {
    let recipes = BTreeSet::from(["out_1".to_string()]);
    let probes = BTreeSet::new();
    let ctx = ResolveCtx {
        mode: IterMode::ManyToOne,
        outputs: OutputShape::Multi(1),
        recipes_in_scope: &recipes,
        probe_keys_in_scope: &probes,
    };

    assert_eq!(
        crate::resolver::resolve("out_1", &ctx),
        crate::resolver::Resolved::Builtin(crate::resolver::BuiltinKind::OutIndexed(1))
    );
}
