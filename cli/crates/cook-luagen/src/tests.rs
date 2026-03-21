use cook_lang::ast::*;

use crate::generate;
use crate::lua_string::escape_lua_string;
use crate::template::expand_template_to_lua;

fn make_cookfile(recipes: Vec<Recipe>) -> Cookfile {
    Cookfile {
        vars: vec![],
        configs: std::collections::BTreeMap::new(),
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
                output_pattern: "build/{stem}.o".to_string(),
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
                    output_pattern: "build/{stem}.o".to_string(),
                    using_clause: Some(UsingClause::Shell(
                        "gcc -c {in} -o {out}".to_string(),
                    )),
                },
                line: 3,
            },
            Step::Cook {
                step: CookStep {
                    output_pattern: "build/lib.a".to_string(),
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
                output_pattern: "bin/app".to_string(),
                using_clause: None,
            },
            line: 3,
        }],
    )]);
    let output = generate(&cookfile);
    assert!(output.contains("local _cook_outputs_1 = {\"bin/app\"}"));
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
                output_pattern: "build/{stem}.o".to_string(),
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
                    output_pattern: "bin/app".to_string(),
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
                output_pattern: "build/{stem}.o".to_string(),
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
                    output_pattern: "bin/app".to_string(),
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
                output_pattern: "build/{stem}.o".to_string(),
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
                output_pattern: "build/{stem}.o".to_string(),
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
                output_pattern: "build/{stem}.o".to_string(),
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
        output.contains("cook.add_unit({command = [[./bin/app]], interactive = true, cache = false})"),
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
                output_pattern: "build/{stem}.o".to_string(),
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
        configs: std::collections::BTreeMap::new(),
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
                    output_pattern: "build/{stem}".to_string(),
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
        configs: std::collections::BTreeMap::new(),
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
                    output_pattern: "build/{stem}.o".to_string(),
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
