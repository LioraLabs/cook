use cook_lang::ast::*;

use crate::compile_chore;
use crate::generate;
use crate::lua_string::escape_lua_string;

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
    // No sigil placeholders — string passes through as a quoted literal.
    use std::collections::BTreeSet;
    use crate::resolver::{IterMode, OutputShape, ResolveCtx};
    use crate::template::{ConsultedEnv, expand_sigil_template};
    let r = BTreeSet::new();
    let ctx = ResolveCtx { mode: IterMode::OneToOne, outputs: OutputShape::Single, recipes_in_scope: &r };
    let mut env = ConsultedEnv::new();
    let result = expand_sigil_template("echo hello", &ctx, &mut env, &mut crate::template::FileRefs::new("t")).unwrap();
    assert_eq!(result, "\"echo hello\"");
}

#[test]
fn test_expand_template_single_placeholder() {
    use std::collections::BTreeSet;
    use crate::resolver::{IterMode, OutputShape, ResolveCtx};
    use crate::template::{ConsultedEnv, expand_sigil_template};
    let r = BTreeSet::new();
    let ctx = ResolveCtx { mode: IterMode::OneToOne, outputs: OutputShape::Single, recipes_in_scope: &r };
    let mut env = ConsultedEnv::new();
    let result = expand_sigil_template("$<in>", &ctx, &mut env, &mut crate::template::FileRefs::new("t")).unwrap();
    assert_eq!(result, "_cook_in");
}

#[test]
fn test_expand_template_mixed() {
    use std::collections::BTreeSet;
    use crate::resolver::{IterMode, OutputShape, ResolveCtx};
    use crate::template::{ConsultedEnv, expand_sigil_template};
    let r = BTreeSet::new();
    let ctx = ResolveCtx { mode: IterMode::OneToOne, outputs: OutputShape::Single, recipes_in_scope: &r };
    let mut env = ConsultedEnv::new();
    let result = expand_sigil_template("gcc -c $<in> -o $<out>", &ctx, &mut env, &mut crate::template::FileRefs::new("t")).unwrap();
    assert_eq!(result, "\"gcc -c \" .. _cook_in .. \" -o \" .. _cook_out");
}

#[test]
fn test_expand_template_stem_in_path() {
    // CS-0033: bare $<stem> has no special meaning; falls through to env runtime.
    use std::collections::BTreeSet;
    use crate::resolver::{IterMode, OutputShape, ResolveCtx};
    use crate::template::{ConsultedEnv, expand_sigil_template};
    let r = BTreeSet::new();
    let ctx = ResolveCtx { mode: IterMode::OneShot, outputs: OutputShape::None, recipes_in_scope: &r };
    let mut env = ConsultedEnv::new();
    let result = expand_sigil_template("build/$<stem>.o", &ctx, &mut env, &mut crate::template::FileRefs::new("t")).unwrap();
    assert_eq!(result, "\"build/\" .. cook.require_env(\"stem\") .. \".o\"");
}

#[test]
fn test_expand_template_in_stem_in_path() {
    // CS-0033 form: $<in.stem> expands to path.stem(_cook_in)
    use std::collections::BTreeSet;
    use crate::resolver::{IterMode, OutputShape, ResolveCtx};
    use crate::template::{ConsultedEnv, expand_sigil_template};
    let r = BTreeSet::new();
    let ctx = ResolveCtx { mode: IterMode::OneToOne, outputs: OutputShape::Single, recipes_in_scope: &r };
    let mut env = ConsultedEnv::new();
    let result = expand_sigil_template("build/$<in.stem>.o", &ctx, &mut env, &mut crate::template::FileRefs::new("t")).unwrap();
    assert_eq!(result, "\"build/\" .. path.stem(_cook_in) .. \".o\"");
}

#[test]
fn test_expand_template_all() {
    use std::collections::BTreeSet;
    use crate::resolver::{IterMode, OutputShape, ResolveCtx};
    use crate::template::{ConsultedEnv, expand_sigil_template};
    let r = BTreeSet::new();
    let ctx = ResolveCtx { mode: IterMode::ManyToOne, outputs: OutputShape::Single, recipes_in_scope: &r };
    let mut env = ConsultedEnv::new();
    let result = expand_sigil_template("ar rcs $<out> $<all>", &ctx, &mut env, &mut crate::template::FileRefs::new("t")).unwrap();
    assert_eq!(result, "\"ar rcs \" .. _cook_out .. \" \" .. _cook_all");
}

#[test]
fn test_minimal_recipe() {
    // §{recipes.body-bundling}: a non-interactive shell line bundles into
    // one body unit whose lua_code calls cook.sh with `set -e\n<line>`.
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
    // Surface recipes lower to `cook.__register_surface` (CS-0077 codegen).
    assert!(output.contains("cook.__register_surface(\"build\""));
    assert!(
        output.contains("cook.add_unit({lua_code = ") && output.contains("cook.sh(") && output.contains("set -e\necho hello"),
        "Single shell line should produce one body unit calling cook.sh, got:\n{output}"
    );
    assert!(!output.contains("cook.layer"), "Shell steps should not use cook.layer");
    assert!(!output.contains("cook.exec"), "Shell steps should not use cook.exec");
}

#[test]
fn codegen_emits_register_surface_for_surface_recipes() {
    // SHI-222 Phase 3 Task 3.1: surface `recipe NAME` blocks must lower to
    // `cook.__register_surface(name, meta, body)` with `__line = N` carrying
    // the source line — distinct from `cook.recipe(...)` so collision
    // detection can tag the registration kind correctly.
    let cookfile = Cookfile {
        config_blocks: vec![],
        recipes: vec![Recipe {
            name: "build".to_string(),
            deps: vec![],
            ingredients: vec![],
            excludes: vec![],
            steps: vec![],
            line: 5,
        }],
        chores: vec![],
        uses: vec![],
        imports: vec![],
        register_blocks: vec![],
        top_level_module_calls: vec![],
        probes: vec![],
    };
    let lua = generate(&cookfile);
    assert!(
        lua.contains(r#"cook.__register_surface("build""#),
        "expected cook.__register_surface for surface recipe, got:\n{lua}"
    );
    assert!(
        lua.contains("__line = 5"),
        "expected __line = 5 in metadata, got:\n{lua}"
    );
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
                outputs: vec![OutputPattern::Quoted("build/$<in.stem>.o".to_string())],
                body: Some(Body::ShellBlock(
                    vec!["gcc -c $<in> -o $<out>".to_string()],
                )),
                disposition: Default::default(),
            },
            line: 3,
        }],
    )]);
    let output = generate(&cookfile);
    assert!(output.contains("cook.step_group(function()"), "missing cook.step_group");
    assert!(output.contains("local _cook_outputs_1 = {}"));
    // Iteration source is the flat resolved set (Standard §4.3 union),
    // emitted as the local `ingredients` by `recipe.rs`. Reading
    // `recipe.ingredients[1]` here would silently drop every glob past
    // the first.
    assert!(output.contains("for _, _cook_in in ipairs(ingredients) do"));
    // CS-0022: output pattern $<in.stem> expands directly to path.stem(_cook_in)
    assert!(output.contains("local _cook_out = \"build/\" .. path.stem(_cook_in) .. \".o\""),
        "output pattern should expand $<in.stem> to path.stem(_cook_in), got:\n{output}");
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
                    outputs: vec![OutputPattern::Quoted("build/$<in.stem>.o".to_string())],
                    body: Some(Body::ShellBlock(
                        vec!["gcc -c $<in> -o $<out>".to_string()],
                    )),
                    disposition: Default::default(),
                },
                line: 3,
            },
            Step::Cook {
                step: CookStep {
                    outputs: vec![OutputPattern::Quoted("build/lib.a".to_string())],
                    body: Some(Body::ShellBlock(
                        vec!["ar rcs $<out> $<all>".to_string()],
                    )),
                    disposition: Default::default(),
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
                outputs: vec![OutputPattern::Quoted("bin/app".to_string())],
                body: None,
                disposition: Default::default(),
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
                outputs: vec![OutputPattern::Quoted("build/{in.stem}.o".to_string())],
                body: Some(Body::LuaBlock(
                    "cook.sh(\"gcc -c \" .. input .. \" -o \" .. output)".to_string(),
                )),
                disposition: Default::default(),
            },
            line: 3,
        }],
    )]);
    let output = generate(&cookfile);
    assert!(
        output.contains("lua_code ="),
        "missing cook.add_unit with lua_code, got:\n{output}"
    );
    assert!(
        output.contains("ingredient_groups = {recipe.ingredients[1]}"),
        "missing ingredient_groups, got:\n{output}"
    );
    assert!(output.contains("cook.sh(\"gcc -c \" .. input .. \" -o \" .. output)"));
    assert!(!output.contains("lua = function()"), "should not emit lua = function(), got:\n{output}");
    // Should NOT have old API
    assert!(!output.contains("cook.layer"), "should not use cook.layer");
}

#[test]
fn test_plate_step() {
    // CS-0024: plate body uses $<in> (OneToOne mode) — iterates over prior cook outputs.
    let cookfile = make_cookfile(vec![make_recipe(
        "run",
        vec![],
        vec![],
        vec![
            Step::Cook {
                step: CookStep {
                    outputs: vec![OutputPattern::Quoted("bin/$<in.stem>".to_string())],
                    body: None,
                    disposition: Default::default(),
                },
                line: 2,
            },
            Step::Plate {
                step: PlateStep {
                    body: Body::ShellBlock(vec!["./$<in>".to_string()]),
                },
                line: 3,
            },
        ],
    )]);
    let output = generate(&cookfile);
    assert!(output.contains("cook.step_group(function()"), "missing cook.step_group for plate");
    assert!(output.contains("for _, _plate_in in ipairs(_cook_outputs_1) do"),
        "expected _plate_in iteration, got:\n{output}");
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
    // Step::Lua is execute-phase per §{recipes.lua-steps}; it goes into
    // a body unit's lua_code payload, NOT inlined as raw register-phase Lua.
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
    assert!(
        output.contains("cook.add_unit({lua_code = ") && output.contains("print(\"hello\")"),
        "execute-phase `>` line should appear inside cook.add_unit's lua_code payload, got:\n{output}"
    );
    assert!(!output.contains("cook.exec"));
}

#[test]
fn test_inline_lua_line_inlined() {
    // Step::InlineLua is register-phase per §{recipes.lua-steps}; it's
    // inlined into the recipe-body Lua function, NOT wrapped in cook.add_unit.
    let cookfile = make_cookfile(vec![make_recipe(
        "test",
        vec![],
        vec![],
        vec![Step::InlineLua {
            code: "print(\"hello\")".to_string(),
            line: 2,
        }],
    )]);
    let output = generate(&cookfile);
    assert!(output.contains("    print(\"hello\")"), "got:\n{output}");
    assert!(!output.contains("cook.add_unit"), "got:\n{output}");
}

#[test]
fn test_body_bundling_coalesces_shell_lines() {
    // Two consecutive shell lines coalesce into one cook.sh call inside
    // one body unit (§{recipes.body-bundling}).
    let cookfile = make_cookfile(vec![make_recipe(
        "smoke",
        vec![],
        vec![],
        vec![
            Step::Shell { command: "cd build".to_string(), line: 2, interactive: false },
            Step::Shell { command: "./app".to_string(), line: 3, interactive: false },
        ],
    )]);
    let output = generate(&cookfile);
    let cook_add_unit_count = output.matches("cook.add_unit").count();
    assert_eq!(
        cook_add_unit_count, 1,
        "two adjacent shell lines should coalesce into one body unit, got {cook_add_unit_count}:\n{output}"
    );
    assert!(output.contains("set -e\ncd build\n./app"), "got:\n{output}");
    assert!(output.contains("io.write(cook.sh("), "got:\n{output}");
}

#[test]
fn test_body_bundling_lua_breaks_shell_coalescence() {
    // A `>` line between two shell lines breaks the shell coalescence:
    // both shell calls live in the SAME body unit (one Lua VM), but as
    // two separate cook.sh calls (§{recipes.body-bundling}).
    let cookfile = make_cookfile(vec![make_recipe(
        "split",
        vec![],
        vec![],
        vec![
            Step::Shell { command: "echo a".to_string(), line: 2, interactive: false },
            Step::Lua { code: "local x = 1".to_string(), line: 3 },
            Step::Shell { command: "echo b".to_string(), line: 4, interactive: false },
        ],
    )]);
    let output = generate(&cookfile);
    assert_eq!(output.matches("cook.add_unit").count(), 1, "got:\n{output}");
    assert_eq!(output.matches("cook.sh(").count(), 2, "got:\n{output}");
}

#[test]
fn test_body_bundling_interactive_breaks_bundle() {
    // @interactive is its own draining unit; it breaks the body bundle.
    let cookfile = make_cookfile(vec![make_recipe(
        "demo",
        vec![],
        vec![],
        vec![
            Step::Shell { command: "echo before".to_string(), line: 2, interactive: false },
            Step::Shell { command: "vim x".to_string(), line: 3, interactive: true },
            Step::Shell { command: "echo after".to_string(), line: 4, interactive: false },
        ],
    )]);
    let output = generate(&cookfile);
    // 3 cook.add_unit: first body, interactive, second body
    assert_eq!(output.matches("cook.add_unit").count(), 3, "got:\n{output}");
    assert!(output.contains("interactive = true"), "got:\n{output}");
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
    // `]]` inside the shell command body still needs the [=[ ... ]=] long-bracket
    // wrap to round-trip safely. Under body bundling, the wrap appears around
    // the whole body unit's lua_code payload (which contains a nested cook.sh
    // call whose argument also long-brackets).
    assert!(output.contains("echo ]]"), "got:\n{output}");
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
                outputs: vec![OutputPattern::Quoted("build/$<in.stem>.o".to_string())],
                body: Some(Body::ShellBlock(
                    vec!["gcc -c $<in> -o $<out>".to_string()],
                )),
                disposition: Default::default(),
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
    // CS-0024: plate body is OneShot (no source placeholders) — still wrapped in step_group.
    let cookfile = make_cookfile(vec![make_recipe(
        "run",
        vec![],
        vec![],
        vec![
            Step::Cook {
                step: CookStep {
                    outputs: vec![OutputPattern::Quoted("bin/app".to_string())],
                    body: None,
                    disposition: Default::default(),
                },
                line: 2,
            },
            Step::Plate {
                step: PlateStep {
                    body: Body::ShellBlock(vec!["echo done".to_string()]),
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
                outputs: vec![OutputPattern::Quoted("build/$<in.stem>.o".to_string())],
                body: Some(Body::ShellBlock(
                    vec!["$<CC> $<CFLAGS> -c $<in> -o $<out>".to_string()],
                )),
                disposition: Default::default(),
            },
            line: 3,
        }],
    )]);
    let output = generate(&cookfile);
    assert!(output.contains("_cook_in"), "should expand $<in> to _cook_in");
    assert!(output.contains("_cook_out"), "should expand $<out> to _cook_out");
    assert!(
        output.contains(r#"cook.require_env("CC")"#),
        "should expand $<CC> to cook.require_env(\"CC\"), got: {}",
        output
    );
    assert!(
        output.contains(r#"cook.require_env("CFLAGS")"#),
        "should expand $<CFLAGS> to cook.require_env(\"CFLAGS\"), got: {}",
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
                outputs: vec![OutputPattern::Quoted("build/$<in.stem>.o".to_string())],
                body: Some(Body::ShellBlock(
                    vec!["$<CC> -c $<in> -o $<out>".to_string()],
                )),
                disposition: Default::default(),
            },
            line: 3,
        }],
    )]);
    let output = generate(&cookfile);
    assert!(output.contains(r#"cook.require_env("CC")"#));
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
                outputs: vec![OutputPattern::Quoted("build/$<in.stem>.o".to_string())],
                body: Some(Body::ShellBlock(
                    vec!["gcc -c $<in> -o $<out>".to_string()],
                )),
                disposition: Default::default(),
            },
            line: 3,
        }],
    )]);
    let output = generate(&cookfile);
    // CS-0022: shell block joined with "set -e\n" prefix; gcc command follows
    assert!(output.contains("gcc -c "), "should contain gcc -c command, got:\n{output}");
    assert!(!output.contains("cook.env"), "should not emit cook.env when no config vars");
    assert!(!output.contains("cook.require_env"), "should not emit cook.require_env when no env tokens");
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
                outputs: vec![OutputPattern::Quoted("build/{in.stem}.o".to_string())],
                body: Some(Body::LuaBlock(
                    "cook.sh(\"gcc -c \" .. input .. \" -o \" .. output)".to_string(),
                )),
                disposition: Default::default(),
            },
            line: 3,
        }],
    )]);
    let output = generate(&cookfile);
    // The emitted code must pass the user's Lua block through as a string
    // (`lua_code = [[...]]`) rather than a Lua function value, because
    // unit_api consumes `lua_code` and builds a WorkPayload::LuaChunk that
    // the worker pool executes against a fresh Lua state.
    assert!(
        output.contains("lua_code ="),
        "LuaBlock should emit lua_code = ..."
    );
    assert!(
        !output.contains("lua = function()"),
        "LuaBlock should not emit lua = function()"
    );
}

#[test]
fn test_use_generates_load_module() {
    let cookfile = Cookfile {
        config_blocks: vec![],
        recipes: vec![make_recipe("build", vec![], vec![], vec![
            Step::Shell { command: "echo hi".to_string(), line: 2, interactive: false },
        ])],
        chores: vec![],
        uses: vec![
            UseStatement { module_name: "cpp".to_string(), line: 1 },
        ],
        imports: vec![],
        register_blocks: vec![],
        top_level_module_calls: vec![],
        probes: vec![],
    };
    let output = generate(&cookfile);
    assert!(output.contains(r#"local cpp = cook.load_module("cpp")"#));
    let load_pos = output.find("cook.load_module").unwrap();
    // Surface recipes lower to `cook.__register_surface` (CS-0077 codegen).
    let recipe_pos = output.find("cook.__register_surface").unwrap();
    assert!(load_pos < recipe_pos);
}

#[test]
fn test_test_step_codegen() {
    // CS-0024: test body uses $<in> (OneToOne mode) — iterates over prior cook outputs.
    let cookfile = make_cookfile(vec![make_recipe(
        "run-tests",
        vec![],
        vec!["tests/*.c"],
        vec![
            Step::Cook {
                step: CookStep {
                    outputs: vec![OutputPattern::Quoted("build/$<in.stem>".to_string())],
                    body: Some(Body::ShellBlock(
                        vec!["cc $<in> -o $<out>".to_string()],
                    )),
                    disposition: Default::default(),
                },
                line: 3,
            },
            Step::Test {
                step: TestStep {
                    body: Body::ShellBlock(vec!["./$<in>".to_string()]),
                    as_name: None,
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
        output.contains("for _, _test_in in ipairs(_cook_outputs_1)"),
        "expected _test_in iteration in:\n{output}"
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
    // CS-0024: test body is OneShot (no source placeholders) — timeout and should_fail pass through.
    let cookfile = make_cookfile(vec![make_recipe(
        "run-tests",
        vec![],
        vec![],
        vec![Step::Test {
            step: TestStep {
                body: Body::ShellBlock(vec!["echo run-tests".to_string()]),
                as_name: None,
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
        config_blocks: vec![],
        recipes: vec![],
        chores: vec![],
        uses: vec![
            UseStatement { module_name: "cpp".to_string(), line: 1 },
            UseStatement { module_name: "proto".to_string(), line: 2 },
        ],
        imports: vec![],
        register_blocks: vec![],
        top_level_module_calls: vec![],
        probes: vec![],
    };
    let output = generate(&cookfile);
    let cpp_pos = output.find(r#"local cpp = cook.load_module("cpp")"#).unwrap();
    let proto_pos = output.find(r#"local proto = cook.load_module("proto")"#).unwrap();
    assert!(cpp_pos < proto_pos);
}

#[test]
fn test_no_hash_in_output() {
    // CS-0024: plate body uses $<in> (OneToOne). Old API used cook.layer with a hash argument.
    let cookfile = make_cookfile(vec![make_recipe(
        "build",
        vec![],
        vec!["src/*.c"],
        vec![
            Step::Cook {
                step: CookStep {
                    outputs: vec![OutputPattern::Quoted("build/$<in.stem>.o".to_string())],
                    body: Some(Body::ShellBlock(
                        vec!["gcc -c $<in> -o $<out>".to_string()],
                    )),
                    disposition: Default::default(),
                },
                line: 3,
            },
            Step::Plate {
                step: PlateStep {
                    body: Body::ShellBlock(vec!["./$<in>".to_string()]),
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
    // Shell steps live in a body unit, not a step group
    // (§{recipes.body-bundling}, §{exec.step-groups}).
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
        output.contains("cook.add_unit({lua_code = ") && output.contains("rm -rf build"),
        "shell step should be wrapped in a body unit's lua_code payload, got:\n{output}"
    );
    assert!(
        !output.contains("cook.step_group"),
        "body units are not wrapped in step_group, got:\n{output}"
    );
}

#[test]
fn test_test_step_wrapped_in_step_group() {
    // CS-0024: test body is OneShot — still wrapped in step_group.
    let cookfile = make_cookfile(vec![make_recipe(
        "test",
        vec![],
        vec![],
        vec![Step::Test {
            step: TestStep {
                body: Body::ShellBlock(vec!["echo run".to_string()]),
                as_name: None,
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
                    // CS-0022: one-to-one output pattern so body can use $<in>
                    outputs: vec![OutputPattern::Quoted("build/obj/$<in.stem>.o".to_string())],
                    body: Some(Body::ShellBlock(vec!["gcc -c $<in> -o $<out>".into()])),
                    disposition: Default::default(),
                },
                line: 3,
            },
            Step::Cook {
                step: CookStep {
                    // CS-0022: literal output → many-to-one; body uses $<all> not $<in>
                    outputs: vec![OutputPattern::Quoted("build/bin/app".to_string())],
                    body: Some(Body::ShellBlock(vec![
                        "gcc -o $<out> $<all> $<libmath> $<libstr>".into(),
                    ])),
                    disposition: Default::default(),
                },
                line: 4,
            },
        ],
    )]);
    let output = crate::generate_with_names(&cookfile, &names).expect("codegen");
    assert!(
        output.contains(r#"cook.dep_output("libmath")"#),
        "expected cook.dep_output for libmath, got:\n{output}"
    );
    assert!(
        output.contains(r#"cook.dep_output("libstr")"#),
        "expected cook.dep_output for libstr, got:\n{output}"
    );
    assert!(output.contains("_cook_in"), "built-in $<in> should still work");
    assert!(output.contains("_cook_out"), "built-in $<out> should still work");
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
                outputs: vec![OutputPattern::Quoted("build/$<in.stem>.o".to_string())],
                body: Some(Body::ShellBlock(vec!["$<CC> -c $<in> -o $<out>".into()])),
                disposition: Default::default(),
            },
            line: 3,
        }],
    )]);
    let output = crate::generate_with_names(&cookfile, &names).expect("codegen");
    assert!(
        output.contains(r#"cook.require_env("CC")"#),
        "CC is not a recipe name, should be env var via cook.require_env, got:\n{output}"
    );
}

#[test]
fn test_bare_shell_dep_ref_lowers_to_register_time_eval() {
    // Regression: Standard §5.5 requires `$<NAME>` (bare recipe ref) to
    // substitute in any bare `shell_command` body. Bare-shell bodies are
    // bundled into a `lua_code = ...` payload that the worker VM evaluates,
    // but the worker VM has no `cook.dep_output` (only the register VM does).
    // The fix: emit the resolved shell command via Lua-string concatenation
    // so the `cook.dep_output(...)` call runs at register time and the
    // worker only ever sees a literal `cook.sh("...")` argument.
    //
    // Pre-fix: `lua_code = [[io.write(cook.sh("set -e\n" .. "echo " .. cook.dep_output("greet")))]]`
    //   → worker crashes: `attempt to call a nil value (field 'dep_output')`.
    // Post-fix: `lua_code = "io.write(cook.sh(" .. string.format("%q", "set -e\n" .. ("echo " .. cook.dep_output("greet"))) .. "))\n"`
    //   → register VM resolves `cook.dep_output("greet")`, splices the result
    //   into a `%q`-quoted literal, sends a literal `cook.sh("...")` to worker.
    let names: std::collections::BTreeSet<String> =
        ["greet"].iter().map(|s| s.to_string()).collect();
    let cookfile = make_cookfile(vec![
        make_recipe(
            "greet",
            vec![],
            vec!["Cookfile"],
            vec![],
        ),
        make_recipe(
            "shout",
            vec!["greet"],
            vec![],
            vec![Step::Shell {
                command: "echo \"shout sees: $<greet>\"".to_string(),
                line: 6,
                interactive: false,
            }],
        ),
    ]);
    let output = crate::generate_with_names(&cookfile, &names).expect("codegen");
    // The lua_code value must be built by Lua-string concat (so cook.dep_output
    // runs at register time) — NOT a single long-string that ships the call to
    // the worker. Marker: `string.format("%q"` is the pre-quoting bridge.
    assert!(
        output.contains("string.format(\"%q\""),
        "bare-shell body with $<dep> must use string.format(%%q, ...) to bake \
         register-time-resolved command into worker chunk, got:\n{output}"
    );
    // The cook.dep_output call must appear OUTSIDE a long-string literal —
    // i.e. directly in the register-phase Lua source, not inside `[[ ... ]]`.
    let dep_call_pos = output
        .find(r#"cook.dep_output("greet")"#)
        .expect("expected cook.dep_output(\"greet\") call");
    let preceding = &output[..dep_call_pos];
    let opens: usize = preceding.matches("[[").count();
    let closes: usize = preceding.matches("]]").count();
    assert_eq!(
        opens, closes,
        "cook.dep_output must not be inside a [[ … ]] long string \
         (otherwise it ships to the worker VM where dep_output is not \
         registered), got:\n{output}"
    );
}

#[test]
fn test_bare_shell_env_ref_lowers_to_register_time_eval() {
    // Same regression as the dep-ref case for `cook.require_env`:
    // both helpers are register-VM-only.
    let names: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let cookfile = make_cookfile(vec![make_recipe(
        "shout",
        vec![],
        vec![],
        vec![Step::Shell {
            command: "echo \"home is: $<HOME>\"".to_string(),
            line: 2,
            interactive: false,
        }],
    )]);
    let output = crate::generate_with_names(&cookfile, &names).expect("codegen");
    let req_call_pos = output
        .find(r#"cook.require_env("HOME")"#)
        .expect("expected cook.require_env(\"HOME\") call");
    let preceding = &output[..req_call_pos];
    let opens: usize = preceding.matches("[[").count();
    let closes: usize = preceding.matches("]]").count();
    assert_eq!(
        opens, closes,
        "cook.require_env must not be inside a [[ … ]] long string in a bare \
         shell body — register VM is the only one with require_env, got:\n{output}"
    );
}

#[test]
fn test_bare_shell_no_sigil_keeps_long_string_shape() {
    // Pre-fix shape preservation: a bare shell with no sigil placeholders
    // continues to emit as `lua_code = [[ io.write(cook.sh([[...]])) ]]`
    // (a single long-string literal) so existing snapshots / fixtures
    // remain byte-stable for the common case.
    let names: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let cookfile = make_cookfile(vec![make_recipe(
        "shout",
        vec![],
        vec![],
        vec![Step::Shell {
            command: "echo hello".to_string(),
            line: 2,
            interactive: false,
        }],
    )]);
    let output = crate::generate_with_names(&cookfile, &names).expect("codegen");
    // Match `lua_code = [` followed by zero-or-more `=` and another `[` —
    // i.e., a Lua long-string literal at any bracket level (`[[`, `[=[`, `[==[`, …).
    let has_long_string_lua_code = output
        .split("lua_code = ")
        .skip(1)
        .any(|s| {
            let bytes = s.as_bytes();
            if bytes.first() != Some(&b'[') {
                return false;
            }
            let mut i = 1usize;
            while i < bytes.len() && bytes[i] == b'=' {
                i += 1;
            }
            i < bytes.len() && bytes[i] == b'['
        });
    assert!(
        has_long_string_lua_code,
        "no-sigil bare shell should still use a single long-string lua_code, got:\n{output}"
    );
    assert!(
        !output.contains("string.format(\"%q\""),
        "no-sigil bare shell should NOT bring in the register-time concat path, got:\n{output}"
    );
}

#[test]
fn test_dep_ref_in_plate_command() {
    // CS-0024: plate bodies may not reference {out}; use {app} dep-ref instead.
    let names: std::collections::BTreeSet<String> =
        ["app"].iter().map(|s| s.to_string()).collect();
    let cookfile = make_cookfile(vec![make_recipe(
        "run",
        vec![],
        vec![],
        vec![
            Step::Cook {
                step: CookStep {
                    outputs: vec![OutputPattern::Quoted("bin/runner".to_string())],
                    body: None,
                    disposition: Default::default(),
                },
                line: 2,
            },
            Step::Plate {
                step: PlateStep {
                    body: Body::ShellBlock(vec!["./$<app>".to_string()]),
                },
                line: 3,
            },
        ],
    )]);
    let output = crate::generate_with_names(&cookfile, &names).expect("codegen");
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
                outputs: vec![OutputPattern::Quoted("build/obj/$<protos.stem>.o".to_string())],
                body: Some(Body::ShellBlock(vec!["gcc -c $<in> -o $<out>".into()])),
                disposition: Default::default(),
            },
            line: 2,
        }],
    )]);
    let output = crate::generate_with_names(&cookfile, &names).expect("codegen");
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
                    outputs: vec![OutputPattern::Quoted("build/obj/$<protos.stem>.o".to_string())],
                    body: Some(Body::ShellBlock(vec!["gcc -c $<in> -o $<out>".into()])),
                    disposition: Default::default(),
                },
                line: 2,
            },
            Step::Cook {
                step: CookStep {
                    outputs: vec![OutputPattern::Quoted("build/lib/libprotos.a".to_string())],
                    body: Some(Body::ShellBlock(vec!["ar rcs $<out> $<all>".into()])),
                    disposition: Default::default(),
                },
                line: 3,
            },
        ],
    )]);
    let output = crate::generate_with_names(&cookfile, &names).expect("codegen");
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
                outputs: vec![OutputPattern::Quoted("build/obj/$<protos.stem>.o".to_string())],
                body: Some(Body::ShellBlock(vec![
                    "gcc -c $<in> -I$<core>/include -o $<out>".into(),
                ])),
                disposition: Default::default(),
            },
            line: 2,
        }],
    )]);
    let output = crate::generate_with_names(&cookfile, &names).expect("codegen");
    // Iteration driven by protos
    assert!(output.contains(r#"cook.dep_output_list("protos")"#));
    // String substitution of core in command
    assert!(output.contains(r#"cook.dep_output("core")"#));
}

// ── Config block dispatcher emission ─────────────────────────────

#[test]
fn test_codegen_emits_unnamed_config_block() {
    let cookfile = Cookfile {
        config_blocks: vec![ConfigBlock {
            name: None,
            body: "env.CC = \"gcc\"".to_string(),
            line: 1,
        }],
        recipes: vec![],
        chores: vec![],
        uses: vec![],
        imports: vec![],
        register_blocks: vec![],
        top_level_module_calls: vec![],
        probes: vec![],
    };
    let out = generate(&cookfile);
    assert!(out.contains("function __cook_run_config_blocks"));
    assert!(out.contains("env.CC = \"gcc\""));
}

#[test]
fn test_codegen_emits_named_config_block() {
    let cookfile = Cookfile {
        config_blocks: vec![ConfigBlock {
            name: Some("release".to_string()),
            body: "env.CXXFLAGS = \"-O3\"".to_string(),
            line: 1,
        }],
        recipes: vec![],
        chores: vec![],
        uses: vec![],
        imports: vec![],
        register_blocks: vec![],
        top_level_module_calls: vec![],
        probes: vec![],
    };
    let out = generate(&cookfile);
    assert!(out.contains("function __cook_run_config_blocks"));
    assert!(out.contains("selected_name == \"release\""));
    assert!(out.contains("env.CXXFLAGS = \"-O3\""));
}

#[test]
fn test_codegen_skips_dispatcher_when_no_config_blocks() {
    let cookfile = Cookfile {
        config_blocks: vec![],
        recipes: vec![],
        chores: vec![],
        uses: vec![],
        imports: vec![],
        register_blocks: vec![],
        top_level_module_calls: vec![],
        probes: vec![],
    };
    let out = generate(&cookfile);
    assert!(!out.contains("__cook_run_config_blocks"));
}

#[test]
fn test_codegen_emits_unnamed_and_named_in_order() {
    let cookfile = Cookfile {
        config_blocks: vec![
            ConfigBlock { name: None,                           body: "base()".into(), line: 1 },
            ConfigBlock { name: Some("dev".to_string()),        body: "dev()".into(),  line: 4 },
            ConfigBlock { name: Some("release".to_string()),    body: "rel()".into(),  line: 7 },
        ],
        recipes: vec![],
        chores: vec![],
        uses: vec![],
        imports: vec![],
        register_blocks: vec![],
        top_level_module_calls: vec![],
        probes: vec![],
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
                    outputs: vec![OutputPattern::Quoted("build/obj/math/$<in.stem>.o".into())],
                    body: Some(Body::ShellBlock(vec!["gcc -c $<in> -o $<out>".into()])),
                    disposition: Default::default(),
                },
                line: 3,
            },
            Step::Cook {
                step: CookStep {
                    outputs: vec![OutputPattern::Quoted("build/lib/libmath.a".into())],
                    body: Some(Body::ShellBlock(vec!["ar rcs $<out> $<all>".into()])),
                    disposition: Default::default(),
                },
                line: 4,
            },
        ]),
        make_recipe("libstr", vec![], vec!["src/str/*.c"], vec![
            Step::Cook {
                step: CookStep {
                    outputs: vec![OutputPattern::Quoted("build/obj/str/$<in.stem>.o".into())],
                    body: Some(Body::ShellBlock(vec!["gcc -c $<in> -o $<out>".into()])),
                    disposition: Default::default(),
                },
                line: 8,
            },
            Step::Cook {
                step: CookStep {
                    outputs: vec![OutputPattern::Quoted("build/lib/libstr.a".into())],
                    body: Some(Body::ShellBlock(vec!["ar rcs $<out> $<all>".into()])),
                    disposition: Default::default(),
                },
                line: 9,
            },
        ]),
        make_recipe("app", vec![], vec!["src/main.c"], vec![
            Step::Cook {
                step: CookStep {
                    // CS-0022: literal output → many-to-one; body uses $<all>
                    outputs: vec![OutputPattern::Quoted("build/obj/main.o".into())],
                    body: Some(Body::ShellBlock(vec!["gcc -c $<all> -o $<out>".into()])),
                    disposition: Default::default(),
                },
                line: 13,
            },
            Step::Cook {
                step: CookStep {
                    outputs: vec![OutputPattern::Quoted("build/bin/app".into())],
                    body: Some(Body::ShellBlock(vec![
                        "gcc -o $<out> $<all> $<libmath> $<libstr>".into(),
                    ])),
                    disposition: Default::default(),
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
    let lua = crate::generate_with_names(&cookfile, &names).expect("codegen");
    assert!(lua.contains(r#"cook.dep_output("libmath")"#), "missing dep_output for libmath");
    assert!(lua.contains(r#"cook.dep_output("libstr")"#), "missing dep_output for libstr");

    // libmath recipe should NOT have dep_output calls (it has no deps).
    // Surface recipes lower to `cook.__register_surface` (CS-0077 codegen).
    let libmath_section = lua.split("cook.__register_surface(\"libmath\"").nth(1).unwrap();
    let libmath_end = libmath_section
        .find("cook.__register_surface(")
        .unwrap_or(libmath_section.len());
    let libmath_lua = &libmath_section[..libmath_end];
    assert!(!libmath_lua.contains("cook.dep_output"),
        "libmath should have no dep_output calls");
}

// ── BlockStep codegen (multi-output) ─────────────────────────────

#[test]
fn blockstep_shell_multi_output() {
    let source = r#"recipe "wasm"
    ingredients "src/*.rs"
    cook "a.js" "b.wasm" {
        wasm-pack build
        cp x a.js
        cp y b.wasm
    }
end
"#;
    let cookfile = cook_lang::parse(source).expect("parse");
    let lua = crate::generate(&cookfile);
    // Outputs table:
    assert!(lua.contains(r#"_cook_outs = {"a.js", "b.wasm"}"#), "missing outs table: {lua}");
    // Single add_unit call with all three commands joined, fail-fast via set -e:
    assert!(lua.contains(r#"command = "set -e\nwasm-pack build\ncp x a.js\ncp y b.wasm""#)
        || lua.contains(r#"command = "set -e\\nwasm-pack build\\ncp x a.js\\ncp y b.wasm""#),
        "generated Lua missing expected shell command: {lua}");
    // Should not iterate per input:
    let for_count = lua.matches("for _, _cook_in in ipairs").count();
    assert_eq!(for_count, 0, "BlockStep should not emit a per-input loop");
    // Should not emit a Lua function body for the shell block:
    assert!(!lua.contains("lua = function()"), "ShellBlock should not emit lua = function(): {lua}");
}

#[test]
fn blockstep_lua_multi_output() {
    let source = r#"recipe "wasm"
    ingredients "src/*.rs"
    cook "a.js" "b.wasm" >{
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
    // Must emit lua_code = ... so the worker can execute the code body.
    // Emitting `lua = function()` silently drops the code since unit_api
    // does not consume Lua function values.
    assert!(
        !lua.contains("lua = function()"),
        "BlockStep+LuaBlock must not emit lua = function(); got:\n{lua}"
    );
    assert!(
        lua.contains("lua_code ="),
        "BlockStep+LuaBlock must emit lua_code = ...; got:\n{lua}"
    );
    assert!(
        lua.contains("ingredient_groups ="),
        "BlockStep+LuaBlock must emit ingredient_groups = ...; got:\n{lua}"
    );
}

#[test]
fn onetoone_lua_emits_lua_code_not_function() {
    // CS-0022: use $<in.stem> in output pattern to trigger one-to-one mode.
    let source = r#"recipe "lib"
    ingredients "lib/*.c"
    cook "build/obj/$<in.stem>.o" >{
        sh("gcc -c " .. input .. " -o " .. output)
    }
end
"#;
    let cookfile = cook_lang::parse(source).expect("parse");
    let lua = crate::generate(&cookfile);
    assert!(
        !lua.contains("lua = function()"),
        "OneToOne+LuaBlock must not emit lua = function(); got:\n{lua}"
    );
    assert!(
        lua.contains("lua_code ="),
        "OneToOne+LuaBlock must emit lua_code = ...; got:\n{lua}"
    );
    assert!(
        lua.contains("ingredient_groups ="),
        "OneToOne+LuaBlock must emit ingredient_groups = ...; got:\n{lua}"
    );
}

// Removed by SHI-222 Phase 4 Task 4.4: `test_dep_ref_wave_grouping_integration`
// pinned the legacy `cook_engine::wave_grouper::compute_waves` shape, which is
// gone now that the engine walks a single unified work-unit DAG (no waves at
// runtime). The codegen-side property the test was reaching for — that
// `{dep}` references and `: dep` declarations surface distinct dep maps so the
// engine can wire the right edges — is covered by:
//   * `recipe_dep_ref_emits_inferred_deps` and related tests above
//   * `cook-engine/tests/unified_dag_build.rs` (cross-recipe edges across the
//     unified DAG).

// ── CS-0009: empty-output warning + accessor-placement validation ──

#[test]
fn test_empty_output_reference_warns_not_errors() {
    // A recipe with no steps and no ingredients has an empty output list.
    // A name reference to such a recipe MUST warn at registration and expand
    // to empty, not error.
    let names: std::collections::BTreeSet<String> =
        ["empty_recipe"].iter().map(|s| s.to_string()).collect();
    let cookfile = make_cookfile(vec![
        make_recipe("empty_recipe", vec![], vec![], vec![]),
        make_recipe(
            "consumer",
            vec![],
            vec![],
            vec![Step::Cook {
                step: CookStep {
                    outputs: vec![OutputPattern::Quoted("build/out".to_string())],
                    body: Some(Body::ShellBlock(vec![
                        "echo $<empty_recipe> > $<out>".into(),
                    ])),
                    disposition: Default::default(),
                },
                line: 2,
            }],
        ),
    ]);
    let (output, warnings) =
        crate::generate_with_names_and_warnings(&cookfile, &names);
    assert!(!warnings.is_empty(), "expected empty-output warning");
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("empty_recipe") && w.contains("consumer")),
        "warning should name both referent and referrer, got: {:?}",
        warnings
    );
    assert!(
        output.contains(r#"cook.dep_output("empty_recipe")"#),
        "lowering should still produce the call, not elide it"
    );
}

#[test]
fn test_accessor_placeholder_in_using_string_without_driver_is_rejected() {
    let names: std::collections::BTreeSet<String> =
        ["protos"].iter().map(|s| s.to_string()).collect();
    let cookfile = make_cookfile(vec![
        make_recipe("protos", vec![], vec![], vec![]),
        make_recipe(
            "bad",
            vec![],
            vec![],
            vec![Step::Cook {
                step: CookStep {
                    outputs: vec![OutputPattern::Quoted("build/fixed.o".to_string())], // no accessor in output
                    body: Some(Body::ShellBlock(vec![
                        "echo $<protos.stem>".into(), // accessor in shell-block only
                    ])),
                    disposition: Default::default(),
                },
                line: 2,
            }],
        ),
    ]);
    let result = crate::generate_with_names_checked(&cookfile, &names);
    assert!(
        result.is_err(),
        "accessor placeholder in shell-block without matching driver must error"
    );
    let err = result.unwrap_err();
    assert!(
        err.to_string().contains("protos"),
        "error should name the accessor ref, got: {}",
        err
    );
}

#[test]
fn test_accessor_placeholder_in_plate_command_rejected() {
    let names: std::collections::BTreeSet<String> =
        ["protos"].iter().map(|s| s.to_string()).collect();
    let cookfile = make_cookfile(vec![
        make_recipe("protos", vec![], vec![], vec![]),
        make_recipe(
            "bad",
            vec![],
            vec![],
            vec![
                Step::Cook {
                    step: CookStep {
                        outputs: vec![OutputPattern::Quoted("bin/app".to_string())],
                        body: None,
                        disposition: Default::default(),
                    },
                    line: 2,
                },
                Step::Plate {
                    step: PlateStep {
                        body: Body::ShellBlock(vec!["./$<out> $<protos.stem>".to_string()]),
                    },
                    line: 3,
                },
            ],
        ),
    ]);
    let result = crate::generate_with_names_checked(&cookfile, &names);
    assert!(
        result.is_err(),
        "accessor placeholder in plate command must error"
    );
}

#[test]
fn test_accessor_placeholder_in_test_command_rejected() {
    let names: std::collections::BTreeSet<String> =
        ["protos"].iter().map(|s| s.to_string()).collect();
    let cookfile = make_cookfile(vec![
        make_recipe("protos", vec![], vec![], vec![]),
        make_recipe(
            "bad",
            vec![],
            vec![],
            vec![Step::Test {
                step: TestStep {
                    body: Body::ShellBlock(vec!["echo $<protos.stem>".to_string()]),
                    as_name: None,
                    timeout: None,
                    should_fail: false,
                },
                line: 2,
            }],
        ),
    ]);
    let result = crate::generate_with_names_checked(&cookfile, &names);
    assert!(
        result.is_err(),
        "accessor placeholder in test command must error"
    );
}

#[test]
fn test_accessor_placeholder_in_bare_shell_rejected() {
    let names: std::collections::BTreeSet<String> =
        ["protos"].iter().map(|s| s.to_string()).collect();
    let cookfile = make_cookfile(vec![
        make_recipe("protos", vec![], vec![], vec![]),
        make_recipe(
            "bad",
            vec![],
            vec![],
            vec![Step::Shell {
                command: "echo $<protos.stem>".to_string(),
                line: 2,
                interactive: false,
            }],
        ),
    ]);
    let result = crate::generate_with_names_checked(&cookfile, &names);
    assert!(
        result.is_err(),
        "accessor placeholder in bare shell must error"
    );
}

#[test]
fn test_accessor_placeholder_with_driver_in_output_pattern_ok() {
    // CS-0022: $<protos.stem> in the output pattern is valid (driver declaration).
    // $<protos.stem> in the shell-block body is REJECTED per CS-0022 §6.7 Note 6.7.2
    // ("The $<lib.ACCESSOR> form is rejected inside any cook-step body").
    // The correct form is $<in.stem> in the body.
    let names: std::collections::BTreeSet<String> =
        ["protos"].iter().map(|s| s.to_string()).collect();

    // Valid: $<protos.stem> only in output pattern, body uses $<in>/$<out>
    let cookfile_ok = make_cookfile(vec![
        make_recipe("protos", vec![], vec![], vec![]),
        make_recipe(
            "compile",
            vec![],
            vec![],
            vec![Step::Cook {
                step: CookStep {
                    outputs: vec![OutputPattern::Quoted("build/$<protos.stem>.o".to_string())],
                    body: Some(Body::ShellBlock(vec![
                        "gcc -c $<in> -o $<out>".into(),
                    ])),
                    disposition: Default::default(),
                },
                line: 2,
            }],
        ),
    ]);
    let result = crate::generate_with_names_checked(&cookfile_ok, &names);
    assert!(
        result.is_ok(),
        "dep-driven output pattern with valid body should pass, got: {:?}",
        result.err()
    );
}

// ── Chore codegen tests (CS-0020 / §4a) ──────────────────────────

fn make_chore(name: &str, deps: Vec<&str>, steps: Vec<Step>) -> Chore {
    Chore {
        name: name.to_string(),
        params: vec![],
        deps: deps.into_iter().map(String::from).collect(),
        steps,
        line: 1,
    }
}

#[test]
fn test_compile_chore_resolves_recipe_ref_to_dep_output() {
    // §10.2 step 2: a `$<recipe>` reference in a chore body (e.g. a `play`
    // chore launching a just-built binary via `$<engine>`) MUST resolve to
    // that recipe's output via `cook.dep_output`, creating the cross-recipe
    // edge — not fall through to `cook.require_env` (the bug).
    let chore = make_chore(
        "play",
        vec!["engine"],
        vec![Step::Shell {
            command: "$<engine> --run".to_string(),
            line: 2,
            interactive: true,
        }],
    );
    let mut names = std::collections::BTreeSet::new();
    names.insert("engine".to_string());
    let lua = compile_chore(&chore, &[], &names);
    assert!(
        lua.contains("cook.dep_output(\"engine\")"),
        "chore $<engine> must lower to cook.dep_output, got:\n{lua}"
    );
    assert!(
        !lua.contains("cook.require_env(\"engine\")"),
        "chore $<engine> must NOT lower to require_env, got:\n{lua}"
    );
}

#[test]
fn test_compile_chore_basic_shell_interactive_cache_false() {
    // §{chores.no-caching}: every unit has cache = false.
    // §{exec.interactive-drain}: every shell step is interactive.
    let chore = make_chore(
        "clean",
        vec![],
        vec![Step::Shell {
            command: "rm -rf build".to_string(),
            line: 2,
            interactive: true,
        }],
    );
    let lua = compile_chore(&chore, &[], &std::collections::BTreeSet::new());
    // Surface chores lower to `cook.__register_surface_chore` (CS-0077 codegen).
    // The metadata table always carries `__line = N` even when the chore has
    // no deps, so the table is non-empty.
    assert!(
        lua.contains("cook.__register_surface_chore(\"clean\", {__line = 1}, function(__cook_params)"),
        "chore should register via __register_surface_chore, got:\n{lua}"
    );
    assert!(
        lua.contains("interactive = true"),
        "chore shell step must be interactive, got:\n{lua}"
    );
    assert!(
        lua.contains("cache = false"),
        "chore unit must have cache = false, got:\n{lua}"
    );
    assert!(lua.contains("rm -rf build"), "command missing, got:\n{lua}");
    // Each shell step is its own unit (no bundling across shells).
    assert_eq!(
        lua.matches("cook.add_unit").count(),
        1,
        "one shell step → one unit, got:\n{lua}"
    );
}

#[test]
fn test_compile_chore_multiple_shell_steps_not_bundled() {
    // All shells are interactive, so each is its own draining unit (no coalescing).
    let chore = make_chore(
        "setup",
        vec![],
        vec![
            Step::Shell { command: "mkdir -p dist".to_string(), line: 2, interactive: true },
            Step::Shell { command: "cp -r src dist/".to_string(), line: 3, interactive: true },
        ],
    );
    let lua = compile_chore(&chore, &[], &std::collections::BTreeSet::new());
    assert_eq!(
        lua.matches("cook.add_unit").count(),
        2,
        "two shell steps → two units (no coalescing for interactive steps), got:\n{lua}"
    );
    assert_eq!(
        lua.matches("interactive = true").count(),
        2,
        "both units must be interactive, got:\n{lua}"
    );
    // Both must have cache = false.
    assert_eq!(
        lua.matches("cache = false").count(),
        2,
        "both units must have cache = false, got:\n{lua}"
    );
}

#[test]
fn test_compile_chore_with_lua_step_cache_false() {
    // Execute-phase Lua steps in a chore compile to a body unit with
    // cache = false AND interactive = true (CS-0051: drain-thread execution).
    let chore = make_chore(
        "status",
        vec![],
        vec![Step::Lua {
            code: r#"print("hello")"#.to_string(),
            line: 2,
        }],
    );
    let lua = compile_chore(&chore, &[], &std::collections::BTreeSet::new());
    assert!(lua.contains(r#"print("hello")"#), "Lua code missing, got:\n{lua}");
    assert!(
        lua.contains("cache = false"),
        "chore Lua body unit must have cache = false, got:\n{lua}"
    );
    // Emitted as a body unit on the drain thread (CS-0051).
    assert!(lua.contains("lua_code ="), "Lua step should emit lua_code =, got:\n{lua}");
    assert!(
        lua.contains("interactive = true"),
        "chore Lua-bundle unit must be interactive (CS-0051), got:\n{lua}"
    );
}

#[test]
fn chore_lua_block_unit_emits_interactive_true() {
    // CS-0051: Lua steps in a chore body run on the drain thread (controlling
    // terminal), so the emitted body unit must carry `interactive = true`.
    // Uses `>` prefix for a Lua line.
    let cookfile_src = r#"
chore mychore
    @echo first
    > print("from lua")
"#;
    let lua = generate_lua_for_test(cookfile_src);
    // The chore should have exactly one Lua-bundle unit (lua_code = ...).
    let lua_unit_count = lua.matches("lua_code =").count();
    assert_eq!(
        lua_unit_count,
        1,
        "expected exactly one Lua-bundle unit (lua_code =), got:\n{lua}"
    );
    // That unit must carry interactive = true (CS-0051: drain-thread execution).
    // Note: cook.add_unit spans multiple lines when the Lua string uses [[ ]] syntax,
    // so we check the whole output rather than line-by-line.
    assert!(
        lua.contains("interactive = true"),
        "chore Lua-bundle unit must include `interactive = true`, got:\n{lua}"
    );
    assert!(
        lua.contains("cache = false"),
        "chore Lua-bundle unit must include `cache = false`, got:\n{lua}"
    );
    assert!(
        lua.contains(r#"print("from lua")"#),
        "chore Lua-bundle unit must include the Lua code, got:\n{lua}"
    );
}

#[test]
fn test_compile_chore_with_deps() {
    // Chore deps map to `requires` in the metadata table.
    let chore = make_chore(
        "play",
        vec!["build"],
        vec![Step::Shell {
            command: "echo done".to_string(),
            line: 2,
            interactive: true,
        }],
    );
    let lua = compile_chore(&chore, &[], &std::collections::BTreeSet::new());
    assert!(
        lua.contains(r#"requires = {"build"}"#),
        "chore deps should become requires, got:\n{lua}"
    );
}

#[test]
fn test_compile_chore_wraps_with_enter_exit() {
    // §{chores.no-caching}: codegen wraps body with _enter_chore/_exit_chore
    // so the runtime can enforce the cache=true ban.
    let chore = make_chore(
        "clean",
        vec![],
        vec![Step::Shell {
            command: "rm -rf .tmp".to_string(),
            line: 2,
            interactive: true,
        }],
    );
    let lua = compile_chore(&chore, &[], &std::collections::BTreeSet::new());
    assert!(lua.contains("cook._enter_chore()"), "missing _enter_chore, got:\n{lua}");
    assert!(lua.contains("cook._exit_chore()"), "missing _exit_chore, got:\n{lua}");
    // _enter_chore must appear before the add_unit call.
    let enter_pos = lua.find("cook._enter_chore()").unwrap();
    let unit_pos = lua.find("cook.add_unit(").unwrap();
    let exit_pos = lua.find("cook._exit_chore()").unwrap();
    assert!(enter_pos < unit_pos, "_enter_chore must come before add_unit");
    assert!(unit_pos < exit_pos, "add_unit must come before _exit_chore");
}

#[test]
fn test_generate_includes_chores() {
    // generate() must emit register-phase Lua for both recipes and chores.
    let cookfile = Cookfile {
        config_blocks: vec![],
        recipes: vec![make_recipe("build", vec![], vec![], vec![])],
        chores: vec![make_chore(
            "clean",
            vec![],
            vec![Step::Shell {
                command: "rm -rf build".to_string(),
                line: 2,
                interactive: true,
            }],
        )],
        uses: vec![],
        imports: vec![],
        register_blocks: vec![],
        top_level_module_calls: vec![],
        probes: vec![],
    };
    let lua = generate(&cookfile);
    // Surface recipes/chores lower to `cook.__register_surface[_chore]`
    // (CS-0077 codegen). The kind tag carries to `RegisteredRecipe.kind`
    // through the register-phase capture closures in cook-register.
    assert!(lua.contains("cook.__register_surface(\"build\""), "recipe missing, got:\n{lua}");
    assert!(lua.contains("cook.__register_surface_chore(\"clean\""), "chore missing, got:\n{lua}");
    // Chore section must have cache = false.
    let chore_section = lua.split("cook.__register_surface_chore(\"clean\"").nth(1).unwrap_or("");
    assert!(
        chore_section.contains("cache = false"),
        "chore section must have cache = false, got section:\n{chore_section}"
    );
}

// ── CS-0022 Phase G: placeholder validation returns error not panic ──

#[test]
fn cs_0022_validate_placeholders_returns_error_not_panic() {
    // $<out_1> in single-output step must return Err, not panic.
    // Use a one-to-one step (output has $<in.stem>) so $<in> is valid,
    // but $<out_1> is the only violation.
    let src = r#"recipe "build"
    ingredients "src/*.c"
    cook "build/$<in.stem>.o" {
        gcc -c $<in> -o $<out_1>
    }
end
"#;
    let cookfile = cook_lang::parse(src).expect("parse");
    let names = crate::dep_ref::extract_recipe_names(&cookfile);
    let result = crate::generate_with_names_checked(&cookfile, &names);
    assert!(
        result.is_err(),
        "{{out_1}} in single-output step must error, not panic"
    );
    let err_str = result.unwrap_err().to_string();
    assert!(
        err_str.contains("CS-0022"),
        "error must contain CS-0022, got: {err_str}"
    );
    assert!(
        err_str.contains("out_1"),
        "error must name the bad placeholder, got: {err_str}"
    );
}

#[test]
fn cs_0022_validate_bare_in_in_many_to_one_returns_error() {
    // $<in> in a many-to-one (literal output) step must error.
    let src = r#"recipe "build"
    ingredients "src/*.c"
    cook "build/app" {
        gcc $<in> -o $<out>
    }
end
"#;
    let cookfile = cook_lang::parse(src).expect("parse");
    let names = crate::dep_ref::extract_recipe_names(&cookfile);
    let result = crate::generate_with_names_checked(&cookfile, &names);
    assert!(
        result.is_err(),
        "$<in> in many-to-one step must error"
    );
    let err_str = result.unwrap_err().to_string();
    assert!(err_str.contains("CS-0022"), "error must contain CS-0022, got: {err_str}");
}

#[test]
fn cs_0022_bare_stem_in_output_pattern_returns_error() {
    // $<stem> in output pattern must be rejected per CS-0022 §6.7.
    let src = r#"recipe "build"
    ingredients "src/*.c"
    cook "build/$<stem>.o" {
        gcc -c $<in> -o $<out>
    }
end
"#;
    let cookfile = cook_lang::parse(src).expect("parse");
    let names = crate::dep_ref::extract_recipe_names(&cookfile);
    let result = crate::generate_with_names_checked(&cookfile, &names);
    assert!(
        result.is_err(),
        "bare $<stem> in output pattern must error"
    );
    let err_str = result.unwrap_err().to_string();
    assert!(
        err_str.contains("CS-0022"),
        "error must contain CS-0022, got: {err_str}"
    );
    assert!(
        err_str.contains("stem"),
        "error must name 'stem', got: {err_str}"
    );
}

#[test]
fn cs_0022_lib_accessor_in_body_returns_error() {
    // $<libmath.dir> in a cook-step body must error per CS-0022 §6.7.
    let src = r#"recipe "libmath"
    ingredients "src/math/*.c"

recipe "build"
    ingredients "src/*.c"
    cook "build/$<in.stem>.o" {
        gcc -c $<in> -o $<out> -L $<libmath.dir>
    }
end
"#;
    let cookfile = cook_lang::parse(src).expect("parse");
    let names = crate::dep_ref::extract_recipe_names(&cookfile);
    let result = crate::generate_with_names_checked(&cookfile, &names);
    assert!(
        result.is_err(),
        "$<libmath.dir> in a cook-step body must error"
    );
    let err_str = result.unwrap_err().to_string();
    assert!(
        err_str.contains("CS-0022") || err_str.contains("libmath"),
        "error must mention libmath, got: {err_str}"
    );
}

#[test]
fn cs_0022_out_bare_in_multi_output_returns_error() {
    // $<out> in multi-output step must error.
    let src = r#"recipe "build"
    cook "a.js" "a.wasm" {
        gen --js $<out>
    }
end
"#;
    let cookfile = cook_lang::parse(src).expect("parse");
    let names = crate::dep_ref::extract_recipe_names(&cookfile);
    let result = crate::generate_with_names_checked(&cookfile, &names);
    assert!(result.is_err(), "$<out> in multi-output step must error");
    let err_str = result.unwrap_err().to_string();
    assert!(err_str.contains("CS-0022"), "error must contain CS-0022, got: {err_str}");
}

#[test]
fn cs_0022_multi_output_mixed_drivers_returns_error() {
    // A cook step with mixed iteration drivers must error.
    let src = r#"recipe "libmath"
    ingredients "src/math/*.c"

recipe "build"
    ingredients "src/*.c"
    cook "$<in.stem>.o" "$<libmath.stem>.bin" {
        do-stuff
    }
end
"#;
    let cookfile = cook_lang::parse(src).expect("parse");
    let names = crate::dep_ref::extract_recipe_names(&cookfile);
    let result = crate::generate_with_names_checked(&cookfile, &names);
    assert!(result.is_err(), "mixed iteration drivers must error");
    // The coherence error message mentions the patterns
    let err_str = result.unwrap_err().to_string();
    assert!(
        err_str.contains("CS-0022") || err_str.contains("driver"),
        "error must mention driver mismatch, got: {err_str}"
    );
}

// ── CS-0024: plate step six (mode × form) coverage ───────────────

#[test]
fn test_plate_step_shell_one_to_one() {
    let src = "recipe r\n    ingredients \"src/*.c\"\n    cook \"build/$<in.stem>\" { cc $<in> -o $<out> }\n    plate {\n        ./$<in>\n    }\n";
    let cookfile = cook_lang::parse(src).expect("parse");
    let lua = crate::generate(&cookfile);
    assert!(
        lua.contains("for _, _plate_in in ipairs(_cook_outputs_1)"),
        "expected one-to-one plate loop, got:\n{lua}"
    );
    assert!(lua.contains("cook.add_unit("), "expected cook.add_unit, got:\n{lua}");
    assert!(lua.contains("cache = false"), "expected cache=false, got:\n{lua}");
}

#[test]
fn test_plate_step_shell_many_to_one() {
    let src = "recipe r\n    ingredients \"src/*.c\"\n    cook \"build/$<in.stem>\" { cc $<in> -o $<out> }\n    plate { tar -czf bundle.tgz $<all> }\n";
    let cookfile = cook_lang::parse(src).expect("parse");
    let lua = crate::generate(&cookfile);
    assert!(
        !lua.contains("for _, _plate_in"),
        "many-to-one should not emit a loop, got:\n{lua}"
    );
    assert!(
        lua.contains("table.concat(_cook_outputs_1, \" \")"),
        "expected table.concat for $<all>, got:\n{lua}"
    );
    assert!(lua.contains("cook.add_unit("), "expected cook.add_unit, got:\n{lua}");
    assert!(lua.contains("cache = false"), "expected cache=false, got:\n{lua}");
}

#[test]
fn test_plate_step_shell_one_shot() {
    let src = "recipe r\n    plate { echo build complete }\n";
    let cookfile = cook_lang::parse(src).expect("parse");
    let lua = crate::generate(&cookfile);
    assert!(
        !lua.contains("for _, _plate_in"),
        "one-shot should not emit a loop, got:\n{lua}"
    );
    assert!(lua.contains("cook.add_unit("), "expected cook.add_unit, got:\n{lua}");
    assert!(lua.contains("cache = false"), "expected cache=false, got:\n{lua}");
}

#[test]
fn test_plate_step_lua_one_to_one() {
    let src = "recipe r\n    ingredients \"src/*.c\"\n    cook \"build/$<in.stem>\" { cc $<in> -o $<out> }\n    plate >{\n        cook.sh(\"strip \" .. input)\n    }\n";
    let cookfile = cook_lang::parse(src).expect("parse");
    let lua = crate::generate(&cookfile);
    assert!(
        lua.contains("for _, _plate_in in ipairs(_cook_outputs_1)"),
        "expected one-to-one plate loop, got:\n{lua}"
    );
    // Lua binding: `local input = ...` prepended at register time via string.format.
    assert!(
        lua.contains("local input = "),
        "expected 'local input = ' binding, got:\n{lua}"
    );
    assert!(lua.contains("lua_code ="), "expected lua_code field, got:\n{lua}");
    assert!(lua.contains("cache = false"), "expected cache=false, got:\n{lua}");
}

#[test]
fn test_plate_step_lua_many_to_one() {
    let src = "recipe r\n    ingredients \"src/*.c\"\n    cook \"build/$<in.stem>\" { cc $<in> -o $<out> }\n    plate >{\n        for _, b in ipairs(inputs) do cook.sh(\"strip \" .. b) end\n    }\n";
    let cookfile = cook_lang::parse(src).expect("parse");
    let lua = crate::generate(&cookfile);
    assert!(
        !lua.contains("for _, _plate_in in ipairs"),
        "many-to-one should not emit a _plate_in loop, got:\n{lua}"
    );
    // Lua binding: `local inputs = {...}` serialised at register time.
    assert!(
        lua.contains("local inputs = {"),
        "expected 'local inputs = {{...}}' binding, got:\n{lua}"
    );
    assert!(lua.contains("lua_code ="), "expected lua_code field, got:\n{lua}");
    assert!(lua.contains("cache = false"), "expected cache=false, got:\n{lua}");
}

#[test]
fn test_plate_step_lua_one_shot() {
    let src = "recipe r\n    plate >{\n        os.execute(\"echo done\")\n    }\n";
    let cookfile = cook_lang::parse(src).expect("parse");
    let lua = crate::generate(&cookfile);
    assert!(
        !lua.contains("for _, _plate_in"),
        "one-shot should not emit a loop, got:\n{lua}"
    );
    assert!(
        !lua.contains("local input = "),
        "one-shot should not emit input binding, got:\n{lua}"
    );
    assert!(lua.contains("lua_code ="), "expected lua_code field, got:\n{lua}");
    assert!(lua.contains("cache = false"), "expected cache=false, got:\n{lua}");
}

// ── CS-0024: test step six (mode × form) coverage ────────────────

#[test]
fn test_test_step_shell_one_to_one_with_modifiers() {
    let src = "recipe r\n    ingredients \"src/*.c\"\n    cook \"build/$<in.stem>\" { cc $<in> -o $<out> }\n    test { ./$<in> } timeout 60 should_fail\n";
    let cookfile = cook_lang::parse(src).expect("parse");
    let lua = crate::generate(&cookfile);
    assert!(
        lua.contains("for _, _test_in in ipairs(_cook_outputs_1)"),
        "expected one-to-one test loop, got:\n{lua}"
    );
    assert!(lua.contains("cook.add_test("), "expected cook.add_test, got:\n{lua}");
    assert!(lua.contains("timeout = 60"), "expected timeout = 60, got:\n{lua}");
    assert!(lua.contains("should_fail = true"), "expected should_fail = true, got:\n{lua}");
}

#[test]
fn test_test_step_shell_many_to_one() {
    let src = "recipe r\n    ingredients \"src/*.c\"\n    cook \"build/$<in.stem>\" { cc $<in> -o $<out> }\n    test { run-suite $<all> }\n";
    let cookfile = cook_lang::parse(src).expect("parse");
    let lua = crate::generate(&cookfile);
    assert!(
        !lua.contains("for _, _test_in"),
        "many-to-one should not emit a loop, got:\n{lua}"
    );
    assert!(
        lua.contains("table.concat(_cook_outputs_1, \" \")"),
        "expected table.concat for $<all>, got:\n{lua}"
    );
    assert!(lua.contains("cook.add_test("), "expected cook.add_test, got:\n{lua}");
    assert!(lua.contains("timeout = 300"), "expected default timeout 300, got:\n{lua}");
}

#[test]
fn test_test_step_shell_one_shot() {
    let src = "recipe r\n    test { echo smoke-test } timeout 10\n";
    let cookfile = cook_lang::parse(src).expect("parse");
    let lua = crate::generate(&cookfile);
    assert!(
        !lua.contains("for _, _test_in"),
        "one-shot should not emit a loop, got:\n{lua}"
    );
    assert!(lua.contains("cook.add_test("), "expected cook.add_test, got:\n{lua}");
    assert!(lua.contains("timeout = 10"), "expected timeout = 10, got:\n{lua}");
    assert!(lua.contains("should_fail = false"), "expected should_fail = false, got:\n{lua}");
}

#[test]
fn test_test_step_lua_one_to_one() {
    let src = "recipe r\n    ingredients \"src/*.c\"\n    cook \"build/$<in.stem>\" { cc $<in> -o $<out> }\n    test >{\n        cook.sh(\"./ \" .. input)\n    }\n";
    let cookfile = cook_lang::parse(src).expect("parse");
    let lua = crate::generate(&cookfile);
    assert!(
        lua.contains("for _, _test_in in ipairs(_cook_outputs_1)"),
        "expected one-to-one test loop, got:\n{lua}"
    );
    assert!(
        lua.contains("local input = "),
        "expected 'local input = ' binding, got:\n{lua}"
    );
    assert!(lua.contains("lua_code ="), "expected lua_code field, got:\n{lua}");
    assert!(lua.contains("timeout = 300"), "expected default timeout 300, got:\n{lua}");
}

#[test]
fn test_test_step_lua_many_to_one() {
    let src = "recipe r\n    ingredients \"src/*.c\"\n    cook \"build/$<in.stem>\" { cc $<in> -o $<out> }\n    test >{\n        for _, b in ipairs(inputs) do cook.sh(\"./ \" .. b) end\n    } timeout 120\n";
    let cookfile = cook_lang::parse(src).expect("parse");
    let lua = crate::generate(&cookfile);
    assert!(
        !lua.contains("for _, _test_in in ipairs"),
        "many-to-one should not emit a _test_in loop, got:\n{lua}"
    );
    assert!(
        lua.contains("local inputs = {"),
        "expected 'local inputs = {{...}}' binding, got:\n{lua}"
    );
    assert!(lua.contains("lua_code ="), "expected lua_code field, got:\n{lua}");
    assert!(lua.contains("timeout = 120"), "expected timeout = 120, got:\n{lua}");
}

#[test]
fn test_test_step_lua_one_shot() {
    let src = "recipe r\n    test >{\n        os.execute(\"echo smoke\")\n    } should_fail\n";
    let cookfile = cook_lang::parse(src).expect("parse");
    let lua = crate::generate(&cookfile);
    assert!(
        !lua.contains("for _, _test_in"),
        "one-shot should not emit a loop, got:\n{lua}"
    );
    assert!(
        !lua.contains("local input = "),
        "one-shot should not emit input binding, got:\n{lua}"
    );
    assert!(lua.contains("lua_code ="), "expected lua_code field, got:\n{lua}");
    assert!(lua.contains("should_fail = true"), "expected should_fail = true, got:\n{lua}");
}

// ── CS-0024: plate/test placeholder rejection tests ───────────────

#[test]
fn test_plate_out_rejected() {
    // $<out> is explicitly forbidden in plate bodies (CS-0024 firewall).
    let src = "recipe r\n    ingredients \"src/*.c\"\n    cook \"build/$<in.stem>\" { cc $<in> -o $<out> }\n    plate { ./$<out> }\n";
    let cookfile = cook_lang::parse(src).expect("parse");
    let names = crate::dep_ref::extract_recipe_names(&cookfile);
    let err = crate::generate_with_names(&cookfile, &names).unwrap_err();
    let msg = format!("{}", err);
    assert!(
        msg.contains("$<out>") || msg.contains("out"),
        "expected $<out> rejection, got: {msg}"
    );
}

#[test]
fn test_plate_mixed_in_and_all_rejected() {
    // Using both $<in> and $<all> in the same plate body is a mixed-mode error.
    let src = "recipe r\n    ingredients \"src/*.c\"\n    cook \"build/$<in.stem>\" { cc $<in> -o $<out> }\n    plate { echo $<in> $<all> }\n";
    let cookfile = cook_lang::parse(src).expect("parse");
    let names = crate::dep_ref::extract_recipe_names(&cookfile);
    let err = crate::generate_with_names(&cookfile, &names).unwrap_err();
    let msg = format!("{}", err);
    assert!(
        msg.contains("$<in>") && msg.contains("$<all>"),
        "expected mixed-mode rejection naming both $<in> and $<all>, got: {msg}"
    );
}

#[test]
fn test_plate_lua_mixed_input_and_inputs_rejected() {
    // Using both `input` and `inputs` in the same Lua plate body is rejected.
    let src = "recipe r\n    ingredients \"src/*.c\"\n    cook \"build/$<in.stem>\" { cc $<in> -o $<out> }\n    plate >{\n        print(input)\n        print(inputs[1])\n    }\n";
    let cookfile = cook_lang::parse(src).expect("parse");
    let names = crate::dep_ref::extract_recipe_names(&cookfile);
    let err = crate::generate_with_names(&cookfile, &names).unwrap_err();
    let msg = format!("{}", err);
    assert!(
        msg.contains("input") && msg.contains("inputs"),
        "expected mixed-binding rejection naming both 'input' and 'inputs', got: {msg}"
    );
}

#[test]
fn test_plate_bare_stem_rejected() {
    // Bare $<stem> in a plate body is rejected; use $<in.stem> instead.
    let src = "recipe r\n    ingredients \"src/*.c\"\n    cook \"build/$<in.stem>\" { cc $<in> -o $<out> }\n    plate { ./$<stem>.out }\n";
    let cookfile = cook_lang::parse(src).expect("parse");
    let names = crate::dep_ref::extract_recipe_names(&cookfile);
    let err = crate::generate_with_names(&cookfile, &names).unwrap_err();
    let msg = format!("{}", err);
    assert!(
        msg.contains("stem"),
        "expected bare-accessor rejection mentioning 'stem', got: {msg}"
    );
}

#[test]
fn test_plate_lib_accessor_rejected() {
    // $<lib.stem> in a plate body hits the §5.4 firewall — plate has no output pattern.
    let src = "recipe lib\n    ingredients \"x/*.c\"\n    cook \"build/$<in.stem>.o\" { cc -c $<in> -o $<out> }\nrecipe r: lib\n    cook \"build/app\" { cc $<lib> -o $<out> }\n    plate { echo $<lib.stem> }\n";
    let cookfile = cook_lang::parse(src).expect("parse");
    let names = crate::dep_ref::extract_recipe_names(&cookfile);
    let err = crate::generate_with_names(&cookfile, &names).unwrap_err();
    let msg = format!("{}", err);
    assert!(
        msg.contains("firewall") || msg.contains("lib"),
        "expected lib-accessor firewall rejection, got: {msg}"
    );
}

// ─── Task 4.2: consulted_env_keys emission tests ─────────────────────────────

fn generate_lua_for_test(cookfile_text: &str) -> String {
    let cookfile = cook_lang::parse(cookfile_text).expect("parse");
    crate::generate(&cookfile)
}

#[test]
fn cook_step_with_env_tokens_emits_consulted_env_keys() {
    // Recipe with `using { gcc $<CFLAGS> -c $<in> -o $<out> }` should emit
    // consulted_env_keys = {"CFLAGS"} (in/out are builtins, not env).
    let cookfile_text = r#"
recipe build
    ingredients "src/*.c"
    cook "build/$<in.stem>.o" { gcc $<CFLAGS> -c $<in> -o $<out> }
end
"#;
    let lua = generate_lua_for_test(cookfile_text);
    assert!(
        lua.contains("consulted_env_keys = {\"CFLAGS\"}"),
        "expected consulted_env_keys with CFLAGS, got:\n{lua}"
    );
}

#[test]
fn lua_block_step_with_no_env_reads_emits_empty_keyset() {
    // COOK-59 Task 4.5 / CS-0090: cook-step Lua using-blocks no longer emit
    // the `consulted_env_keys = "*"` sentinel. Instead, the codegen scans
    // the Lua body for `cook.env.<KEY>` reads (see `lua_env::scan_env_reads`)
    // and emits the matched keys as a literal Lua list. A body with no such
    // reads emits the empty list `{}` so the cache doesn't see any
    // synthetic environment dependency.
    let cookfile_text = r#"
recipe build
    ingredients "src/*.c"
    cook "build/{in.stem}.o" >{
        os.execute("gcc -c " .. input .. " -o " .. output)
    }
end
"#;
    let lua = generate_lua_for_test(cookfile_text);
    assert!(
        lua.contains("consulted_env_keys = {}"),
        "lua_block payload with no cook.env reads should emit empty keyset, got:\n{lua}"
    );
    assert!(
        !lua.contains("consulted_env_keys = \"*\""),
        "lua_block payload must not emit the legacy `*` sentinel:\n{lua}"
    );
}

#[test]
fn lua_block_step_records_static_cook_env_reads() {
    // COOK-59 Task 4.5 / CS-0090: a cook-step Lua using-block that reads
    // `cook.env.FOO` and `cook.env.BAR` MUST emit a sorted, deduplicated
    // list of those keys as `consulted_env_keys`.
    let cookfile_text = r#"
recipe touch
    ingredients "Cookfile"
    cook (input .. ".out") >{
        local f = io.open(output, "w")
        f:write("FOO=" .. tostring(cook.env.FOO))
        f:write("BAR=" .. tostring(cook.env.BAR))
        f:close()
    }
end
"#;
    let lua = generate_lua_for_test(cookfile_text);
    assert!(
        lua.contains("consulted_env_keys = {\"BAR\", \"FOO\"}"),
        "expected `consulted_env_keys = {{\"BAR\", \"FOO\"}}` (sorted), got:\n{lua}"
    );
}

// ─── Task 4 review: chore-body sigil regression tests (E.8 motivating case) ──

#[test]
fn chore_body_sigil_lowers_to_require_env() {
    // CS-0033 App. E.8: $<ADB> in a chore bare shell command must lower to
    // cook.require_env("ADB") and must not survive verbatim into the emitted Lua.
    let cookfile_text = r#"config
    env.ADB = "adb"

chore devices
    $<ADB> devices
"#;
    let lua = generate_lua_for_test(cookfile_text);
    assert!(
        lua.contains(r#"cook.require_env("ADB")"#),
        "expected cook.require_env(\"ADB\") in emitted lua; got:\n{}",
        lua
    );
    assert!(
        !lua.contains(r#"[[$<ADB>"#),
        "raw $<ADB> must not survive into command field; got:\n{}",
        lua
    );
}

#[test]
fn chore_body_passes_literal_braces_through() {
    // Brace-expansion forms and awk scripts that contain $1 / {print} must not
    // be misidentified as sigil placeholders (strict-bail rule: $< is the only
    // sigil prefix recognised by the lexer).
    let cookfile_text = r#"chore demo
    for i in {1..3}; do echo "$i"; done
    awk '{print $1}' file.txt
"#;
    let lua = generate_lua_for_test(cookfile_text);
    assert!(
        lua.contains("for i in {1..3}"),
        "literal {{1..3}} must survive; got:\n{}",
        lua
    );
    assert!(
        lua.contains("awk '{print $1}'"),
        "awk script must survive; got:\n{}",
        lua
    );
}

// ─── Task 2.5: as_name codegen tests ────────────────────────────────────────

#[test]
fn test_step_codegen_with_as_emits_name_field() {
    let cook_src = r#"
recipe r
    test { true } as 'my-test' timeout 5
"#;
    let lua = generate_lua_for_test(cook_src);
    assert!(
        lua.contains("name = \"my-test\""),
        "expected emitted Lua to set name; got:\n{lua}"
    );
}

#[test]
fn test_step_codegen_without_as_omits_name_field_or_uses_auto() {
    let cook_src = r#"
recipe r
    test { true } timeout 5
"#;
    let lua = generate_lua_for_test(cook_src);
    // Auto-name path: codegen MAY emit `name = "test#1"` or omit the field
    // entirely; both are valid per §3.2 (the runner generates the auto-name
    // if the field is absent or empty). We only assert no `as_name` was
    // forced through.
    assert!(!lua.contains("name = \"my-test\""));
}

#[test]
fn test_step_codegen_substitutes_as_name() {
    // The `as '$<in.stem>-rt'` modifier substitutes per CS-0033 at codegen.
    let cook_src = r#"
recipe r
    ingredients "src/*.txt"
    cook "build/$<in.stem>.out" { echo > $<out> }
    test { test -s $<in> } as '$<in.stem>-rt'
"#;
    let lua = generate_lua_for_test(cook_src);
    // The emitted Lua should contain a name expression that substitutes
    // through the existing iteration binding (`_test_in` or equivalent).
    // The exact name varies per emitter; assert the bare token doesn't
    // leak through unsubstituted.
    assert!(
        !lua.contains("name = \"$<in.stem>-rt\""),
        "as_name should be substituted, not literal:\n{lua}"
    );
}

// ── Standard §4.3: ingredients is union(includes) \ union(excludes) ──

#[test]
fn cook_step_iterates_union_of_all_include_globs() {
    // Regression for the silent-drop bug: with multiple include globs,
    // codegen used to hardcode `recipe.ingredients[1]` as the iteration
    // source, ignoring globs 2..N entirely. The fix routes through the
    // local `ingredients` (= cook.resolve_ingredients(...)) which is
    // the Standard-correct union.
    let cookfile = make_cookfile(vec![make_recipe(
        "build",
        vec![],
        vec!["src/*.c", "include/*.h"],
        vec![Step::Cook {
            step: CookStep {
                outputs: vec![OutputPattern::Quoted("build/$<in.stem>.o".to_string())],
                body: Some(Body::ShellBlock(
                    vec!["touch $<out>".to_string()],
                )),
                disposition: Default::default(),
            },
            line: 3,
        }],
    )]);
    let output = generate(&cookfile);
    // Iteration source is the merged local, NOT the per-pattern table.
    assert!(
        output.contains("for _, _cook_in in ipairs(ingredients) do"),
        "cook step must iterate the merged `ingredients` local, got:\n{output}"
    );
    assert!(
        !output.contains("ipairs(recipe.ingredients[1])"),
        "cook step must NOT iterate recipe.ingredients[1] (silently drops globs 2..N), got:\n{output}"
    );
    // The cook.resolve_ingredients call carries both globs.
    assert!(
        output.contains("cook.resolve_ingredients({\"src/*.c\", \"include/*.h\"}, {})"),
        "ingredients local must aggregate every include glob, got:\n{output}"
    );
}

#[test]
fn cook_step_many_to_one_iterates_union_too() {
    // Many-to-one steps (literal output, $<all> body) used the same
    // hardcoded `recipe.ingredients[1]` as the iteration source. The
    // fix applies uniformly across iteration modes.
    let cookfile = make_cookfile(vec![make_recipe(
        "build",
        vec![],
        vec!["src/*.c", "include/*.h"],
        vec![Step::Cook {
            step: CookStep {
                outputs: vec![OutputPattern::Quoted("build/app".to_string())],
                body: Some(Body::ShellBlock(
                    vec!["echo $<all>".to_string()],
                )),
                disposition: Default::default(),
            },
            line: 3,
        }],
    )]);
    let output = generate(&cookfile);
    assert!(
        output.contains("table.concat(ingredients, \" \")"),
        "many-to-one $<all> must concat the merged `ingredients` local, got:\n{output}"
    );
    assert!(
        !output.contains("table.concat(recipe.ingredients[1]"),
        "many-to-one must NOT concat recipe.ingredients[1] only, got:\n{output}"
    );
}

#[test]
fn plate_step_iterates_union_of_all_include_globs() {
    // Plate steps fall back to the recipe's resolved ingredient set
    // (Standard §4.7.1) when no preceding cook step exists. The fallback
    // must read the merged `ingredients` local, not `recipe.ingredients[1]`.
    let cookfile = make_cookfile(vec![make_recipe(
        "show",
        vec![],
        vec!["src/*.c", "include/*.h"],
        vec![Step::Plate {
            step: PlateStep {
                body: Body::ShellBlock(vec!["echo $<in>".to_string()]),
            },
            line: 3,
        }],
    )]);
    let output = generate(&cookfile);
    assert!(
        output.contains("for _, _plate_in in ipairs(ingredients) do"),
        "plate must iterate the merged `ingredients` local, got:\n{output}"
    );
    assert!(
        !output.contains("ipairs(recipe.ingredients[1])"),
        "plate must NOT iterate recipe.ingredients[1] only, got:\n{output}"
    );
}

#[test]
fn plate_step_emits_passthrough_after_iteration() {
    // Standard §5.4.1: a `plate` step's output is its input list,
    // forwarded as the recipe's terminal outputs so `$<recipe>` refs
    // expand to the plate's ingredients (or the preceding cook step's
    // outputs). The codegen calls `cook.passthrough(<source>)` after
    // the iteration loop, inside the enclosing step_group.
    let cookfile = make_cookfile(vec![make_recipe(
        "greet",
        vec![],
        vec!["Cookfile"],
        vec![Step::Plate {
            step: PlateStep {
                body: Body::ShellBlock(vec!["echo \"$<in>\"".to_string()]),
            },
            line: 3,
        }],
    )]);
    let output = generate(&cookfile);
    assert!(
        output.contains("cook.passthrough(ingredients)"),
        "plate must emit cook.passthrough(ingredients) so its input list \
         flows out as the recipe's terminal outputs, got:\n{output}"
    );
}

#[test]
fn plate_step_oneshot_with_ingredients_still_passthroughs() {
    // The Standard rule applies even when the plate body doesn't use
    // $<in>/$<all> (OneShot mode) — the input list is still the plate's
    // output. A bare `plate { echo "hi" }` after `ingredients "Cookfile"`
    // therefore emits a passthrough, and downstream `$<greet>` sees
    // `Cookfile` (not the empty string).
    let cookfile = make_cookfile(vec![make_recipe(
        "greet",
        vec![],
        vec!["Cookfile"],
        vec![Step::Plate {
            step: PlateStep {
                body: Body::ShellBlock(vec!["echo hello".to_string()]),
            },
            line: 3,
        }],
    )]);
    let output = generate(&cookfile);
    assert!(
        output.contains("cook.passthrough(ingredients)"),
        "OneShot plate with ingredients must still passthrough, got:\n{output}"
    );
}

#[test]
fn plate_step_with_no_source_omits_passthrough() {
    // A recipe with no ingredients and no preceding cook step has no
    // input list to pass through; emitting `cook.passthrough(...)` would
    // reference an undefined Lua local. Codegen skips the call in this
    // shape; the recipe's terminal outputs stay empty.
    let cookfile = make_cookfile(vec![make_recipe(
        "greet",
        vec![],
        vec![],
        vec![Step::Plate {
            step: PlateStep {
                body: Body::ShellBlock(vec!["echo hello".to_string()]),
            },
            line: 3,
        }],
    )]);
    let output = generate(&cookfile);
    assert!(
        !output.contains("cook.passthrough"),
        "a plate with no source must not emit cook.passthrough, got:\n{output}"
    );
}

#[test]
fn plate_step_after_cook_passthroughs_cook_outputs() {
    // When a plate follows a cook step, the source is the cook step's
    // outputs (`_cook_outputs_N`), not the recipe's ingredients.
    let cookfile = make_cookfile(vec![make_recipe(
        "build",
        vec![],
        vec!["src/*.c"],
        vec![
            Step::Cook {
                step: CookStep {
                    outputs: vec![OutputPattern::Quoted("out/$<in.stem>.o".to_string())],
                    body: Some(Body::ShellBlock(vec!["touch $<out>".to_string()])),
                    disposition: Default::default(),
                },
                line: 3,
            },
            Step::Plate {
                step: PlateStep {
                    body: Body::ShellBlock(vec!["echo $<in>".to_string()]),
                },
                line: 4,
            },
        ],
    )]);
    let output = generate(&cookfile);
    assert!(
        output.contains("cook.passthrough(_cook_outputs_1)"),
        "plate after cook step 1 must passthrough _cook_outputs_1, got:\n{output}"
    );
}

#[test]
fn test_step_emits_passthrough() {
    // Standard §5.4.1 lists `test` alongside `plate` as passthrough.
    let cookfile = make_cookfile(vec![make_recipe(
        "check",
        vec![],
        vec!["tests/*.sh"],
        vec![Step::Test {
            step: TestStep {
                body: Body::ShellBlock(vec!["bash $<in>".to_string()]),
                timeout: Some(5),
                should_fail: false,
                as_name: None,
            },
            line: 3,
        }],
    )]);
    let output = generate(&cookfile);
    assert!(
        output.contains("cook.passthrough(ingredients)"),
        "test step must emit cook.passthrough(ingredients), got:\n{output}"
    );
}

#[test]
fn test_step_iterates_union_of_all_include_globs() {
    // Test steps share plate's iteration-source fallback (Standard §4.8.1).
    let cookfile = make_cookfile(vec![make_recipe(
        "check",
        vec![],
        vec!["tests/*.sh", "extra/*.sh"],
        vec![Step::Test {
            step: TestStep {
                body: Body::ShellBlock(vec!["bash $<in>".to_string()]),
                timeout: Some(5),
                should_fail: false,
                as_name: None,
            },
            line: 3,
        }],
    )]);
    let output = generate(&cookfile);
    assert!(
        output.contains("ipairs(ingredients)"),
        "test step must iterate the merged `ingredients` local, got:\n{output}"
    );
    assert!(
        !output.contains("ipairs(recipe.ingredients[1])"),
        "test step must NOT iterate recipe.ingredients[1] only, got:\n{output}"
    );
}

#[test]
fn cook_step_excludes_threaded_through_resolve_ingredients() {
    // !"glob" exclude items have always reached cook.resolve_ingredients;
    // pin that the fix doesn't regress this path. The exclude appears in
    // the second slot of the resolve_ingredients call.
    let recipe = Recipe {
        name: "build".to_string(),
        deps: vec![],
        ingredients: vec!["src/*.c".to_string(), "include/*.h".to_string()],
        excludes: vec!["src/skip.c".to_string()],
        steps: vec![Step::Cook {
            step: CookStep {
                outputs: vec![OutputPattern::Quoted("build/$<in.stem>.o".to_string())],
                body: Some(Body::ShellBlock(
                    vec!["touch $<out>".to_string()],
                )),
                disposition: Default::default(),
            },
            line: 3,
        }],
        line: 1,
    };
    let cookfile = make_cookfile(vec![recipe]);
    let output = generate(&cookfile);
    assert!(
        output.contains(
            "cook.resolve_ingredients({\"src/*.c\", \"include/*.h\"}, {\"src/skip.c\"})"
        ),
        "exclude must appear in the resolve_ingredients call, got:\n{output}"
    );
}

// ── Standard §5.4: bare $<lib> in output pattern is rejected ──

#[test]
fn bare_recipe_ref_in_output_pattern_is_rejected() {
    // Standard §5.4 third bullet: a `cook` step whose output pattern list
    // contains a bare `$<lib>` (no accessor) naming a recipe MUST be
    // rejected at load time. The accessor form `$<lib.stem>` (dep-driven
    // iteration) and the bare form inside a `using` body (string-
    // substitution) are both legal — only "bare in an output pattern"
    // is banned.
    let src = r#"recipe lib
    ingredients "src/*.c"
    cook "build/$<in.stem>.o" { gcc -c $<in> -o $<out> }

recipe broken
    cook "out/$<lib>.txt" { echo hi > $<out> }
"#;
    let cookfile = cook_lang::parse(src).expect("parse");
    let names = crate::dep_ref::extract_recipe_names(&cookfile);
    let result = crate::generate_with_names_checked(&cookfile, &names);
    let err = result.expect_err("bare $<lib> in output pattern must be rejected");
    let msg = err.to_string();
    assert!(
        msg.contains("$<lib>"),
        "diagnostic must name the offending placeholder, got: {msg}"
    );
    assert!(
        msg.contains("output pattern"),
        "diagnostic must mention output pattern context, got: {msg}"
    );
    assert!(
        msg.contains("$<lib.stem>") || msg.contains("path accessor"),
        "diagnostic must hint at the accessor fix, got: {msg}"
    );
}

#[test]
fn dep_driven_accessor_in_output_pattern_still_accepted() {
    // Sibling check: the accessor form (`$<lib.stem>`) is the canonical
    // dep-driven shape and MUST keep parsing cleanly. This is what the
    // bare-form rejection above is teaching the user to write instead.
    let src = r#"recipe lib
    ingredients "src/*.c"
    cook "build/$<in.stem>.o" { gcc -c $<in> -o $<out> }

recipe driven
    cook "out/$<lib.stem>.txt" { echo hi > $<out> }
"#;
    let cookfile = cook_lang::parse(src).expect("parse");
    let names = crate::dep_ref::extract_recipe_names(&cookfile);
    let result = crate::generate_with_names_checked(&cookfile, &names);
    assert!(
        result.is_ok(),
        "dep-driven $<lib.stem> in output pattern must remain accepted, got: {:?}",
        result.err()
    );
}

#[test]
fn bare_recipe_ref_in_using_body_still_accepted() {
    // Sibling check: bare `$<lib>` IS legal inside a `using` body —
    // there it expands to the space-joined list of `lib`'s outputs
    // (Standard §5.5). Only the output-pattern position is rejected.
    let src = r#"recipe lib
    ingredients "src/*.c"
    cook "build/$<in.stem>.o" { gcc -c $<in> -o $<out> }

recipe link
    cook "build/app" { gcc $<lib> -o $<out> }
"#;
    let cookfile = cook_lang::parse(src).expect("parse");
    let names = crate::dep_ref::extract_recipe_names(&cookfile);
    let result = crate::generate_with_names_checked(&cookfile, &names);
    assert!(
        result.is_ok(),
        "bare $<lib> in a cook-step body must remain accepted, got: {:?}",
        result.err()
    );
}

#[test]
fn test_codegen_emits_register_block() {
    let cookfile = Cookfile {
        config_blocks: vec![],
        recipes: vec![],
        chores: vec![],
        uses: vec![],
        imports: vec![],
        register_blocks: vec![RegisterBlock {
            body: "    cook_cc.bin(\"game\", { sources = { \"src/main.c\" } })".to_string(),
            line: 1,
        }],
        top_level_module_calls: vec![],
        probes: vec![],
    };
    let out = generate(&cookfile);
    assert!(out.contains("cook_cc.bin(\"game\", { sources = { \"src/main.c\" } })"),
        "expected register-block body in output, got:\n{}", out);
}

#[test]
fn test_codegen_emits_register_blocks_in_source_order() {
    let cookfile = Cookfile {
        config_blocks: vec![],
        recipes: vec![],
        chores: vec![],
        uses: vec![],
        imports: vec![],
        register_blocks: vec![
            RegisterBlock { body: "    first()".to_string(),  line: 1 },
            RegisterBlock { body: "    second()".to_string(), line: 5 },
        ],
        top_level_module_calls: vec![],
        probes: vec![],
    };
    let out = generate(&cookfile);
    let first_pos = out.find("first()").expect("first() in output");
    let second_pos = out.find("second()").expect("second() in output");
    assert!(first_pos < second_pos, "expected first() before second() in output");
}

#[test]
fn test_codegen_interleaves_register_blocks_with_recipes() {
    let cookfile = Cookfile {
        config_blocks: vec![],
        recipes: vec![Recipe {
            name: "mid".to_string(),
            deps: vec![],
            ingredients: vec![],
            excludes: vec![],
            steps: vec![Step::Shell { command: "echo".into(), line: 5, interactive: false }],
            line: 4,
        }],
        chores: vec![],
        uses: vec![],
        imports: vec![],
        register_blocks: vec![
            RegisterBlock { body: "    before()".to_string(), line: 1 },
            RegisterBlock { body: "    after()".to_string(),  line: 8 },
        ],
        top_level_module_calls: vec![],
        probes: vec![],
    };
    let out = generate(&cookfile);
    let before_pos = out.find("before()").expect("before() in output");
    // Surface recipes lower to `cook.__register_surface` (CS-0077 codegen).
    let recipe_pos = out
        .find("cook.__register_surface(\"mid\"")
        .expect("recipe registration in output");
    let after_pos  = out.find("after()").expect("after() in output");
    assert!(before_pos < recipe_pos, "before() should precede `cook.__register_surface(\"mid\"`");
    assert!(recipe_pos < after_pos, "`cook.__register_surface(\"mid\"` should precede after()");
}

#[test]
fn test_codegen_emits_top_level_module_call() {
    let cookfile = Cookfile {
        config_blocks: vec![],
        recipes: vec![],
        chores: vec![],
        uses: vec![],
        imports: vec![],
        register_blocks: vec![],
        top_level_module_calls: vec![TopLevelModuleCall {
            code: "cook_cc.bin(\"game\", { sources = { \"src/main.c\" } })".to_string(),
            line: 1,
        }],
        probes: vec![],
    };
    let out = generate(&cookfile);
    assert!(out.contains("cook_cc.bin(\"game\", { sources = { \"src/main.c\" } })"),
        "expected top-level module_call body in output, got:\n{}", out);
}

#[test]
fn test_codegen_interleaves_top_level_module_calls_with_recipes() {
    let cookfile = Cookfile {
        config_blocks: vec![],
        recipes: vec![Recipe {
            name: "mid".to_string(),
            deps: vec![],
            ingredients: vec![],
            excludes: vec![],
            steps: vec![Step::Shell { command: "echo".into(), line: 5, interactive: false }],
            line: 4,
        }],
        chores: vec![],
        uses: vec![],
        imports: vec![],
        register_blocks: vec![],
        top_level_module_calls: vec![
            TopLevelModuleCall { code: "cpp.bin(\"a\", {})".to_string(), line: 1 },
            TopLevelModuleCall { code: "cpp.bin(\"b\", {})".to_string(), line: 8 },
        ],
        probes: vec![],
    };
    let out = generate(&cookfile);
    let a_pos      = out.find("cpp.bin(\"a\"").expect("`a` in output");
    // Surface recipes lower to `cook.__register_surface` (CS-0077 codegen).
    let recipe_pos = out
        .find("cook.__register_surface(\"mid\"")
        .expect("recipe registration in output");
    let b_pos      = out.find("cpp.bin(\"b\"").expect("`b` in output");
    assert!(a_pos < recipe_pos, "cpp.bin(\"a\") should precede recipe registration");
    assert!(recipe_pos < b_pos, "recipe registration should precede cpp.bin(\"b\")");
}

// ── COOK-36 Task 3: __params metadata + body-fn local-binding prelude ──────

#[test]
fn compile_chore_emits_param_metadata_and_locals() {
    use cook_lang::ast::{Chore, ChoreParam, Step};
    let chore = Chore {
        name: "deploy".into(),
        params: vec![
            ChoreParam::Required { name: "target".into(), line: 1, col: 13 },
            ChoreParam::DefaultedString {
                name: "host".into(), default: "prod".into(), line: 1, col: 20,
            },
        ],
        deps: vec![],
        steps: vec![Step::Lua { code: "deploy.run(target, host)".into(), line: 2 }],
        line: 1,
    };
    let lua = compile_chore(&chore, &[], &std::collections::BTreeSet::new());
    assert!(lua.contains("__params"), "lua: {lua}");
    assert!(lua.contains(r#"{name = "target", kind = "required""#), "lua: {lua}");
    assert!(lua.contains(r#"{name = "host", kind = "defaulted_string", default = "prod""#), "lua: {lua}");
    assert!(lua.contains("function(__cook_params)"), "lua: {lua}");
    assert!(lua.contains("local target = __cook_params.target"), "lua: {lua}");
    assert!(lua.contains("local host = __cook_params.host"), "lua: {lua}");
}

#[test]
fn compile_chore_emits_defaulted_lua_param_metadata() {
    use cook_lang::ast::{Chore, ChoreParam, Step};
    let chore = Chore {
        name: "release".into(),
        params: vec![
            ChoreParam::DefaultedLua {
                name: "version".into(),
                default_lua: "cook.git.head_tag() or \"v0\"".into(),
                line: 1, col: 0,
            },
        ],
        deps: vec![],
        steps: vec![Step::Lua { code: "release.cut(version)".into(), line: 2 }],
        line: 1,
    };
    let lua = compile_chore(&chore, &[], &std::collections::BTreeSet::new());
    assert!(
        lua.contains(r#"{name = "version", kind = "defaulted_lua", default = function() return (cook.git.head_tag() or "v0") end}"#),
        "lua: {lua}"
    );
    assert!(lua.contains("function(__cook_params)"), "lua: {lua}");
    assert!(lua.contains("local version = __cook_params.version"), "lua: {lua}");
}

#[test]
fn compile_chore_emits_variadic_param_metadata() {
    use cook_lang::ast::{Chore, ChoreParam, Step};
    let chore = Chore {
        name: "lint".into(),
        params: vec![
            ChoreParam::VariadicPlus { name: "files".into(), line: 1, col: 0 },
        ],
        deps: vec![],
        steps: vec![Step::Lua { code: "linter.run(files)".into(), line: 2 }],
        line: 1,
    };
    let lua = compile_chore(&chore, &[], &std::collections::BTreeSet::new());
    assert!(
        lua.contains(r#"{name = "files", kind = "variadic_plus"}"#),
        "lua: {lua}"
    );
    assert!(lua.contains("local files = __cook_params.files"), "lua: {lua}");
}

#[test]
fn compile_chore_with_no_params_does_not_emit_param_metadata_or_prelude() {
    use cook_lang::ast::{Chore, Step};
    let chore = Chore {
        name: "clean".into(),
        params: vec![],
        deps: vec![],
        steps: vec![Step::Lua { code: "fs.remove('build')".into(), line: 2 }],
        line: 1,
    };
    let lua = compile_chore(&chore, &[], &std::collections::BTreeSet::new());
    assert!(!lua.contains("__params"), "lua: {lua}");
    assert!(!lua.contains("local "), "no local-binding prelude expected for paramless chore. lua: {lua}");
    // Paramless chores still take `function(__cook_params)` so the runtime
    // can pass nil/{} uniformly. Confirm.
    assert!(lua.contains("function(__cook_params)"), "lua: {lua}");
}

// ── SHI-222 Phase 3 Task 3.2: pin chore + requires-bearing recipe shapes ──
//
// Task 3.1 pinned the no-deps surface-recipe emission shape
// (`cook.__register_surface("build", {__line = N}, ...)`). These two
// tests round out that contract by pinning:
//
//   (a) chores lower to `cook.__register_surface_chore(name, ...)` with
//       `__line = N` (so the capture layer can tag `RecipeKind::Chore`
//       and feed `SurfaceChore` into collision diagnostics), and
//   (b) recipes with declared `deps` emit `requires = {...}` in the
//       metadata table — the field name the capture layer reads via
//       `parse_meta_lists` and lands in `RegisteredRecipe.requires`.
//
// Together with `codegen_emits_register_surface_for_surface_recipes`
// (Task 3.1) these lock down the full surface-emission shape that the
// register-phase capture layer depends on, and that the surface-vs-
// dynamic collision test in `cook-register::tests` exercises end-to-end.

#[test]
fn codegen_chore_uses_register_surface_chore() {
    let cookfile = Cookfile {
        config_blocks: vec![],
        recipes: vec![],
        chores: vec![Chore {
            name: "clean".to_string(),
            params: vec![],
            deps: vec![],
            steps: vec![],
            line: 8,
        }],
        uses: vec![],
        imports: vec![],
        register_blocks: vec![],
        top_level_module_calls: vec![],
        probes: vec![],
    };
    let lua = generate(&cookfile);
    assert!(
        lua.contains(r#"cook.__register_surface_chore("clean""#),
        "expected cook.__register_surface_chore for surface chore, got:\n{lua}"
    );
    assert!(
        lua.contains("__line = 8"),
        "expected __line = 8 in chore metadata, got:\n{lua}"
    );
}

#[test]
fn codegen_register_surface_includes_requires() {
    let cookfile = Cookfile {
        config_blocks: vec![],
        recipes: vec![Recipe {
            name: "app".to_string(),
            deps: vec!["lib".to_string()],
            ingredients: vec![],
            excludes: vec![],
            steps: vec![],
            line: 10,
        }],
        chores: vec![],
        uses: vec![],
        imports: vec![],
        register_blocks: vec![],
        top_level_module_calls: vec![],
        probes: vec![],
    };
    let lua = generate(&cookfile);
    assert!(
        lua.contains(r#"cook.__register_surface("app""#),
        "expected cook.__register_surface for surface recipe, got:\n{lua}"
    );
    assert!(
        lua.contains(r#"requires = {"lib"}"#),
        "expected requires = {{\"lib\"}} in metadata, got:\n{lua}"
    );
    assert!(
        lua.contains("__line = 10"),
        "expected __line = 10 in metadata, got:\n{lua}"
    );
}

/// COOK-36 Task 8: chore shell units with params must emit `env = {...}` in
/// the `cook.add_unit` call so the register phase captures param values as
/// per-unit env vars, exported to the child shell at execution time.
#[test]
fn compile_chore_shell_step_emits_env_table_for_param() {
    use cook_lang::ast::{Chore, ChoreParam, Step};
    let chore = Chore {
        name: "say".into(),
        params: vec![
            ChoreParam::Required { name: "target".into(), line: 1, col: 5 },
        ],
        deps: vec![],
        steps: vec![Step::Shell {
            command: "sh -c 'echo $target'".into(),
            line: 2,
            interactive: true,
        }],
        line: 1,
    };
    let lua = compile_chore(&chore, &[], &std::collections::BTreeSet::new());
    // Must emit env = {["target"] = __cook_params.target}
    assert!(lua.contains("env ="), "env field missing from add_unit. lua:\n{lua}");
    assert!(lua.contains(r#"["target"] = __cook_params.target"#), "env key should be string literal, not variable reference. lua:\n{lua}");
}

// ── COOK-63 §8.3: ingredients <probe> data-member fan-out codegen ──────

#[test]
fn for_each_probe_cook_fans_out_per_member() {
    let src = "recipe art\n    ingredients cards\n    cook \"build/art/$<in.id>.png\" { gen \"$<in.name>\" $<out> }\n";
    let cookfile = cook_lang::parse(src).expect("parse");
    let lua = generate(&cookfile);
    // Member set sourced from the probe value (the COOK-64 pre-pass populates it).
    assert!(lua.contains("local _items = cook.cache.get(\"cards\")"),
        "missing probe member source, got:\n{lua}");
    // One cook.add_unit per member, member bound as `item`.
    assert!(lua.contains("for _, item in ipairs(_items) do"),
        "missing per-member loop, got:\n{lua}");
    // Output path interpolates the member field.
    assert!(lua.contains("tostring(item[\"id\"])"),
        "output should interpolate $<in.id>, got:\n{lua}");
    // Command interpolates the member field and $<out>.
    assert!(lua.contains("tostring(item[\"name\"])"),
        "command should interpolate $<in.name>, got:\n{lua}");
    assert!(lua.contains("cook.add_unit({inputs = {}, output = _cook_out, command = "),
        "missing ingredients-probe add_unit, got:\n{lua}");
}

#[test]
fn for_each_probe_field_indexes_array() {
    let src = "recipe a\n    ingredients cards:items\n    cook \"o/$<in.id>\" { build $<out> }\n";
    let lua = generate(&cook_lang::parse(src).unwrap());
    assert!(lua.contains("local _items = cook.cache.get(\"cards\")[\"items\"]"),
        "key:field should index the named field, got:\n{lua}");
}

#[test]
fn for_each_test_fans_out_per_member() {
    let src = "recipe eval\n    ingredients cases\n    test { assert-eval \"$<in.input>\" \"$<in.expect>\" }\n";
    let lua = generate(&cook_lang::parse(src).unwrap());
    assert!(lua.contains("for _, item in ipairs(_items) do"), "missing per-member loop, got:\n{lua}");
    assert!(lua.contains("tostring(item[\"input\"])"), "test body should interpolate $<in.input>, got:\n{lua}");
    assert!(lua.contains("cook.add_test({command = "), "missing test add_test, got:\n{lua}");
}

#[test]
fn for_each_surface_carries_source_metadata() {
    // COOK-64: the register pre-pass learns a recipe's ingredients-probe-feeding
    // probe from `__for_each` on the surface meta — without running the body.
    let probe = generate(&cook_lang::parse(
        "recipe a\n    ingredients cards\n    cook \"o/$<in.id>\" { x $<out> }\n",
    ).unwrap());
    assert!(probe.contains(r#"__for_each = {kind = "probe", key = "cards"}"#),
        "probe source metadata missing, got:\n{probe}");

    let field = generate(&cook_lang::parse(
        "recipe a\n    ingredients cards:items\n    cook \"o/$<in.id>\" { x $<out> }\n",
    ).unwrap());
    assert!(field.contains(r#"__for_each = {kind = "probe", key = "cards", field = "items"}"#),
        "key:field metadata missing, got:\n{field}");
}

#[test]
fn for_each_unit_folds_member_into_fingerprint() {
    // COOK-64 §17.1 observable #5: each fan-out unit carries its member so the
    // register fold distinguishes per-member fingerprints.
    let src = "recipe art\n    ingredients cards\n    cook \"o/$<in.id>\" { build $<out> }\n";
    let lua = generate(&cook_lang::parse(src).unwrap());
    assert!(lua.contains("member = cook.member_to_string(item)"),
        "ingredients-probe cook unit should carry member, got:\n{lua}");
}

// ── §22.5.2 — native probe lowering (COOK-68) ──────────────────────────────

fn make_probe_cf(produce: ProbeProduce) -> Cookfile {
    Cookfile {
        config_blocks: vec![], recipes: vec![], chores: vec![], uses: vec![],
        imports: vec![], register_blocks: vec![], top_level_module_calls: vec![],
        probes: vec![Probe {
            name: "p".into(), deps: vec![], ingredients: vec![], excludes: vec![],
            produce, line: 1,
        }],
    }
}

#[test]
fn probe_lua_block_lowers_to_cook_probe() {
    let cf = Cookfile {
        config_blocks: vec![], recipes: vec![], chores: vec![], uses: vec![],
        imports: vec![], register_blocks: vec![], top_level_module_calls: vec![],
        probes: vec![Probe {
            name: "services".into(), deps: vec!["cards".into()],
            ingredients: vec!["data/s.json".into()], excludes: vec![],
            produce: ProbeProduce::Lua("return {}".into()), line: 1,
        }],
    };
    let lua = generate(&cf);
    assert!(lua.contains("cook.probe(\"services\""), "lua:\n{lua}");
    assert!(lua.contains("requires = {\"cards\"}"), "lua:\n{lua}");
    assert!(lua.contains("files = cook.resolve_ingredients({\"data/s.json\"}, {})"), "lua:\n{lua}");
    assert!(lua.contains("return {}"), "lua:\n{lua}");
}

#[test]
fn probe_shell_json_lowers_with_json_decode() {
    let cf = make_probe_cf(ProbeProduce::Shell {
        commands: vec!["cat data.json".into()], typing: ShellProduceType::Json });
    let lua = generate(&cf);
    assert!(lua.contains("cook.json_decode(cook.sh("), "lua:\n{lua}");
    assert!(lua.contains("cat data.json"), "lua:\n{lua}");
}

#[test]
fn probe_shell_string_default_trims_newline() {
    let cf = make_probe_cf(ProbeProduce::Shell {
        commands: vec!["git rev-parse HEAD".into()], typing: ShellProduceType::String });
    let lua = generate(&cf);
    assert!(lua.contains("cook.sh("), "lua:\n{lua}");
    assert!(lua.contains(r#":gsub("\n$", "")"#), "lua:\n{lua}");
}

#[test]
fn probe_shell_lines_builds_array() {
    let cf = make_probe_cf(ProbeProduce::Shell {
        commands: vec!["git tag".into()], typing: ShellProduceType::Lines });
    let lua = generate(&cf);
    assert!(lua.contains(r#"gmatch("[^\n]+")"#), "lua:\n{lua}");
    assert!(lua.contains("return _r"), "lua:\n{lua}");
}

#[test]
fn probe_tools_lowers_with_command_v_and_sha256() {
    let cf = make_probe_cf(ProbeProduce::Tools(vec!["cc".into(), "ld".into()]));
    let lua = generate(&cf);
    assert!(lua.contains("command -v cc"), "lua:\n{lua}");
    assert!(lua.contains("command -v ld"), "lua:\n{lua}");
    assert!(lua.contains("sha256sum"), "lua:\n{lua}");
    assert!(lua.contains("path = _p"), "lua:\n{lua}");
    assert!(lua.contains("hash = _h"), "lua:\n{lua}");
    // Table keys must be quoted-string literals, NOT long-bracket `[[name]]`
    // (which is ambiguous as a table index — `_t[[[name]]]`).
    assert!(lua.contains(r#"_t["cc"]"#), "lua:\n{lua}");
    assert!(lua.contains(r#"_t["ld"]"#), "lua:\n{lua}");
    // The re-run TRIGGER: the named tools must be declared as probe inputs so
    // the fingerprint folds each binary's hash (COOK-164). Without this the
    // probe is a permanent cache hit and never re-runs on a tool upgrade.
    assert!(lua.contains(r#"tools = {"cc", "ld"}"#), "lua:\n{lua}");
}

#[test]
fn probe_env_lowers_with_cook_env_reads() {
    let cf = make_probe_cf(ProbeProduce::Env(vec!["SDKROOT".into(), "CC".into()]));
    let lua = generate(&cf);
    assert!(lua.contains("cook.env.SDKROOT"), "lua:\n{lua}");
    assert!(lua.contains("cook.env.CC"), "lua:\n{lua}");
    assert!(lua.contains(r#"_e["SDKROOT"]"#), "lua:\n{lua}");
    assert!(lua.contains(r#"_e["CC"]"#), "lua:\n{lua}");
    // The re-run TRIGGER: named env-vars declared as probe inputs so the
    // fingerprint folds each env value (COOK-164).
    assert!(lua.contains(r#"env = {"SDKROOT", "CC"}"#), "lua:\n{lua}");
}

#[test]
fn probe_shell_produce_with_brackets_escalates_levels() {
    // A shell command containing `]]` must not collide with the long-bracket
    // wraps: the inner `cook.sh([=[ … ]=])` escalates past the `]]`, and the
    // outer `produce = [==[ … ]==]` escalates past the inner `]=]`. Guards the
    // silent-truncation class for nested long strings.
    let cf = make_probe_cf(ProbeProduce::Shell {
        commands: vec!["echo ]]".into()], typing: ShellProduceType::String });
    let lua = generate(&cf);
    assert!(lua.contains("[=["), "expected escalated inner bracket, lua:\n{lua}");
    assert!(lua.contains("[==["), "expected escalated outer bracket, lua:\n{lua}");
}

#[test]
fn probe_no_ingredients_no_deps_omits_those_fields() {
    let cf = make_probe_cf(ProbeProduce::Lua("return 1".into()));
    let lua = generate(&cf);
    assert!(lua.contains("cook.probe("), "lua:\n{lua}");
    assert!(!lua.contains("resolve_ingredients"), "lua:\n{lua}");
    assert!(!lua.contains("requires ="), "lua:\n{lua}");
}

// ─── COOK-84: test-step inputs emission ──────────────────────────────────────

#[test]
fn test_step_with_ingredients_emits_inputs_field() {
    let src = "recipe unit\n    ingredients \"src/*.rs\"\n    test {\n        cargo test\n    } timeout 60\n";
    let lua = generate_lua_for_test(src);
    assert!(lua.contains("inputs = ingredients,"),
        "add_test must carry the resolved ingredient list:\n{lua}");
}

#[test]
fn test_step_without_ingredients_emits_no_inputs_field() {
    let src = "recipe build\n    cook \"build/out.txt\" {\n        echo hi > build/out.txt\n    }\n    test {\n        test -s $<in>\n    } timeout 5\n";
    let lua = generate_lua_for_test(src);
    assert!(!lua.contains("inputs = ingredients"),
        "cook-step-sourced tests must not reference the absent ingredients local:\n{lua}");
}

// ─── CS-0101: $<file:PATH> lowering (hoisted cook.file_ref locals + file_refs field) ─

/// Parse + checked-generate helper for the CS-0101 tests below.
fn checked_lua(src: &str) -> String {
    let cookfile = cook_lang::parse(src).expect("parse");
    let names = crate::dep_ref::extract_recipe_names(&cookfile);
    crate::generate_with_names_checked(&cookfile, &names).expect("codegen")
}

#[test]
fn file_ref_lowering_hoists_local_and_passes_file_refs() {
    let src = r#"recipe "html"
    ingredients "src/page.md"
    cook "build/$<in.stem>.html" {
        render --tokens $<file:tokens.css> $<in> -o $<out>
    }
end
"#;
    let lua = checked_lua(src);
    assert!(
        lua.contains("local _cook_fr_s0_1 = cook.file_ref(\"tokens.css\")"),
        "expected hoisted file-ref local, lua:\n{lua}"
    );
    assert!(
        lua.contains("_cook_fr_s0_1"),
        "expected substitution via the hoisted local, lua:\n{lua}"
    );
    assert!(
        lua.contains("file_refs = {\"tokens.css\"}"),
        "expected file_refs field on cook.add_unit, lua:\n{lua}"
    );
}

#[test]
fn file_ref_dedupes_repeated_pattern() {
    let src = r#"recipe "html"
    ingredients "src/page.md"
    cook "build/page.html" {
        render $<file:t.css> $<file:t.css> -o $<out>
    }
end
"#;
    let lua = checked_lua(src);
    let count = lua.matches("cook.file_ref(\"t.css\")").count();
    assert_eq!(
        count, 1,
        "repeated $<file:t.css> must hoist exactly one local, lua:\n{lua}"
    );
}

#[test]
fn file_ref_with_probe_ref_keeps_hoist_outside_deferred_fn() {
    // Probe refs defer the command into a `function() return ... end` closure;
    // the file-ref local must be hoisted BEFORE it (captured as an upvalue) so
    // the substitution value is still computed at register time.
    let src = r#"recipe "obj"
    ingredients "src/*.c"
    cook "build/$<in.stem>.o" {
        cc $<cc:zlib.cflags> --tokens $<file:t.css> -c $<in> -o $<out>
    }
end
"#;
    let lua = checked_lua(src);
    let hoist_pos = lua
        .find("local _cook_fr_s0_1 = cook.file_ref(\"t.css\")")
        .unwrap_or_else(|| panic!("expected hoisted file-ref local, lua:\n{lua}"));
    let deferred_pos = lua
        .find("function() return")
        .unwrap_or_else(|| panic!("expected probe-deferred command closure, lua:\n{lua}"));
    assert!(
        hoist_pos < deferred_pos,
        "file-ref hoist (at {hoist_pos}) must precede the deferred closure (at {deferred_pos}), lua:\n{lua}"
    );
}

#[test]
fn file_ref_in_output_pattern_is_codegen_error() {
    // CS-0101: a file reference is an input, not an iteration driver —
    // rejected in cook output patterns at codegen.
    let src = r#"recipe "bad"
    ingredients "src/*.md"
    cook "build/$<file:tokens.css>.html" {
        render -o $<out>
    }
end
"#;
    let cookfile = cook_lang::parse(src).expect("parse");
    let names = crate::dep_ref::extract_recipe_names(&cookfile);
    let result = crate::generate_with_names_checked(&cookfile, &names);
    assert!(
        result.is_err(),
        "$<file:PATH> in an output pattern must be a codegen error"
    );
    let err_str = result.unwrap_err().to_string();
    assert!(
        err_str.contains("not valid in a cook output pattern"),
        "error must explain the output-pattern rejection, got: {err_str}"
    );
}

#[test]
fn file_ref_in_fan_out_hoisted_once_outside_member_loop() {
    let src = r#"probe scenes
    produce as json { echo '[{"id":"intro"},{"id":"outro"}]' }

recipe html
    ingredients scenes
    cook "build/$<in.id>.html" { render --tokens $<file:t.css> $<in.id> -o $<out> }
"#;
    let lua = checked_lua(src);
    let count = lua.matches("cook.file_ref(").count();
    assert_eq!(
        count, 1,
        "fan-out must hoist the file ref exactly once (outside the member loop), lua:\n{lua}"
    );
    let hoist_pos = lua.find("cook.file_ref(").unwrap();
    let loop_pos = lua
        .find("for _, item in")
        .unwrap_or_else(|| panic!("expected member loop, lua:\n{lua}"));
    assert!(
        hoist_pos < loop_pos,
        "file-ref hoist (at {hoist_pos}) must precede the member loop (at {loop_pos}), lua:\n{lua}"
    );
    assert!(
        lua.contains("file_refs = {\"t.css\"}"),
        "expected file_refs field on the fan-out cook.add_unit, lua:\n{lua}"
    );
}
