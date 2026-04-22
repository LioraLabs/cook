use cook_lang::ast::*;

use crate::generate;
use crate::lua_string::escape_lua_string;
use crate::template::expand_template_to_lua;

fn make_cookfile(recipes: Vec<Recipe>) -> Cookfile {
    Cookfile {
        vars: vec![],
        config_blocks: vec![],
        recipes,
        uses: vec![],
        imports: vec![],
    }
}

fn make_recipe(
    name: &str,
    deps: Vec<&str>,
    ingredients: Vec<&str>,
    steps: Vec<Step>,
) -> Recipe {
    Recipe {
        name: name.to_string(),
        deps: deps.into_iter().map(String::from).collect(),
        ingredients: ingredients.into_iter().map(String::from).collect(),
        excludes: vec![],
        steps,
        line: 1,
    }
}

#[test]
fn test_expand_template_no_placeholders() {
    let result = expand_template_to_lua("echo hello");
    assert_eq!(result, "\"echo hello\"");
}

#[test]
fn test_expand_template_single_placeholder() {
    let result = expand_template_to_lua("{in}");
    assert_eq!(result, "_cook_in");
}

#[test]
fn test_expand_template_mixed() {
    let result = expand_template_to_lua("gcc -c {in} -o {out}");
    assert_eq!(
        result,
        "\"gcc -c \" .. _cook_in .. \" -o \" .. _cook_out"
    );
}

#[test]
fn test_expand_template_stem_in_path() {
    let result = expand_template_to_lua("build/{stem}.o");
    assert_eq!(result, "\"build/\" .. _cook_stem .. \".o\"");
}

#[test]
fn test_expand_template_all() {
    let result = expand_template_to_lua("ar rcs {out} {all}");
    assert_eq!(
        result,
        "\"ar rcs \" .. _cook_out .. \" \" .. _cook_all"
    );
}

#[test]
fn test_minimal_recipe() {
    let cookfile = make_cookfile(vec![make_recipe(
        "build",
        vec![],
        vec![],
        vec![Step::Shell {
            command: "echo hello".to_string(),
            line: 2,
            interactive: false,
        }],
    )]);
    let output = generate(&cookfile);
    assert!(output.contains("cook.recipe(\"build\", {}, function()"));
    assert!(
        output.contains("cook.add_unit({command = [[echo hello]], cache = false})"),
        "Shell steps should emit cook.add_unit with cache=false, got:\n{output}"
    );
    // Shell steps should NOT have cook.layer or cook.exec
    assert!(!output.contains("cook.layer"), "Shell steps should not use cook.layer");
    assert!(!output.contains("cook.exec"), "Shell steps should not use cook.exec");
}

#[test]
fn test_recipe_with_deps_and_ingredients() {
    let cookfile = make_cookfile(vec![make_recipe(
        "build",
        vec!["clean"],
        vec!["src/*.c"],
        vec![],
    )]);
    let output = generate(&cookfile);
    assert!(output.contains("ingredients = {\"src/*.c\"}"));
    assert!(output.contains("requires = {\"clean\"}"));
}

#[test]
fn test_cook_step_one_to_one() {
    let cookfile = make_cookfile(vec![make_recipe(
        "build",
        vec![],
        vec!["src/*.c"],
        vec![Step::Cook {
            step: CookStep {
                outputs: vec!["build/{stem}.o".to_string()],
                using_clause: Some(UsingClause::Shell(
                    "gcc -c {in} -o {out}".to_string(),
                )),
            },
            line: 3,
        }],
    )]);
    let output = generate(&cookfile);
    assert!(output.contains("cook.step_group(function()"), "missing cook.step_group");
    assert!(output.contains("local _cook_outputs_1 = {}"));
    assert!(output.contains("for _, _cook_in in ipairs(recipe.ingredients[1]) do"));
    assert!(output.contains("local _cook_stem = path.stem(_cook_in)"));
    assert!(output.contains("local _cook_out = \"build/\" .. _cook_stem .. \".o\""));
    assert!(
        output.contains("cook.add_unit({inputs = {_cook_in}, output = _cook_out, command = "),
        "missing cook.add_unit call, got:\n{output}"
    );
    assert!(output.contains("table.insert(_cook_outputs_1, _cook_out)"));
    // Should NOT have old API calls
    assert!(!output.contains("cook.layer"), "should not use cook.layer");
    assert!(!output.contains("cook.exec"), "should not use cook.exec");
    assert!(!output.contains("cook.begin_step"), "should not use cook.begin_step");
    assert!(!output.contains("cook.end_step"), "should not use cook.end_step");
}

#[test]
fn test_cook_step_many_to_one() {
    let cookfile = make_cookfile(vec![make_recipe(
        "build",
        vec![],
        vec!["src/*.c"],
        vec![
            Step::Cook {
                step: CookStep {
                    outputs: vec!["build/{stem}.o".to_string()],
                    using_clause: Some(UsingClause::Shell(
                        "gcc -c {in} -o {out}".to_string(),
                    )),
                },
                line: 3,
            },
            Step::Cook {
                step: CookStep {
                    outputs: vec!["build/lib.a".to_string()],
                    using_clause: Some(UsingClause::Shell(
                        "ar rcs {out} {all}".to_string(),
                    )),
                },
                line: 4,
            },
        ],
    )]);
    let output = generate(&cookfile);
    assert!(output.contains("local _cook_outputs_2 = {}"));
    assert!(output.contains("local _cook_all = table.concat(_cook_outputs_1, \" \")"));
    assert!(output.contains("local _cook_out = \"build/lib.a\""));
    assert!(
        output.contains("cook.add_unit({inputs = _cook_outputs_1, output = _cook_out, command = "),
        "missing cook.add_unit for many-to-one, got:\n{output}"
    );
    assert!(output.contains("table.insert(_cook_outputs_2, _cook_out)"));
    // Should NOT have old API calls
    assert!(!output.contains("cook.layer"), "should not use cook.layer");
    assert!(!output.contains("cook.exec"), "should not use cook.exec");
}

#[test]
fn test_cook_step_declaration() {
    let cookfile = make_cookfile(vec![make_recipe(
        "build",
        vec![],
        vec![],
        vec![Step::Cook {
            step: CookStep {
                outputs: vec!["bin/app".to_string()],
                using_clause: None,
            },
            line: 3,
        }],
    )]);
    let output = generate(&cookfile);
    // Output variable is hoisted to recipe scope (empty), then populated
    assert!(output.contains("local _cook_outputs_1 = {}"));
    assert!(output.contains("_cook_outputs_1[1] = \"bin/app\""));
    // DeclarationOnly should NOT have cook.add_unit
    assert!(!output.contains("cook.add_unit"), "DeclarationOnly should not emit cook.add_unit");
}

#[test]
fn test_cook_step_lua_block() {
    let cookfile = make_cookfile(vec![make_recipe(
        "build",
        vec![],
        vec!["src/*.c"],
        vec![Step::Cook {
            step: CookStep {
                outputs: vec!["build/{stem}.o".to_string()],
                using_clause: Some(UsingClause::LuaBlock(
                    "cook.sh(\"gcc -c \" .. input .. \" -o \" .. output)".to_string(),
                )),
            },
            line: 3,
        }],
    )]);
    let output = generate(&cookfile);
    assert!(
        output.contains("cook.add_unit({inputs = {_cook_in}, output = _cook_out, lua = function()"),
        "missing cook.add_unit with lua function, got:\n{output}"
    );
    assert!(output.contains("local input = _cook_in"));
    assert!(output.contains("local output = _cook_out"));
    assert!(output.contains("local input_1 = recipe.ingredients[1]"));
    assert!(output.contains("cook.sh(\"gcc -c \" .. input .. \" -o \" .. output)"));
    // Should NOT have old API
    assert!(!output.contains("cook.layer"), "should not use cook.layer");
}

#[test]
fn test_plate_step() {
    let cookfile = make_cookfile(vec![make_recipe(
        "run",
        vec![],
        vec![],
        vec![
            Step::Cook {
                step: CookStep {
                    outputs: vec!["bin/app".to_string()],
                    using_clause: None,
                },
                line: 2,
            },
            Step::Plate {
                step: PlateStep {
                    command: "./{out}".to_string(),
                },
                line: 3,
            },
        ],
    )]);
    let output = generate(&cookfile);
    assert!(output.contains("cook.step_group(function()"), "missing cook.step_group for plate");
    assert!(output.contains("for _, _plate_out in ipairs(_cook_outputs_1) do"));
    assert!(
        output.contains("cook.add_unit({command = "),
        "missing cook.add_unit for plate, got:\n{output}"
    );
    assert!(
        output.contains("cache = false"),
        "plate step should have cache = false, got:\n{output}"
    );
    // Should NOT have old API
    assert!(!output.contains("cook.layer"), "should not use cook.layer");
    assert!(!output.contains("cook.exec"), "should not use cook.exec");
}

#[test]
fn test_lua_line_emitted() {
    let cookfile = make_cookfile(vec![make_recipe(
        "test",
        vec![],
        vec![],
        vec![Step::Lua {
            code: "print(\"hello\")".to_string(),
            line: 2,
        }],
    )]);
    let output = generate(&cookfile);
    assert!(output.contains("    print(\"hello\")"));
    assert!(!output.contains("cook.exec"));
    assert!(!output.contains("cook.add_unit"));
}

#[test]
fn test_shell_with_double_brackets() {
    let cookfile = make_cookfile(vec![make_recipe(
        "test",
        vec![],
        vec![],
        vec![Step::Shell {
            command: "echo ]]".to_string(),
            line: 2,
            interactive: false,
        }],
    )]);
    let output = generate(&cookfile);
    assert!(output.contains("[=[echo ]]]=]"));
}

#[test]
fn test_escape_lua_string() {
    assert_eq!(escape_lua_string("hello"), "hello");
    assert_eq!(escape_lua_string("he\"llo"), "he\\\"llo");
    assert_eq!(escape_lua_string("he\\llo"), "he\\\\llo");
    assert_eq!(escape_lua_string("he\nllo"), "he\\nllo");
}

#[test]
fn test_cook_step_emits_step_group() {
    let cookfile = make_cookfile(vec![make_recipe(
        "build",
        vec![],
        vec!["src/*.c"],
        vec![Step::Cook {
            step: CookStep {
                outputs: vec!["build/{stem}.o".to_string()],
                using_clause: Some(UsingClause::Shell(
                    "gcc -c {in} -o {out}".to_string(),
                )),
            },
            line: 3,
        }],
    )]);
    let output = generate(&cookfile);
    assert!(
        output.contains("cook.step_group(function()"),
        "cook step should be wrapped in cook.step_group"
    );
    assert!(
        !output.contains("cook.begin_step()"),
        "should not use old cook.begin_step()"
    );
    assert!(
        !output.contains("cook.end_step()"),
        "should not use old cook.end_step()"
    );
    // Verify ordering: step_group before the loop, end) after
    let group_pos = output.find("cook.step_group(function()").unwrap();
    let loop_pos = output.find("for _, _cook_in").unwrap();
    assert!(group_pos < loop_pos, "step_group should come before the loop");
}

#[test]
fn test_plate_step_emits_step_group() {
    let cookfile = make_cookfile(vec![make_recipe(
        "run",
        vec![],
        vec![],
        vec![
            Step::Cook {
                step: CookStep {
                    outputs: vec!["bin/app".to_string()],
                    using_clause: None,
                },
                line: 2,
            },
            Step::Plate {
                step: PlateStep {
                    command: "./{out}".to_string(),
                },
                line: 3,
            },
        ],
    )]);
    let output = generate(&cookfile);
    // Should have step_group around both cook (declaration) and plate steps
    let markers: Vec<_> = output.match_indices("cook.step_group(function()").collect();
    // Cook (declaration) has step_group, Plate has step_group = 2
    assert_eq!(markers.len(), 2, "expected 2 step_group calls, got:\n{output}");
    // Should NOT have old markers
    assert!(!output.contains("cook.begin_step()"), "should not use old cook.begin_step()");
    assert!(!output.contains("cook.end_step()"), "should not use old cook.end_step()");
}

#[test]
fn test_config_var_in_cook_step() {
    let cookfile = make_cookfile(vec![make_recipe(
        "build",
        vec![],
        vec!["src/*.c"],
        vec![Step::Cook {
            step: CookStep {
                outputs: vec!["build/{stem}.o".to_string()],
                using_clause: Some(UsingClause::Shell(
                    "{CC} {CFLAGS} -c {in} -o {out}".to_string(),
                )),
            },
            line: 3,
        }],
    )]);
    let output = generate(&cookfile);
    assert!(output.contains("_cook_in"), "should expand {{in}} to _cook_in");
    assert!(output.contains("_cook_out"), "should expand {{out}} to _cook_out");
    assert!(
        output.contains(r#"cook.env["CC"]"#),
        "should expand {{CC}} to cook.env[\"CC\"], got: {}",
        output
    );
    assert!(
        output.contains(r#"cook.env["CFLAGS"]"#),
        "should expand {{CFLAGS}} to cook.env[\"CFLAGS\"], got: {}",
        output
    );
}

#[test]
fn test_config_var_only_template() {
    let cookfile = make_cookfile(vec![make_recipe(
        "build",
        vec![],
        vec!["src/*.c"],
        vec![Step::Cook {
            step: CookStep {
                outputs: vec!["build/{stem}.o".to_string()],
                using_clause: Some(UsingClause::Shell(
                    "{CC} -c {in} -o {out}".to_string(),
                )),
            },
            line: 3,
        }],
    )]);
    let output = generate(&cookfile);
    assert!(output.contains(r#"cook.env["CC"]"#));
    assert!(output.contains("_cook_in"));
    assert!(output.contains("_cook_out"));
}

#[test]
fn test_no_config_vars_unchanged() {
    let cookfile = make_cookfile(vec![make_recipe(
        "build",
        vec![],
        vec!["src/*.c"],
        vec![Step::Cook {
            step: CookStep {
                outputs: vec!["build/{stem}.o".to_string()],
                using_clause: Some(UsingClause::Shell(
                    "gcc -c {in} -o {out}".to_string(),
                )),
            },
            line: 3,
        }],
    )]);
    let output = generate(&cookfile);
    assert!(output.contains("\"gcc -c \""));
    assert!(!output.contains("cook.env"));
}

#[test]
fn test_interactive_shell_step() {
    let cookfile = make_cookfile(vec![make_recipe(
        "run",
        vec![],
        vec![],
        vec![Step::Shell {
            command: "./bin/app".to_string(),
            line: 5,
            interactive: true,
        }],
    )]);
    let output = generate(&cookfile);
    assert!(
        output.contains("cook.add_unit({command = [[./bin/app]], interactive = true, line = 5, cache = false})"),
        "expected cook.add_unit with interactive=true, got: {output}"
    );
    assert!(
        !output.contains("cook.interactive"),
        "interactive step should not emit old cook.interactive"
    );
    assert!(
        !output.contains("cook.exec"),
        "interactive step should not emit cook.exec"
    );
}

#[test]
fn test_cook_step_lua_block_no_raw_string() {
    let cookfile = make_cookfile(vec![make_recipe(
        "build",
        vec![],
        vec!["src/*.c"],
        vec![Step::Cook {
            step: CookStep {
                outputs: vec!["build/{stem}.o".to_string()],
                using_clause: Some(UsingClause::LuaBlock(
                    "cook.sh(\"gcc -c \" .. input .. \" -o \" .. output)".to_string(),
                )),
            },
            line: 3,
        }],
    )]);
    let output = generate(&cookfile);
    // New API uses lua = function() ... end} -- no raw [=[ string needed
    assert!(
        output.contains("lua = function()"),
        "LuaBlock should use lua = function()"
    );
    assert!(
        output.contains("end})"),
        "LuaBlock should close with end{{}})"
    );
    // Should NOT contain old-style [=[ raw string passthrough
    assert!(
        !output.contains("[=["),
        "New API should not emit [=[ long string"
    );
}

#[test]
fn test_use_generates_load_module() {
    let cookfile = Cookfile {
        vars: vec![],
        config_blocks: vec![],
        recipes: vec![make_recipe("build", vec![], vec![], vec![
            Step::Shell { command: "echo hi".to_string(), line: 2, interactive: false },
        ])],
        uses: vec![
            UseStatement { module_name: "cpp".to_string(), line: 1 },
        ],
        imports: vec![],
    };
    let output = generate(&cookfile);
    assert!(output.contains(r#"local cpp = cook.load_module("cpp")"#));
    let load_pos = output.find("cook.load_module").unwrap();
    let recipe_pos = output.find("cook.recipe").unwrap();
    assert!(load_pos < recipe_pos);
}

#[test]
fn test_test_step_codegen() {
    let cookfile = make_cookfile(vec![make_recipe(
        "run-tests",
        vec![],
        vec!["tests/*.c"],
        vec![
            Step::Cook {
                step: CookStep {
                    outputs: vec!["build/{stem}".to_string()],
                    using_clause: Some(UsingClause::Shell(
                        "cc {in} -o {out}".to_string(),
                    )),
                },
                line: 3,
            },
            Step::Test {
                step: TestStep {
                    command: "./{out}".to_string(),
                    timeout: None,
                    should_fail: false,
                },
                line: 4,
            },
        ],
    )]);
    let output = generate(&cookfile);
    assert!(
        output.contains("cook.add_test("),
        "expected cook.add_test in:\n{output}"
    );
    assert!(
        output.contains("_test_out"),
        "expected _test_out variable in:\n{output}"
    );
    assert!(
        output.contains("timeout = 300"),
        "expected default timeout 300 in:\n{output}"
    );
    assert!(
        output.contains("should_fail = false"),
        "expected should_fail = false in:\n{output}"
    );
    // Should NOT have old API
    assert!(!output.contains("cook.test_layer"), "should not use old cook.test_layer");
}

#[test]
fn test_test_step_codegen_with_options() {
    let cookfile = make_cookfile(vec![make_recipe(
        "run-tests",
        vec![],
        vec![],
        vec![Step::Test {
            step: TestStep {
                command: "./{out}".to_string(),
                timeout: Some(30),
                should_fail: true,
            },
            line: 2,
        }],
    )]);
    let output = generate(&cookfile);
    assert!(
        output.contains("timeout = 30"),
        "expected timeout = 30 in:\n{output}"
    );
    assert!(
        output.contains("should_fail = true"),
        "expected should_fail = true in:\n{output}"
    );
}

#[test]
fn test_multiple_uses_generate_in_order() {
    let cookfile = Cookfile {
        vars: vec![],
        config_blocks: vec![],
        recipes: vec![],
        uses: vec![
            UseStatement { module_name: "cpp".to_string(), line: 1 },
            UseStatement { module_name: "proto".to_string(), line: 2 },
        ],
        imports: vec![],
    };
    let output = generate(&cookfile);
    let cpp_pos = output.find(r#"local cpp = cook.load_module("cpp")"#).unwrap();
    let proto_pos = output.find(r#"local proto = cook.load_module("proto")"#).unwrap();
    assert!(cpp_pos < proto_pos);
}

#[test]
fn test_no_hash_in_output() {
    // Verify that the new codegen does NOT emit hash values
    let cookfile = make_cookfile(vec![make_recipe(
        "build",
        vec![],
        vec!["src/*.c"],
        vec![
            Step::Cook {
                step: CookStep {
                    outputs: vec!["build/{stem}.o".to_string()],
                    using_clause: Some(UsingClause::Shell(
                        "gcc -c {in} -o {out}".to_string(),
                    )),
                },
                line: 3,
            },
            Step::Plate {
                step: PlateStep {
                    command: "./{out}".to_string(),
                },
                line: 4,
            },
        ],
    )]);
    let output = generate(&cookfile);
    // Old API passed hash as a numeric literal to cook.layer -- this should be gone
    assert!(!output.contains("cook.layer"), "should not contain cook.layer with hash argument");
}

#[test]
fn test_shell_step_no_step_group() {
    // Shell steps are standalone, not wrapped in step_group
    let cookfile = make_cookfile(vec![make_recipe(
        "clean",
        vec![],
        vec![],
        vec![Step::Shell {
            command: "rm -rf build".to_string(),
            line: 2,
            interactive: false,
        }],
    )]);
    let output = generate(&cookfile);
    assert!(
        output.contains("cook.add_unit({command = [[rm -rf build]], cache = false})"),
        "shell step should emit add_unit, got:\n{output}"
    );
    // Shell steps should NOT be wrapped in step_group
    assert!(
        !output.contains("cook.step_group"),
        "raw shell steps should not be wrapped in step_group"
    );
}

#[test]
fn test_test_step_wrapped_in_step_group() {
    let cookfile = make_cookfile(vec![make_recipe(
        "test",
        vec![],
        vec![],
        vec![Step::Test {
            step: TestStep {
                command: "./{out}".to_string(),
                timeout: None,
                should_fail: false,
            },
            line: 2,
        }],
    )]);
    let output = generate(&cookfile);
    assert!(
        output.contains("cook.step_group(function()"),
        "test step should be wrapped in step_group, got:\n{output}"
    );
}

#[test]
fn test_recipe_with_excludes() {
    let cookfile = make_cookfile(vec![Recipe {
        name: "lib".to_string(),
        deps: vec![],
        ingredients: vec!["src/*.c".to_string()],
        excludes: vec!["src/lua.c".to_string(), "src/luac.c".to_string()],
        steps: vec![],
        line: 1,
    }]);
    let output = generate(&cookfile);
    assert!(
        output.contains(r#"excludes = {"src/lua.c", "src/luac.c"}"#),
        "expected excludes in metadata, got:\n{output}"
    );
    assert!(output.contains(r#"ingredients = {"src/*.c"}"#));
}

#[test]
fn test_recipe_without_excludes() {
    let cookfile = make_cookfile(vec![make_recipe(
        "build",
        vec![],
        vec!["src/*.c"],
        vec![],
    )]);
    let output = generate(&cookfile);
    assert!(!output.contains("excludes"), "should not emit excludes when empty");
}

#[test]
fn test_ingredients_lua_variable_emitted() {
    let recipe = make_recipe(
        "build",
        vec![],
        vec!["src/*.c"],
        vec![Step::Lua {
            code: "print(ingredients)".to_string(),
            line: 3,
        }],
    );
    let cookfile = make_cookfile(vec![recipe]);
    let output = generate(&cookfile);
    assert!(
        output.contains("local ingredients = cook.resolve_ingredients("),
        "should emit ingredients variable, got:\n{}",
        output
    );
    assert!(output.contains("\"src/*.c\""));
}

#[test]
fn test_ingredients_lua_variable_with_excludes() {
    let mut recipe = make_recipe(
        "build",
        vec![],
        vec!["src/*.c"],
        vec![Step::Lua {
            code: "print(ingredients)".to_string(),
            line: 3,
        }],
    );
    recipe.excludes = vec!["src/skip.c".to_string()];
    let cookfile = make_cookfile(vec![recipe]);
    let output = generate(&cookfile);
    assert!(output.contains("\"src/*.c\""));
    assert!(output.contains("\"src/skip.c\""));
}

#[test]
fn test_no_ingredients_no_variable() {
    let recipe = make_recipe(
        "clean",
        vec![],
        vec![],
        vec![Step::Shell {
            command: "rm -rf build".to_string(),
            line: 2,
            interactive: false,
        }],
    );
    let cookfile = make_cookfile(vec![recipe]);
    let output = generate(&cookfile);
    assert!(
        !output.contains("cook.resolve_ingredients"),
        "should NOT emit ingredients variable for recipe without ingredients"
    );
}

// ── Recipe-name-aware template expansion ─────────────────────────

#[test]
fn test_dep_ref_in_command_emits_dep_output() {
    let names: std::collections::BTreeSet<String> =
        ["libmath", "libstr"].iter().map(|s| s.to_string()).collect();
    let cookfile = make_cookfile(vec![make_recipe(
        "app",
        vec![],
        vec!["src/main.c"],
        vec![
            Step::Cook {
                step: CookStep {
                    outputs: vec!["build/obj/main.o".to_string()],
                    using_clause: Some(UsingClause::Shell("gcc -c {in} -o {out}".into())),
                },
                line: 3,
            },
            Step::Cook {
                step: CookStep {
                    outputs: vec!["build/bin/app".to_string()],
                    using_clause: Some(UsingClause::Shell(
                        "gcc -o {out} {in} {libmath} {libstr}".into(),
                    )),
                },
                line: 4,
            },
        ],
    )]);
    let output = crate::generate_with_names(&cookfile, &names);
    assert!(
        output.contains(r#"cook.dep_output("libmath")"#),
        "expected cook.dep_output for libmath, got:\n{output}"
    );
    assert!(
        output.contains(r#"cook.dep_output("libstr")"#),
        "expected cook.dep_output for libstr, got:\n{output}"
    );
    assert!(output.contains("_cook_in"), "built-in {{in}} should still work");
    assert!(output.contains("_cook_out"), "built-in {{out}} should still work");
}

#[test]
fn test_env_var_still_works_when_not_recipe() {
    let names: std::collections::BTreeSet<String> =
        ["libmath"].iter().map(|s| s.to_string()).collect();
    let cookfile = make_cookfile(vec![make_recipe(
        "build",
        vec![],
        vec!["src/*.c"],
        vec![Step::Cook {
            step: CookStep {
                outputs: vec!["build/{stem}.o".to_string()],
                using_clause: Some(UsingClause::Shell("{CC} -c {in} -o {out}".into())),
            },
            line: 3,
        }],
    )]);
    let output = crate::generate_with_names(&cookfile, &names);
    assert!(
        output.contains(r#"cook.env["CC"]"#),
        "CC is not a recipe name, should be env var, got:\n{output}"
    );
}

#[test]
fn test_dep_ref_in_plate_command() {
    let names: std::collections::BTreeSet<String> =
        ["app"].iter().map(|s| s.to_string()).collect();
    let cookfile = make_cookfile(vec![make_recipe(
        "run",
        vec![],
        vec![],
        vec![
            Step::Cook {
                step: CookStep {
                    outputs: vec!["bin/runner".to_string()],
                    using_clause: None,
                },
                line: 2,
            },
            Step::Plate {
                step: PlateStep {
                    command: "./{out} {app}".to_string(),
                },
                line: 3,
            },
        ],
    )]);
    let output = crate::generate_with_names(&cookfile, &names);
    assert!(
        output.contains(r#"cook.dep_output("app")"#),
        "expected cook.dep_output for app in plate step, got:\n{output}"
    );
}

// ── Dep-driven iteration codegen ─────────────────────────────────

#[test]
fn test_dep_driven_iteration_codegen() {
    let names: std::collections::BTreeSet<String> =
        ["protos"].iter().map(|s| s.to_string()).collect();
    let cookfile = make_cookfile(vec![make_recipe(
        "compile_protos",
        vec![],
        vec![],
        vec![Step::Cook {
            step: CookStep {
                outputs: vec!["build/obj/{protos.stem}.o".to_string()],
                using_clause: Some(UsingClause::Shell("gcc -c {in} -o {out}".into())),
            },
            line: 2,
        }],
    )]);
    let output = crate::generate_with_names(&cookfile, &names);
    assert!(
        output.contains(r#"cook.dep_output_list("protos")"#),
        "should use dep_output_list for iteration, got:\n{output}"
    );
    assert!(
        output.contains("path.stem(_cook_in)"),
        "should extract stem from dep items, got:\n{output}"
    );
    assert!(
        !output.contains("recipe.ingredients"),
        "should NOT iterate over own ingredients, got:\n{output}"
    );
}

#[test]
fn test_dep_driven_followed_by_many_to_one() {
    let names: std::collections::BTreeSet<String> =
        ["protos"].iter().map(|s| s.to_string()).collect();
    let cookfile = make_cookfile(vec![make_recipe(
        "compile_protos",
        vec![],
        vec![],
        vec![
            Step::Cook {
                step: CookStep {
                    outputs: vec!["build/obj/{protos.stem}.o".to_string()],
                    using_clause: Some(UsingClause::Shell("gcc -c {in} -o {out}".into())),
                },
                line: 2,
            },
            Step::Cook {
                step: CookStep {
                    outputs: vec!["build/lib/libprotos.a".to_string()],
                    using_clause: Some(UsingClause::Shell("ar rcs {out} {all}".into())),
                },
                line: 3,
            },
        ],
    )]);
    let output = crate::generate_with_names(&cookfile, &names);
    // Second step uses _cook_outputs_1 (from first step), not dep outputs
    assert!(
        output.contains("table.concat(_cook_outputs_1"),
        "second step should chain from first step, got:\n{output}"
    );
}

#[test]
fn test_mixed_dep_iteration_and_substitution() {
    let names: std::collections::BTreeSet<String> =
        ["protos", "core"].iter().map(|s| s.to_string()).collect();
    let cookfile = make_cookfile(vec![make_recipe(
        "server",
        vec![],
        vec![],
        vec![Step::Cook {
            step: CookStep {
                outputs: vec!["build/obj/{protos.stem}.o".to_string()],
                using_clause: Some(UsingClause::Shell(
                    "gcc -c {in} -I{core}/include -o {out}".into(),
                )),
            },
            line: 2,
        }],
    )]);
    let output = crate::generate_with_names(&cookfile, &names);
    // Iteration driven by protos
    assert!(output.contains(r#"cook.dep_output_list("protos")"#));
    // String substitution of core in command
    assert!(output.contains(r#"cook.dep_output("core")"#));
}

// ── Config block dispatcher emission ─────────────────────────────

#[test]
fn test_codegen_emits_unnamed_config_block() {
    let cookfile = Cookfile {
        vars: vec![],
        config_blocks: vec![ConfigBlock {
            name: None,
            body: "env.CC = \"gcc\"".to_string(),
            line: 1,
        }],
        recipes: vec![],
        uses: vec![],
        imports: vec![],
    };
    let out = generate(&cookfile);
    assert!(out.contains("function __cook_run_config_blocks"));
    assert!(out.contains("env.CC = \"gcc\""));
}

#[test]
fn test_codegen_emits_named_config_block() {
    let cookfile = Cookfile {
        vars: vec![],
        config_blocks: vec![ConfigBlock {
            name: Some("release".to_string()),
            body: "env.CXXFLAGS = \"-O3\"".to_string(),
            line: 1,
        }],
        recipes: vec![],
        uses: vec![],
        imports: vec![],
    };
    let out = generate(&cookfile);
    assert!(out.contains("function __cook_run_config_blocks"));
    assert!(out.contains("selected_name == \"release\""));
    assert!(out.contains("env.CXXFLAGS = \"-O3\""));
}

#[test]
fn test_codegen_skips_dispatcher_when_no_config_blocks() {
    let cookfile = Cookfile {
        vars: vec![],
        config_blocks: vec![],
        recipes: vec![],
        uses: vec![],
        imports: vec![],
    };
    let out = generate(&cookfile);
    assert!(!out.contains("__cook_run_config_blocks"));
}

#[test]
fn test_codegen_emits_unnamed_and_named_in_order() {
    let cookfile = Cookfile {
        vars: vec![],
        config_blocks: vec![
            ConfigBlock { name: None,                           body: "base()".into(), line: 1 },
            ConfigBlock { name: Some("dev".to_string()),        body: "dev()".into(),  line: 4 },
            ConfigBlock { name: Some("release".to_string()),    body: "rel()".into(),  line: 7 },
        ],
        recipes: vec![],
        uses: vec![],
        imports: vec![],
    };
    let out = generate(&cookfile);
    let base_idx = out.find("base()").unwrap();
    let dev_idx = out.find("dev()").unwrap();
    let rel_idx = out.find("rel()").unwrap();
    // Unnamed body appears before the `if selected_name` block containing named ones.
    assert!(base_idx < dev_idx);
    assert!(base_idx < rel_idx);
    // Both named-block bodies appear in the generated dispatcher.
    assert!(out.contains("selected_name == \"dev\""));
    assert!(out.contains("selected_name == \"release\""));
}

// ── Cross-recipe dep integration tests ──────────────────────────

#[test]
fn test_cross_recipe_deps_codegen_integration() {
    // Simulate the cross-recipe-deps example
    let names: std::collections::BTreeSet<String> =
        ["libmath", "libstr", "app"].iter().map(|s| s.to_string()).collect();

    let cookfile = make_cookfile(vec![
        make_recipe("libmath", vec![], vec!["src/math/*.c"], vec![
            Step::Cook {
                step: CookStep {
                    outputs: vec!["build/obj/math/{stem}.o".into()],
                    using_clause: Some(UsingClause::Shell("gcc -c {in} -o {out}".into())),
                },
                line: 3,
            },
            Step::Cook {
                step: CookStep {
                    outputs: vec!["build/lib/libmath.a".into()],
                    using_clause: Some(UsingClause::Shell("ar rcs {out} {all}".into())),
                },
                line: 4,
            },
        ]),
        make_recipe("libstr", vec![], vec!["src/str/*.c"], vec![
            Step::Cook {
                step: CookStep {
                    outputs: vec!["build/obj/str/{stem}.o".into()],
                    using_clause: Some(UsingClause::Shell("gcc -c {in} -o {out}".into())),
                },
                line: 8,
            },
            Step::Cook {
                step: CookStep {
                    outputs: vec!["build/lib/libstr.a".into()],
                    using_clause: Some(UsingClause::Shell("ar rcs {out} {all}".into())),
                },
                line: 9,
            },
        ]),
        make_recipe("app", vec![], vec!["src/main.c"], vec![
            Step::Cook {
                step: CookStep {
                    outputs: vec!["build/obj/main.o".into()],
                    using_clause: Some(UsingClause::Shell("gcc -c {in} -o {out}".into())),
                },
                line: 13,
            },
            Step::Cook {
                step: CookStep {
                    outputs: vec!["build/bin/app".into()],
                    using_clause: Some(UsingClause::Shell(
                        "gcc -o {out} {in} {libmath} {libstr}".into(),
                    )),
                },
                line: 14,
            },
        ]),
    ]);

    // Pre-scan extracts recipe names
    let extracted = crate::dep_ref::extract_recipe_names(&cookfile);
    assert_eq!(extracted, names);

    // Dep ref extraction
    let app_recipe = cookfile.recipes.iter().find(|r| r.name == "app").unwrap();
    let app_refs = crate::dep_ref::extract_dep_refs(app_recipe, &names);
    let dep_recipe_names: std::collections::BTreeSet<String> =
        app_refs.iter().map(|r| r.recipe_name.clone()).collect();
    assert!(dep_recipe_names.contains("libmath"));
    assert!(dep_recipe_names.contains("libstr"));

    // Codegen produces correct Lua
    let lua = crate::generate_with_names(&cookfile, &names);
    assert!(lua.contains(r#"cook.dep_output("libmath")"#), "missing dep_output for libmath");
    assert!(lua.contains(r#"cook.dep_output("libstr")"#), "missing dep_output for libstr");

    // libmath recipe should NOT have dep_output calls (it has no deps)
    let libmath_section = lua.split("cook.recipe(\"libmath\"").nth(1).unwrap();
    let libmath_end = libmath_section.find("cook.recipe(").unwrap_or(libmath_section.len());
    let libmath_lua = &libmath_section[..libmath_end];
    assert!(!libmath_lua.contains("cook.dep_output"),
        "libmath should have no dep_output calls");
}

// ── BlockStep codegen (multi-output) ─────────────────────────────

#[test]
fn blockstep_shell_multi_output() {
    let source = r#"recipe "wasm"
    ingredients "src/*.rs"
    cook "a.js" "b.wasm" using {
        wasm-pack build
        cp x a.js
        cp y b.wasm
    }
end
"#;
    let cookfile = cook_lang::parse(source).expect("parse");
    let lua = crate::generate(&cookfile);
    // Should produce a _cook_outs table with both declared outputs:
    assert!(
        lua.contains(r#"_cook_outs = {"a.js", "b.wasm"}"#),
        "generated Lua missing outs table: {lua}"
    );
    // Should emit sh() for each command line:
    assert!(lua.contains(r#"sh("wasm-pack build")"#), "missing sh call: {lua}");
    assert!(lua.contains(r#"sh("cp x a.js")"#), "missing sh call: {lua}");
    assert!(lua.contains(r#"sh("cp y b.wasm")"#), "missing sh call: {lua}");
    // Should not iterate per input:
    let for_count = lua.matches("for _, _cook_in in ipairs").count();
    assert_eq!(for_count, 0, "BlockStep should not emit a per-input loop: {lua}");
}

#[test]
fn blockstep_lua_multi_output() {
    let source = r#"recipe "wasm"
    ingredients "src/*.rs"
    cook "a.js" "b.wasm" using >{
        sh("wasm-pack build")
    }
end
"#;
    let cookfile = cook_lang::parse(source).expect("parse");
    let lua = crate::generate(&cookfile);
    // Lua block with N > 1 outputs -> BlockStep, not OneToOne.
    assert!(
        lua.contains(r#"_cook_outs = {"a.js", "b.wasm"}"#),
        "generated Lua missing outs table: {lua}"
    );
    let for_count = lua.matches("for _, _cook_in in ipairs").count();
    assert_eq!(for_count, 0, "BlockStep should not emit a per-input loop: {lua}");
}

#[test]
fn test_dep_ref_wave_grouping_integration() {
    let names: std::collections::BTreeSet<String> =
        ["libmath", "libstr", "app", "run"].iter().map(|s| s.to_string()).collect();

    // app uses {libmath} and {libstr} -> inferred deps
    // run: app -> explicit dep
    let mut inferred_deps = std::collections::BTreeMap::new();
    inferred_deps.insert("app".to_string(), vec!["libmath".to_string(), "libstr".to_string()]);

    let mut explicit_deps = std::collections::BTreeMap::new();
    explicit_deps.insert("run".to_string(), vec!["app".to_string()]);
    explicit_deps.insert("app".to_string(), vec![]);
    explicit_deps.insert("libmath".to_string(), vec![]);
    explicit_deps.insert("libstr".to_string(), vec![]);

    let waves = cook_engine::wave_grouper::compute_waves(&explicit_deps, &inferred_deps, &names).unwrap();

    assert_eq!(waves.len(), 2, "should have 2 waves");
    // Wave 1: libmath, libstr, app (same wave via inferred deps)
    assert_eq!(waves[0].recipes.len(), 3);
    assert!(waves[0].recipes.contains(&"libmath".to_string()));
    assert!(waves[0].recipes.contains(&"libstr".to_string()));
    assert!(waves[0].recipes.contains(&"app".to_string()));
    // Wave 2: run (after explicit dep on app)
    assert_eq!(waves[1].recipes, vec!["run".to_string()]);
}
