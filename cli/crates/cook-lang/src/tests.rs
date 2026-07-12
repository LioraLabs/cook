use super::*;

// ── SHI-71: Unquoted recipe names ──────────────────────────────────

#[test]
fn test_bare_recipe_name_parses() {
    let source = "recipe build\n    cook.log(\"hi\")\n";
    let result = parse(source).unwrap();
    assert_eq!(result.recipes[0].name, "build");
}

#[test]
fn test_bare_recipe_name_with_deps() {
    let source = "recipe build: lib setup\n    cook.log(\"hi\")\n";
    let result = parse(source).unwrap();
    assert_eq!(result.recipes[0].name, "build");
    assert_eq!(result.recipes[0].deps, vec!["lib", "setup"]);
}

#[test]
fn test_bare_dotted_dep() {
    let source = "recipe bundle: backend.build\n";
    let result = parse(source).unwrap();
    assert_eq!(result.recipes[0].deps, vec!["backend.build"]);
}

#[test]
fn test_bare_use_statement() {
    let source = "use cpp\n\nrecipe build\n    cook.log(\"hi\")\n";
    let cookfile = parse(source).unwrap();
    assert_eq!(cookfile.uses[0].module_name, "cpp");
}

#[test]
fn test_config_block_with_lua_body_parses() {
    let source = "\
config release
    env.CXXFLAGS = \"-O3\"
    cpp.defaults({defines = {\"NDEBUG\"}})
";
    let result = parse(source).unwrap();
    assert_eq!(result.config_blocks.len(), 1);
    let block = &result.config_blocks[0];
    assert_eq!(block.name.as_deref(), Some("release"));
    assert!(block.body.contains("env.CXXFLAGS"));
    assert!(block.body.contains("cpp.defaults"));
}

#[test]
fn test_unnamed_config_block_parses() {
    let source = "\
config
    env.CC = \"gcc\"
";
    let result = parse(source).unwrap();
    assert_eq!(result.config_blocks.len(), 1);
    assert!(result.config_blocks[0].name.is_none());
    assert!(result.config_blocks[0].body.contains("env.CC"));
}

#[test]
fn test_config_block_preserves_multiline_body() {
    let source = "\
config dev
    env.CC = \"clang\"
    -- debug flags
    env.CXXFLAGS = \"-O0 -g\"
";
    let result = parse(source).unwrap();
    let body = &result.config_blocks[0].body;
    assert!(body.contains("clang"));
    assert!(body.contains("-- debug flags"));
    assert!(body.contains("-O0 -g"));
    assert_eq!(body.lines().count(), 3);
}

// ── SHI-73 / CS-0134: Module calls in recipe bodies ─────────────────
//
// CS-0134 (Cook Standard, purely-declarative recipe body) removes the `>>`
// sigil. A bare `<id>.<id>(...)` line inside a recipe body now auto-
// classifies as register-phase Lua (Step::InlineLua) — no sigil needed.

#[test]
fn test_module_call_single_line_is_inline_lua() {
    // CS-0134: an indented bare module-call in a recipe body auto-classifies
    // as register-phase InlineLua.
    let source = "recipe build\n    cpp.compile(\"src/*.cpp\")\n";
    let result = parse(source).unwrap();
    assert_eq!(result.recipes[0].steps.len(), 1);
    match &result.recipes[0].steps[0] {
        Step::InlineLua { code, .. } => {
            assert_eq!(code, "cpp.compile(\"src/*.cpp\")");
        }
        other => panic!("expected InlineLua step (CS-0134), got {:?}", other),
    }
}

#[test]
fn test_loose_shell_after_module_call_rejected() {
    // CS-0134: a bare module call is InlineLua; a following loose shell line
    // (`echo done`) is rejected.
    let source = "recipe build\n    cpp.compile(\"src/*.cpp\")\n    echo done\n";
    let err = parse(source).unwrap_err();
    let msg = format!("{}", err);
    assert!(msg.contains("loose shell commands are not allowed"), "got: {}", msg);
}

#[test]
fn test_non_module_dot_loose_shell_rejected() {
    // CS-0134: `./run.sh` is not a module call and is rejected as loose shell.
    let source = "recipe build\n    ./run.sh\n";
    let err = parse(source).unwrap_err();
    let msg = format!("{}", err);
    assert!(msg.contains("loose shell commands are not allowed"), "got: {}", msg);
}

#[test]
fn test_module_call_no_args_is_inline_lua() {
    // CS-0134: an indented bare module-call with no args becomes InlineLua.
    let source = "recipe build\n    cpp.detect_compiler()\n";
    let result = parse(source).unwrap();
    match &result.recipes[0].steps[0] {
        Step::InlineLua { code, .. } => {
            assert_eq!(code, "cpp.detect_compiler()");
        }
        other => panic!("expected InlineLua step (CS-0134), got {:?}", other),
    }
}

// ── Original tests ─────────────────────────────────────────────────

#[test]
fn test_empty_cookfile() {
    let result = parse("").unwrap();
    assert_eq!(result.recipes.len(), 0);
}

#[test]
fn test_minimal_recipe() {
    let source = "recipe \"build\"\n    cook.log(\"hi\")\n";
    let result = parse(source).unwrap();
    assert_eq!(result.recipes.len(), 1);
    assert_eq!(result.recipes[0].name, "build");
    assert!(result.recipes[0].deps.is_empty());
    assert!(result.recipes[0].ingredients.is_empty());
    assert_eq!(result.recipes[0].steps.len(), 1);
    match &result.recipes[0].steps[0] {
        Step::InlineLua { code, line } => {
            assert_eq!(code, "cook.log(\"hi\")");
            assert_eq!(*line, 2);
        }
        other => panic!("expected InlineLua step (CS-0134), got {:?}", other),
    }
}

#[test]
fn test_recipe_with_deps() {
    let source = "recipe \"build\": \"setup\" \"lib\"\n    cook.log(\"hi\")\n";
    let result = parse(source).unwrap();
    let recipe = &result.recipes[0];
    assert_eq!(recipe.deps, vec!["setup".to_string(), "lib".to_string()]);
}

#[test]
fn test_recipe_with_ingredients() {
    let source = "recipe \"lib\"\n    ingredients \"lib/*.c\" \"include/*.h\"\n    cook.log(\"hi\")\n";
    let result = parse(source).unwrap();
    let recipe = &result.recipes[0];
    assert_eq!(
        recipe.ingredients,
        vec!["lib/*.c".to_string(), "include/*.h".to_string()]
    );
}

#[test]
fn test_duplicate_ingredients_error() {
    let source = "recipe \"lib\"\n    ingredients \"lib/*.c\"\n    ingredients \"include/*.h\"\n";
    let result = parse(source);
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(msg.contains("duplicate"), "error was: {}", msg);
}

#[test]
fn test_cook_step_shell() {
    let source = "recipe \"lib\"\n    ingredients \"lib/*.c\"\n    cook \"build/obj/{stem}.o\" {\n        gcc -c {in} -o {out}\n    }\n";
    let result = parse(source).unwrap();
    let recipe = &result.recipes[0];
    assert_eq!(recipe.steps.len(), 1);
    match &recipe.steps[0] {
        Step::Cook { step, line } => {
            assert_eq!(*line, 3);
            assert_eq!(step.outputs[0].as_str(), "build/obj/{stem}.o");
            assert_eq!(
                step.body,
                Some(Body::ShellBlock(vec!["gcc -c {in} -o {out}".to_string()]))
            );
        }
        other => panic!("expected Cook step, got {:?}", other),
    }
}

#[test]
fn test_cook_step_many_to_one() {
    let source = "recipe \"lib\"\n    ingredients \"lib/*.c\"\n    cook \"build/lib.a\" {\n        ar rcs {out} {all}\n    }\n";
    let result = parse(source).unwrap();
    match &result.recipes[0].steps[0] {
        Step::Cook { step, .. } => {
            assert_eq!(step.outputs[0].as_str(), "build/lib.a");
            assert_eq!(
                step.body,
                Some(Body::ShellBlock(vec!["ar rcs {out} {all}".to_string()]))
            );
        }
        other => panic!("expected Cook, got {:?}", other),
    }
}

#[test]
fn cs0133_body_less_cook_rejected() {
    // CS-0133: declaration-only cook steps were removed. A body-less cook
    // (the §8.4.2 "decl + following shell_command" vaporware pattern) is a
    // parse error, converting the former silent 0-node registration + OneShot
    // runtime trap into a compile-time diagnostic.
    let source = "recipe \"build\"\n    ingredients \"src/*.c\"\n    cook \"bin/app\"\n    gcc src/main.c -o bin/app\n";
    let err = parse(source).expect_err("body-less cook must be rejected");
    let msg = err.to_string();
    assert!(msg.contains("declaration-only cook steps were removed"), "got: {}", msg);
}

#[test]
fn test_cook_step_lua_block() {
    let source = "recipe \"lib\"\n    ingredients \"lib/*.c\"\n    cook \"build/obj/{stem}.o\" >{\n        cook.sh(\"gcc -c \" .. input .. \" -o \" .. output)\n    }\n";
    let result = parse(source).unwrap();
    match &result.recipes[0].steps[0] {
        Step::Cook { step, .. } => {
            assert_eq!(step.outputs[0].as_str(), "build/obj/{stem}.o");
            match &step.body {
                Some(Body::LuaBlock(code)) => {
                    assert!(code.contains("cook.sh"), "code was: {}", code);
                }
                other => panic!("expected LuaBlock, got {:?}", other),
            }
        }
        other => panic!("expected Cook, got {:?}", other),
    }
}

#[test]
fn test_mixed_steps() {
    // CS-0134: recipe bodies are purely declarative. A bare `module.call()`
    // line auto-classifies as register-phase InlineLua and can sit between
    // two cook steps.
    let source = r#"recipe "lib": "setup"
    ingredients "lib/*.c" "include/*.h"
    cook "build/obj/{stem}.o" {
        gcc -c {in} -o {out}
    }
    cook.log("compiled")
    cook "build/libmath.a" {
        ar rcs {out} {all}
    }
"#;
    let result = parse(source).unwrap();
    let recipe = &result.recipes[0];
    assert_eq!(recipe.deps, vec!["setup".to_string()]);
    assert_eq!(recipe.ingredients, vec!["lib/*.c".to_string(), "include/*.h".to_string()]);
    assert_eq!(recipe.steps.len(), 3);
    assert!(matches!(&recipe.steps[0], Step::Cook { .. }));
    assert!(matches!(&recipe.steps[1], Step::InlineLua { .. }));
    assert!(matches!(&recipe.steps[2], Step::Cook { .. }));
}

#[test]
fn test_execute_lua_line_in_recipe_rejected() {
    // CS-0134: execute-phase `>` Lua is not allowed in a recipe body.
    let source = r#"recipe "bad"
    cook "a" {
        echo a
    }
    > print("x")
    cook "b" {
        echo b
    }
"#;
    let err = parse(source).unwrap_err();
    let msg = format!("{}", err);
    assert!(msg.contains("execute-phase `>` Lua is not allowed"), "got: {}", msg);
}

#[test]
fn test_indented_module_call_is_inline_lua() {
    // CS-0134: an indented bare module call auto-classifies as register-phase
    // InlineLua.
    let source = "recipe \"r\"\n    pnpm.run(\"build\")\n";
    let result = parse(source).unwrap();
    let recipe = &result.recipes[0];
    assert_eq!(recipe.steps.len(), 1);
    match &recipe.steps[0] {
        Step::InlineLua { code, .. } => assert_eq!(code, "pnpm.run(\"build\")"),
        other => panic!("expected InlineLua, got {:?}", other),
    }
}

#[test]
fn test_multiline_module_call_is_inline_lua() {
    // CS-0134: a multi-line bare module call collects into one InlineLua step.
    let source = "recipe \"r\"\n    cook.add_unit({\n        command = \"x\",\n        outputs = { \"o\" },\n    })\n";
    let result = parse(source).unwrap();
    let recipe = &result.recipes[0];
    assert_eq!(recipe.steps.len(), 1);
    match &recipe.steps[0] {
        Step::InlineLua { code, .. } => {
            assert!(code.contains("cook.add_unit({"), "got: {}", code);
            assert!(code.contains("command = \"x\""), "got: {}", code);
            assert!(code.contains("})"), "got: {}", code);
        }
        other => panic!("expected InlineLua, got {:?}", other),
    }
}

#[test]
fn test_register_sigil_line_removed_in_recipe() {
    // CS-0134: the register-phase `>>` sigil was removed from the language.
    let source = "recipe \"r\"\n    >> local x = 1\n";
    let err = parse(source).unwrap_err();
    let msg = format!("{}", err);
    assert!(msg.contains("`>>` sigil was removed"), "got: {}", msg);
}

#[test]
fn test_register_sigil_block_removed_in_recipe() {
    // CS-0134: the register-phase `>>{ … }` sigil was removed.
    let source = "recipe \"r\"\n    >>{\n        cook.env.K = \"v\"\n    }\n";
    let err = parse(source).unwrap_err();
    let msg = format!("{}", err);
    assert!(msg.contains("`>>{ … }` sigil was removed"), "got: {}", msg);
}

#[test]
fn test_execute_lua_block_in_recipe_rejected() {
    // CS-0134: execute-phase `>{ … }` block is not allowed in a recipe body.
    let source = "recipe \"r\"\n    >{\n        cook.sh(\"echo hi\")\n    }\n";
    let err = parse(source).unwrap_err();
    let msg = format!("{}", err);
    assert!(msg.contains("execute-phase `>{ … }` Lua block is not allowed"), "got: {}", msg);
}

#[test]
fn test_loose_shell_in_recipe_rejected() {
    // CS-0134: loose shell commands are not allowed in a recipe body.
    let source = "recipe \"r\"\n    gcc -o main main.c\n";
    let err = parse(source).unwrap_err();
    let msg = format!("{}", err);
    assert!(msg.contains("loose shell commands are not allowed"), "got: {}", msg);
}

#[test]
fn test_at_prefix_in_recipe_rejected() {
    // CS-0134: the `@` interactive prefix was removed from the language.
    let source = "recipe \"r\"\n    @./bin/app\n";
    let err = parse(source).unwrap_err();
    let msg = format!("{}", err);
    assert!(msg.contains("`@` interactive prefix was removed"), "got: {}", msg);
}

#[test]
fn test_using_register_block_rejected() {
    // App. A.4 "`using >>{` is rejected".
    let source = "recipe \"r\"\n    cook \"out\" using >>{\n        cook.add_unit({command = \"x\"})\n    }\n";
    let err = parse(source).unwrap_err();
    let msg = format!("{}", err);
    assert!(msg.contains("using"), "got: {}", msg);
}

#[test]
fn test_triple_arrow_reserved() {
    // §{lexical.line-prefixes}: `>>>` and longer are reserved.
    let source = "recipe \"r\"\n    >>> print(\"x\")\n";
    let err = parse(source).unwrap_err();
    let msg = format!("{}", err);
    assert!(msg.contains(">"), "got: {}", msg);
}

#[test]
fn test_task_runner_no_metadata() {
    let source = "recipe \"clean\"\n    cook.log(\"hi\")\n";
    let result = parse(source).unwrap();
    let recipe = &result.recipes[0];
    assert!(recipe.deps.is_empty());
    assert!(recipe.ingredients.is_empty());
    assert_eq!(recipe.steps.len(), 1);
}

#[test]
fn test_multiple_recipes() {
    let source = "recipe \"setup\"\n    cook.log(\"a\")\n\nrecipe \"build\": \"setup\"\n    cook.log(\"b\")\n";
    let result = parse(source).unwrap();
    assert_eq!(result.recipes.len(), 2);
    assert_eq!(result.recipes[0].name, "setup");
    assert_eq!(result.recipes[1].name, "build");
    assert_eq!(result.recipes[1].deps, vec!["setup".to_string()]);
}

#[test]
fn test_lua_block_in_chore() {
    // CS-0134: execute-phase `>{ … }` blocks live in chores, not recipes.
    let source = "chore \"build\"\n>{\n    local x = 1\n    print(x)\n}\n";
    let result = parse(source).unwrap();
    assert_eq!(result.chores[0].steps.len(), 1);
    assert!(matches!(&result.chores[0].steps[0], Step::LuaBlock { .. }));
}

#[test]
fn test_lua_block_nested_braces() {
    let source = "chore \"build\"\n>{\n    if true then\n        local t = {1, 2, 3}\n    end\n}\n";
    let result = parse(source).unwrap();
    match &result.chores[0].steps[0] {
        Step::LuaBlock { code, .. } => {
            assert!(code.contains("local t = {1, 2, 3}"));
        }
        other => panic!("expected LuaBlock, got {:?}", other),
    }
}

#[test]
fn test_comments_and_blanks_skipped() {
    let source = "recipe \"build\"\n    # comment\n    cook.log(\"hi\")\n";
    let result = parse(source).unwrap();
    assert_eq!(result.recipes[0].steps.len(), 1);
}

#[test]
fn test_end_outside_recipe() {
    assert!(parse("end\n").is_err());
}

#[test]
fn test_unclosed_lua_block() {
    let source = "recipe \"build\"\n>{\n    local x = 1\nend\n";
    assert!(parse(source).is_err());
}

#[test]
fn test_lua_block_brace_in_string() {
    let source = "chore \"build\"\n>{\n    local s = \"}\"\n    print(s)\n}\n";
    let result = parse(source).unwrap();
    match &result.chores[0].steps[0] {
        Step::LuaBlock { code, .. } => {
            assert!(code.contains("local s = \"}\""));
        }
        other => panic!("expected LuaBlock, got {:?}", other),
    }
}

#[test]
fn test_lua_block_brace_in_comment() {
    let source = "chore \"build\"\n>{\n    local x = 1 -- }\n    print(x)\n}\n";
    let result = parse(source).unwrap();
    match &result.chores[0].steps[0] {
        Step::LuaBlock { code, .. } => {
            assert!(code.contains("local x = 1 -- }"));
        }
        other => panic!("expected LuaBlock, got {:?}", other),
    }
}

#[test]
fn test_strip_keyword() {
    use crate::cook_line::strip_keyword;
    assert_eq!(strip_keyword("ingredients \"a\"", "ingredients"), Some("\"a\""));
    assert_eq!(strip_keyword("cook \"x\"", "cook"), Some("\"x\""));
    assert_eq!(strip_keyword("plate \"x\"", "plate"), Some("\"x\""));
    assert_eq!(strip_keyword("cooking", "cook"), None);
    assert_eq!(strip_keyword("ingredient", "ingredients"), None);
}

#[test]
fn test_mixed_named_and_unnamed_config_blocks() {
    let source = "\
config
    cpp.defaults({})
end
config release
    env.CXXFLAGS = \"-O3\"
end
config dev
    env.CXXFLAGS = \"-O0 -g\"
end
";
    let result = parse(source).unwrap();
    assert_eq!(result.config_blocks.len(), 3);
    assert!(result.config_blocks[0].name.is_none());
    assert_eq!(result.config_blocks[1].name.as_deref(), Some("release"));
    assert_eq!(result.config_blocks[2].name.as_deref(), Some("dev"));
}

#[test]
fn test_empty_config_block() {
    let source = r#"config "empty"

recipe "build"
    cook.log("hi")
"#;
    let result = parse(source).unwrap();
    assert_eq!(result.config_blocks.len(), 1);
    assert_eq!(result.config_blocks[0].body, "");
}

#[test]
fn test_indented_quoted_pair_is_shell_command() {
    // CS-0134: recipe bodies reject loose shell, so the `NAME "value"` shell
    // classification is exercised in a chore (where shell is permitted).
    let source = r#"chore "build"
    CC "gcc"
"#;
    let result = parse(source).unwrap();
    assert_eq!(result.chores.len(), 1);
    assert!(matches!(
        &result.chores[0].steps[0],
        Step::Shell { command, .. } if command.contains("CC")
    ));
}

#[test]
fn test_config_after_recipe_errors() {
    let source = r#"recipe "build"
    cook.log("hi")

config "debug"
    CFLAGS "-g"
"#;
    let result = parse(source);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("config blocks must appear before recipes"), "got: {err}");
}

#[test]
fn test_duplicate_named_config_errors() {
    let source = "\
config release
    env.CC = \"gcc\"
end
config release
    env.CC = \"clang\"
end
";
    let err = parse(source).unwrap_err();
    assert!(format!("{}", err).contains("duplicate config"));
}

#[test]
fn test_duplicate_unnamed_config_errors() {
    let source = "\
config
    env.CC = \"gcc\"
end
config
    env.CC = \"clang\"
end
";
    let err = parse(source).unwrap_err();
    assert!(format!("{}", err).contains("multiple unnamed config"));
}

#[test]
fn test_empty_interactive_step_errors() {
    // CS-0134: the `@` interactive prefix was removed; a bare `@` is rejected
    // with the removal diagnostic.
    let source = "recipe \"run\"\n    @\n";
    let result = parse(source);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("`@` interactive prefix was removed"), "got: {err}");
}

#[test]
fn test_at_in_cook_body_is_not_interactive() {
    let source = r#"recipe "build"
    ingredients "src/*.c"
    cook "build/{stem}.o" {
        @gcc -c {in} -o {out}
    }
"#;
    let result = parse(source).unwrap();
    match &result.recipes[0].steps[0] {
        Step::Cook { step, .. } => {
            match &step.body {
                Some(Body::ShellBlock(cmds)) => {
                    assert!(cmds[0].starts_with('@'), "@ should be preserved in shell block");
                }
                other => panic!("expected ShellBlock using clause, got {:?}", other),
            }
        }
        other => panic!("expected Cook step, got {:?}", other),
    }
}

#[test]
fn test_parse_use_statement() {
    let source = "use \"cpp\"\n\nrecipe \"build\"\n    cook.log(\"hi\")\n";
    let cookfile = crate::parse(source).unwrap();
    assert_eq!(cookfile.uses.len(), 1);
    assert_eq!(cookfile.uses[0].module_name, "cpp");
    assert_eq!(cookfile.uses[0].line, 1);
    assert_eq!(cookfile.recipes.len(), 1);
}

#[test]
fn test_parse_multiple_use_statements() {
    let source = "use \"cpp\"\nuse \"proto\"\n\nrecipe \"build\"\n    cook.log(\"hi\")\n";
    let cookfile = crate::parse(source).unwrap();
    assert_eq!(cookfile.uses.len(), 2);
    assert_eq!(cookfile.uses[0].module_name, "cpp");
    assert_eq!(cookfile.uses[1].module_name, "proto");
}

#[test]
fn test_parse_use_with_configs() {
    let source = "use \"cpp\"\n\nconfig \"debug\"\n    env.CFLAGS = \"-g\"\n\nrecipe \"build\"\n    cook.log(\"hi\")\n";
    let cookfile = crate::parse(source).unwrap();
    assert_eq!(cookfile.uses.len(), 1);
    assert_eq!(cookfile.config_blocks.len(), 1);
    assert_eq!(cookfile.recipes.len(), 1);
}

#[test]
fn test_test_step_basic() {
    let source = "recipe r\n    ingredients \"tests/*.c\"\n    cook \"build/{in.stem}\" { cc {in} -o {out} }\n    test { ./{in} }\n";
    let cookfile = parse(source).expect("should parse");
    match &cookfile.recipes[0].steps[1] {
        Step::Test { step, .. } => {
            assert!(matches!(step.body, Body::ShellBlock(_)));
        }
        other => panic!("expected Test, got {:?}", other),
    }
}

#[test]
fn test_test_step_shell_body_with_dep_ref() {
    // CS-0135: test steps carry only a body (no as/timeout/should_fail).
    let source = "recipe r\n    ingredients \"tests/*.c\"\n    cook \"build/{in.stem}\" { cc {in} -o {out} }\n    test { ./$<in> }\n";
    let cookfile = parse(source).expect("should parse");
    match &cookfile.recipes[0].steps[1] {
        Step::Test { step, .. } => match &step.body {
            Body::ShellBlock(lines) => {
                assert!(lines.iter().any(|l| l.contains("$<in>")), "lines: {:?}", lines);
            }
            other => panic!("expected ShellBlock, got {:?}", other),
        },
        other => panic!("expected Test, got {:?}", other),
    }
}

#[test]
fn test_test_step_should_fail_removed() {
    // CS-0135: `should_fail` was removed in v1.0; trailing content after a
    // test body's closing `}` is a parse error with a did-you-mean hint.
    let source = "recipe r\n    ingredients \"tests/*.c\"\n    cook \"build/{in.stem}\" { cc {in} -o {out} }\n    test { x } should_fail\n";
    let err = parse(source).expect_err("should_fail modifier must be rejected");
    let msg = err.to_string();
    assert!(msg.contains("should_fail was removed in v1.0"), "got: {}", msg);
}

#[test]
fn test_parse_use_after_recipe_fails() {
    let source = "recipe \"build\"\n    echo hello\n\nuse \"cpp\"\n";
    let result = crate::parse(source);
    assert!(result.is_err());
}

#[test]
fn test_parse_import_decl() {
    let source = r#"
import backend ./services/backend
import frontend ./apps/frontend

recipe "bundle": "backend.build" "frontend.build"
"#;
    let cookfile = crate::parse(source).unwrap();
    assert_eq!(cookfile.imports.len(), 2);
    assert_eq!(cookfile.imports[0].name, "backend");
    assert_eq!(cookfile.imports[0].path.to_string(), "./services/backend");
    assert_eq!(cookfile.imports[1].name, "frontend");
    assert_eq!(cookfile.imports[1].path.to_string(), "./apps/frontend");
}

#[test]
fn test_parse_import_after_recipe_fails() {
    let source = r#"
recipe "build"
end

import backend ./services/backend
"#;
    let result = crate::parse(source);
    assert!(result.is_err());
}

#[test]
fn test_parse_duplicate_import_names_fails() {
    let source = r#"
import backend ./services/a
import backend ./services/b
"#;
    let result = crate::parse(source);
    assert!(result.is_err());
}

// ── Ingredient exclusion ──────────────────────────────────────────

#[test]
fn test_ingredients_with_excludes() {
    let source = r#"recipe build
    ingredients "src/*.c" !"src/lua.c" !"src/luac.c"
"#;
    let result = parse(source).unwrap();
    let recipe = &result.recipes[0];
    assert_eq!(recipe.ingredients, vec!["src/*.c"]);
    assert_eq!(recipe.excludes, vec!["src/lua.c", "src/luac.c"]);
}

#[test]
fn test_ingredients_excludes_only() {
    let source = r#"recipe build
    ingredients !"src/test.c"
"#;
    let result = parse(source).unwrap();
    let recipe = &result.recipes[0];
    assert!(recipe.ingredients.is_empty());
    assert_eq!(recipe.excludes, vec!["src/test.c"]);
}

#[test]
fn test_ingredients_no_excludes() {
    let source = r#"recipe build
    ingredients "src/*.c" "include/*.h"
"#;
    let result = parse(source).unwrap();
    let recipe = &result.recipes[0];
    assert_eq!(recipe.ingredients, vec!["src/*.c", "include/*.h"]);
    assert!(recipe.excludes.is_empty());
}

#[test]
fn test_multi_output_lua_block() {
    let source = "recipe \"wasm\"\n    ingredients \"src/*.rs\"\n    cook \"a.js\" \"b.wasm\" >{\n        sh(\"cmd\")\n    }\n";
    let result = crate::parse(source).expect("should parse");
    match &result.recipes[0].steps[0] {
        crate::ast::Step::Cook { step, .. } => {
            let outs: Vec<&str> = step.outputs.iter().map(|p| p.as_str()).collect();
            assert_eq!(outs, vec!["a.js", "b.wasm"]);
            assert!(matches!(step.body, Some(crate::ast::Body::LuaBlock(_))));
        }
        _ => panic!("expected Cook step"),
    }
}

#[test]
fn test_single_output_shell_block() {
    let source = "recipe \"x\"\n    ingredients \"src/*\"\n    cook \"bin/out\" {\n        cmd1\n        cmd2\n    }\n";
    let result = crate::parse(source).expect("should parse");
    match &result.recipes[0].steps[0] {
        crate::ast::Step::Cook { step, .. } => {
            let outs: Vec<&str> = step.outputs.iter().map(|p| p.as_str()).collect();
            assert_eq!(outs, vec!["bin/out"]);
            match &step.body {
                Some(crate::ast::Body::ShellBlock(cmds)) => {
                    assert_eq!(cmds, &vec!["cmd1".to_string(), "cmd2".to_string()]);
                }
                _ => panic!("expected ShellBlock"),
            }
        }
        _ => panic!("expected Cook step"),
    }
}

#[test]
fn test_multi_output_shell_block() {
    let source = "recipe \"wasm\"\n    ingredients \"src/*.rs\"\n    cook \"a.js\" \"b.wasm\" {\n        wasm-pack build\n        cp a.js out/a.js\n        cp b.wasm out/b.wasm\n    }\n";
    let result = crate::parse(source).expect("should parse");
    match &result.recipes[0].steps[0] {
        crate::ast::Step::Cook { step, .. } => {
            let outs: Vec<&str> = step.outputs.iter().map(|p| p.as_str()).collect();
            assert_eq!(outs, vec!["a.js", "b.wasm"]);
            match &step.body {
                Some(crate::ast::Body::ShellBlock(cmds)) => {
                    assert_eq!(cmds.len(), 3);
                    assert_eq!(cmds[0], "wasm-pack build");
                }
                _ => panic!("expected ShellBlock"),
            }
        }
        _ => panic!("expected Cook step"),
    }
}

// ── Chore tests (CS-0020, E.7) ────────────────────────────────────

#[test]
fn test_chore_basic() {
    let input = "chore clean\n    rm -rf build\n";
    let cookfile = parse(input).expect("chore should parse");
    assert_eq!(cookfile.chores.len(), 1);
    assert_eq!(cookfile.chores[0].name, "clean");
    assert_eq!(cookfile.chores[0].steps.len(), 1);
    match &cookfile.chores[0].steps[0] {
        Step::Shell { command, interactive, .. } => {
            assert_eq!(command, "rm -rf build");
            assert!(*interactive, "chore shell step must be default-interactive");
        }
        _ => panic!("expected Shell step"),
    }
}

#[test]
fn test_chore_at_prefix_rejected() {
    // CS-0134: the `@` interactive prefix was removed; chore commands are
    // interactive by default so the marker is rejected.
    let input = "chore deploy\n    @rsync -av out/\n";
    let err = parse(input).unwrap_err();
    let msg = format!("{}", err);
    assert!(msg.contains("`@` interactive prefix was removed"), "got: {}", msg);
}

#[test]
fn test_chore_register_sigil_rejected() {
    // CS-0134: the `>>` register sigil was removed from chores too.
    let input = "chore deploy\n    >> local x = 1\n";
    let err = parse(input).unwrap_err();
    let msg = format!("{}", err);
    assert!(msg.contains("`>>` sigil was removed"), "got: {}", msg);
}

#[test]
fn test_chore_register_sigil_block_rejected() {
    // CS-0134: the register-phase `>>{ … }` sigil was removed from chores too.
    let input = "chore deploy\n    >>{\n        cook.env.K = \"v\"\n    }\n";
    let err = parse(input).unwrap_err();
    let msg = format!("{}", err);
    assert!(msg.contains("`>>{ … }` sigil was removed"), "got: {}", msg);
}

#[test]
fn test_chore_keeps_execute_lua() {
    // CS-0134: chores stay imperative — plain shell + `>` / `>{` still parse.
    let input = "chore build\n    echo hi\n    > cook.sh(\"echo x\")\n    >{\n        cook.sh(\"echo y\")\n    }\n";
    let cookfile = parse(input).expect("chore should parse");
    let steps = &cookfile.chores[0].steps;
    assert_eq!(steps.len(), 3);
    assert!(matches!(&steps[0], Step::Shell { interactive: true, .. }));
    assert!(matches!(&steps[1], Step::Lua { .. }));
    assert!(matches!(&steps[2], Step::LuaBlock { .. }));
}

#[test]
fn test_chore_with_deps() {
    let input = "chore play: build\n    ./build/app\n";
    let cookfile = parse(input).expect("chore should parse");
    assert_eq!(cookfile.chores[0].deps, vec!["build".to_string()]);
}

#[test]
fn test_chore_with_ingredients_rejected() {
    let input = "chore clean\n    ingredients \"build/*\"\n";
    let result = parse(input);
    assert!(result.is_err());
    let err = result.unwrap_err();
    let msg = format!("{}", err);
    assert!(msg.contains("'ingredients' is not allowed in a chore"), "got: {}", msg);
}

#[test]
fn test_chore_with_cook_rejected() {
    let input = "chore deploy\n    cook \"out\" using \"true\"\n";
    let result = parse(input);
    assert!(result.is_err());
    let msg = format!("{}", result.unwrap_err());
    assert!(msg.contains("'cook' is not allowed in a chore"), "got: {}", msg);
}

#[test]
fn test_chore_with_test_rejected() {
    let input = "chore play\n    test { ./run }\n";
    assert!(parse(input).is_err());
}

#[test]
fn test_chore_lua_step() {
    let input = "chore status\n    > print(\"hello\")\n";
    let cookfile = parse(input).expect("chore should parse");
    assert!(matches!(cookfile.chores[0].steps[0], Step::Lua { .. }));
}

#[test]
fn test_chore_implicit_termination() {
    let input = "chore clean\n    rm -rf build\nchore play\n    ./app\n";
    let cookfile = parse(input).expect("two chores should parse");
    assert_eq!(cookfile.chores.len(), 2);
    assert_eq!(cookfile.chores[0].steps.len(), 1);
    assert_eq!(cookfile.chores[1].steps.len(), 1);
}

#[test]
fn test_recipe_after_chore_ok() {
    let input = "chore clean\n    rm -rf build\nrecipe build\n    cook \"out\" { touch out }\n";
    let cookfile = parse(input).expect("recipe after chore should parse");
    assert_eq!(cookfile.chores.len(), 1);
    assert_eq!(cookfile.recipes.len(), 1);
}

#[test]
fn test_use_after_chore_rejected() {
    let input = "chore clean\n    rm -rf build\nuse cpp\n";
    assert!(parse(input).is_err());
}

#[test]
fn test_multi_output_string_form_rejected() {
    let source = "recipe \"x\"\n    ingredients \"src/*\"\n    cook \"a.js\" \"b.wasm\" using \"cmd\"\n";
    let err = crate::parse(source).expect_err("should reject");
    let msg = format!("{}", err);
    assert!(msg.contains("using") && msg.contains("not supported"), "expected using-keyword diagnostic, got: {}", msg);
}

#[test]
fn test_using_string_form_rejected_with_migration_diagnostic() {
    let src = r#"recipe build
    cook "out" using "echo hi"
"#;
    let err = parse(src).expect_err("the using keyword must be rejected");
    match err {
        ParseError::Parse { message, .. } => {
            assert!(message.contains("using"), "diagnostic should name the keyword, got: {message}");
            assert!(message.contains("not supported"), "diagnostic should reject the keyword, got: {message}");
        }
        e => panic!("expected ParseError::Parse, got {:?}", e),
    }
}

// ── CS-0022 Phase G Item 5: one-line shell block ──────────────────

#[test]
fn cs_0022_one_line_shell_block_parses() {
    // `using { cmd }` on one line must parse to a ShellBlock with one command.
    let src = "recipe build\n    cook \"build/{in.stem}.o\" { gcc -c {in} -o {out} }\n";
    let cookfile = parse(src).expect("one-line shell block should parse");
    match &cookfile.recipes[0].steps[0] {
        Step::Cook { step, .. } => {
            match &step.body {
                Some(Body::ShellBlock(cmds)) => {
                    assert_eq!(cmds.len(), 1, "expected 1 command, got {:?}", cmds);
                    assert_eq!(cmds[0], "gcc -c {in} -o {out}");
                }
                other => panic!("expected ShellBlock, got {:?}", other),
            }
        }
        other => panic!("expected Cook step, got {:?}", other),
    }
}

#[test]
fn cs_0022_one_line_shell_block_with_placeholder_braces() {
    // Placeholders like {in} and {out} inside the one-line block must not
    // confuse the brace-depth tracker.
    let src = "recipe build\n    ingredients \"src/*.c\"\n    cook \"build/{in.stem}.o\" { {CC} -c {in} -o {out} }\n";
    let cookfile = parse(src).expect("one-line shell block with placeholders should parse");
    match &cookfile.recipes[0].steps[0] {
        Step::Cook { step, .. } => {
            match &step.body {
                Some(Body::ShellBlock(cmds)) => {
                    assert_eq!(cmds.len(), 1);
                    assert!(cmds[0].contains("{CC}"), "got: {:?}", cmds);
                    assert!(cmds[0].contains("{in}"), "got: {:?}", cmds);
                }
                other => panic!("expected ShellBlock, got {:?}", other),
            }
        }
        other => panic!("expected Cook step, got {:?}", other),
    }
}

#[test]
fn cs_0022_one_line_shell_block_followed_by_more_steps() {
    // A one-line block must correctly advance the token position so that
    // subsequent steps parse correctly.
    let src = "recipe build\n    cook \"build/app\" { gcc main.c -o {out} }\n    test { ./{in} }\n";
    let cookfile = parse(src).expect("should parse");
    assert_eq!(cookfile.recipes[0].steps.len(), 2, "should have cook + test");
    assert!(matches!(&cookfile.recipes[0].steps[0], Step::Cook { .. }));
    assert!(matches!(&cookfile.recipes[0].steps[1], Step::Test { step, .. } if matches!(step.body, Body::ShellBlock(_))));
}

#[test]
fn test_test_string_form_rejected() {
    let source = "recipe r\n    ingredients \"tests/*.c\"\n    cook \"build/{in.stem}\" { cc {in} -o {out} }\n    test \"./{out}\"\n";
    let err = parse(source).unwrap_err();
    let msg = format!("{}", err);
    assert!(
        msg.contains("test") && msg.contains("not supported") && msg.contains("{ cmd }"),
        "expected migration diagnostic for test string form, got: {}",
        msg
    );
}

#[test]
fn test_test_lua_block_parses() {
    let source = "recipe r\n    ingredients \"src/*.c\"\n    cook \"build/{in.stem}\" { cc {in} -o {out} }\n    test >{\n        cook.sh(\"strip \" .. input)\n    }\n";
    let cookfile = parse(source).expect("should parse");
    match &cookfile.recipes[0].steps[1] {
        Step::Test { step, .. } => assert!(matches!(step.body, Body::LuaBlock(_))),
        other => panic!("expected Test Lua, got {:?}", other),
    }
}

// ── §7.2 import path validation ────────────────────────────────────

#[test]
fn test_parse_import_rejects_dotdot_segment() {
    let src = "import bad ../sibling\nrecipe \"x\"\n";
    let result = crate::parse(src);
    assert!(result.is_err(), "expected parse error for '..' import path");
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("'..' segments are not permitted"),
        "expected diagnostic about '..', got: {msg}"
    );
}

#[test]
fn test_parse_import_rejects_embedded_dotdot() {
    let src = "import bad ./foo/../bar\nrecipe \"x\"\n";
    let result = crate::parse(src);
    assert!(result.is_err(), "expected parse error for embedded '..'");
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("'..' segments are not permitted"),
        "expected diagnostic about '..', got: {msg}"
    );
}

#[test]
fn test_parse_import_rejects_absolute_path() {
    let src = "import bad /tmp/x\nrecipe \"x\"\n";
    let result = crate::parse(src);
    assert!(result.is_err(), "expected parse error for absolute import path");
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("absolute paths are not permitted"),
        "expected diagnostic about absolute paths, got: {msg}"
    );
    assert!(
        msg.contains("tree-relative or '//' sigil"),
        "expected verbatim spec suffix, got: {msg}"
    );
}

#[test]
fn test_parse_import_accepts_sigil() {
    let src = "import core //core/lib\nrecipe \"x\"\n";
    let cookfile = crate::parse(src).expect("sigil import should parse");
    assert_eq!(cookfile.imports.len(), 1);
    match &cookfile.imports[0].path {
        ast::ImportPath::Sigil(s) => assert_eq!(s, "core/lib"),
        other => panic!("expected Sigil, got {:?}", other),
    }
}

#[test]
fn test_parse_import_rejects_sigil_with_dotdot() {
    let src = "import bad //../escape\nrecipe \"x\"\n";
    let result = crate::parse(src);
    assert!(result.is_err(), "expected parse error for '..' after sigil");
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("'..' segments are not permitted after '//'"),
        "expected sigil-dotdot diagnostic, got: {msg}"
    );
}

// ── CS-0033: 'env' reserved as recipe-name segment ────────────────

#[test]
fn rejects_recipe_with_env_first_segment() {
    let source = r#"recipe "env.foo"
end"#;
    let result = parse(source);
    assert!(result.is_err(), "expected parse error for env.foo recipe");
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("env") && err.contains("reserved"),
        "diagnostic must name 'env' and 'reserved'; got: {}",
        err
    );
}

#[test]
fn rejects_recipe_with_env_last_segment() {
    let source = r#"recipe "foo.env"
end"#;
    let result = parse(source);
    assert!(result.is_err(), "expected parse error for foo.env recipe");
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("env") && err.contains("reserved"),
        "diagnostic must name 'env' and 'reserved'; got: {}",
        err
    );
}

// ── CS-0035: stateful brace-balance for multi-line spans ──────────

#[test]
fn cs_0035_lua_block_with_multiline_long_string() {
    // A `}` byte inside a multi-line `[[ … ]]` long string MUST NOT close
    // the surrounding `>{ … }` Lua block. Pre-CS-0035, the line-local
    // brace counter saw the bare `}` as a closer.
    let source = "\
chore build
    >{
        local s = [[
            this string contains a } brace
            and another } here
        ]]
        print(s)
    }
";
    let result = parse(source).expect("CS-0035: long string should not close block");
    let step = &result.chores[0].steps[0];
    match step {
        Step::LuaBlock { code, .. } => {
            assert!(code.contains("local s = [["));
            assert!(code.contains("this string contains a } brace"));
            assert!(code.contains("and another } here"));
            assert!(code.contains("]]"));
            assert!(code.contains("print(s)"));
        }
        other => panic!("expected LuaBlock, got {:?}", other),
    }
}

#[test]
fn cs_0035_lua_block_with_multiline_long_string_levels() {
    // `[==[ … ]==]` long strings: closing `]]` of a lower level does not
    // close a higher-level open.
    let source = "\
chore build
    >{
        local s = [==[
            this is opaque text
            with } and ]] inside
        ]==]
        print(s)
    }
";
    let result = parse(source).expect("CS-0035: leveled long string should not close block");
    match &result.chores[0].steps[0] {
        Step::LuaBlock { code, .. } => {
            assert!(code.contains("with } and ]] inside"));
            assert!(code.contains("]==]"));
        }
        other => panic!("expected LuaBlock, got {:?}", other),
    }
}

#[test]
fn cs_0035_lua_block_with_multiline_block_comment() {
    // A `}` byte inside a multi-line `--[[ … ]]` block comment MUST NOT
    // close the surrounding `>{ … }` Lua block.
    let source = "\
chore build
    >{
        --[[
            here is a } in a block comment
            and another } here
        ]]
        local x = 1
    }
";
    let result = parse(source).expect("CS-0035: block comment should not close block");
    match &result.chores[0].steps[0] {
        Step::LuaBlock { code, .. } => {
            assert!(code.contains("--[["));
            assert!(code.contains("here is a } in a block comment"));
            assert!(code.contains("and another } here"));
            assert!(code.contains("local x = 1"));
        }
        other => panic!("expected LuaBlock, got {:?}", other),
    }
}

#[test]
fn cs_0035_shell_block_with_heredoc_brace_in_body() {
    // A `}` byte inside a shell heredoc body MUST NOT close the
    // surrounding `using { … }` shell block.
    let source = "\
recipe emit
    cook \"out.txt\" {
        cat <<EOF > out.txt
        a heredoc with a } brace
        and a } here too
        EOF
        echo done
    }
";
    let result = parse(source).expect("CS-0035: heredoc should not close shell block");
    match &result.recipes[0].steps[0] {
        Step::Cook { step, .. } => {
            let body = step.body.as_ref().expect("using clause");
            match body {
                Body::ShellBlock(lines) => {
                    assert_eq!(lines.len(), 5);
                    assert_eq!(lines[0], "cat <<EOF > out.txt");
                    assert_eq!(lines[1], "a heredoc with a } brace");
                    assert_eq!(lines[2], "and a } here too");
                    assert_eq!(lines[3], "EOF");
                    assert_eq!(lines[4], "echo done");
                }
                other => panic!("expected ShellBlock, got {:?}", other),
            }
        }
        other => panic!("expected Cook step, got {:?}", other),
    }
}

#[test]
fn cs_0035_shell_block_with_quoted_heredoc_delimiter() {
    let source = "\
recipe emit
    cook \"out.txt\" {
        cat <<'END' > out.txt
        } stays literal
        END
    }
";
    let result = parse(source).expect("CS-0035: quoted heredoc delim handled");
    match &result.recipes[0].steps[0] {
        Step::Cook { step, .. } => match step.body.as_ref().unwrap() {
            Body::ShellBlock(lines) => {
                assert_eq!(lines, &vec![
                    "cat <<'END' > out.txt".to_string(),
                    "} stays literal".to_string(),
                    "END".to_string(),
                ]);
            }
            other => panic!("expected ShellBlock, got {:?}", other),
        },
        other => panic!("expected Cook step, got {:?}", other),
    }
}

// ── CS-0135: `as`/`timeout`/`should_fail` removed as test-step modifiers ──

#[test]
fn test_step_rejects_as_after_timeout() {
    // Trailing content after a test body's `}` is always rejected; the first
    // trailing token drives the did-you-mean diagnostic (here `timeout`).
    let src = r#"
recipe r
    test { foo } timeout 30 as 'name'
"#;
    let err = parse(src).expect_err("must reject");
    assert!(
        err.to_string().contains("timeout was removed in v1.0"),
        "got: {err}"
    );
}

#[test]
fn test_step_rejects_should_fail_before_timeout() {
    let src = r#"
recipe r
    test { foo } should_fail timeout 30
"#;
    let err = parse(src).expect_err("must reject");
    assert!(err.to_string().contains("should_fail was removed in v1.0"));
}

#[test]
fn test_step_rejects_as_modifier() {
    let src = r#"
recipe r
    test { foo } as 'name'
"#;
    let err = parse(src).expect_err("must reject");
    assert!(err.to_string().contains("as was removed in v1.0"));
}

#[test]
fn test_step_rejects_timeout_modifier() {
    let src = r#"
recipe r
    test { foo } timeout 30
"#;
    let err = parse(src).expect_err("must reject");
    assert!(err.to_string().contains("timeout was removed in v1.0"));
}

// ── App. A.2: duplicate recipe / chore declaration name rule ───────
//
// Recipes and chores share a single callable namespace. Two declarations
// of either kind that share a name MUST be rejected at parse time.

#[test]
fn test_duplicate_recipe_name_rejected() {
    let src = "\
recipe foo
    cook.log(\"a\")

recipe foo
    cook.log(\"b\")
";
    let err = parse(src).expect_err("expected duplicate-recipe rejection").to_string();
    assert!(
        err.contains("recipe 'foo'") && err.contains("duplicate declaration") && err.contains("line 1"),
        "diagnostic should name the colliding name and the prior declaration line; got: {err}"
    );
}

#[test]
fn test_duplicate_chore_name_rejected() {
    let src = "\
chore tidy
    echo first

chore tidy
    echo second
";
    let err = parse(src).expect_err("expected duplicate-chore rejection").to_string();
    assert!(
        err.contains("chore 'tidy'") && err.contains("duplicate declaration") && err.contains("line 1"),
        "diagnostic should name the colliding name and the prior declaration line; got: {err}"
    );
}

#[test]
fn test_recipe_then_chore_same_name_rejected() {
    let src = "\
recipe play
    cook.log(\"r\")

chore play
    echo c
";
    let err = parse(src).expect_err("expected recipe-vs-chore collision rejection").to_string();
    // Second declaration is the chore; diagnostic names it as the duplicate
    // and points back at the recipe's line 1.
    assert!(
        err.contains("chore 'play'") && err.contains("recipe at line 1"),
        "diagnostic should identify the new chore and the prior recipe line; got: {err}"
    );
}

#[test]
fn test_chore_then_recipe_same_name_rejected() {
    let src = "\
chore play
    echo c

recipe play
    echo r
";
    let err = parse(src).expect_err("expected chore-vs-recipe collision rejection").to_string();
    assert!(
        err.contains("recipe 'play'") && err.contains("chore at line 1"),
        "diagnostic should identify the new recipe and the prior chore line; got: {err}"
    );
}

#[test]
fn test_duplicate_quoted_recipe_name_rejected() {
    // Quoted form on either side still collides — names compare verbatim.
    let src = "\
recipe \"build\"
    cook.log(\"a\")
recipe build
    cook.log(\"b\")
";
    let err = parse(src).expect_err("expected duplicate rejection across quote forms").to_string();
    assert!(
        err.contains("recipe 'build'") && err.contains("duplicate declaration"),
        "got: {err}"
    );
}

// ── Register block parsing (SHI-216 §3.7) ─────────────────────────────

#[test]
fn test_parse_empty_register_block() {
    let cookfile = parse("register\n").unwrap();
    assert_eq!(cookfile.register_blocks.len(), 1);
    assert_eq!(cookfile.register_blocks[0].body, "");
    assert_eq!(cookfile.register_blocks[0].line, 1);
}

#[test]
fn test_parse_register_block_with_body() {
    let source = "register\n    cook_cc.bin(\"game\", {})\n";
    let cookfile = parse(source).unwrap();
    assert_eq!(cookfile.register_blocks.len(), 1);
    assert!(cookfile.register_blocks[0].body.contains("cook_cc.bin"));
}

#[test]
fn test_parse_multiple_register_blocks() {
    let source = "register\n    local x = 1\n\nregister\n    local y = 2\n";
    let cookfile = parse(source).unwrap();
    assert_eq!(cookfile.register_blocks.len(), 2);
    assert!(cookfile.register_blocks[0].body.contains("local x"));
    assert!(cookfile.register_blocks[1].body.contains("local y"));
}

#[test]
fn test_parse_register_block_after_recipe_is_allowed() {
    let source = "recipe build\n    cook.log(\"x\")\n\nregister\n    cook_cc.bin(\"x\", {})\n";
    let cookfile = parse(source).unwrap();
    assert_eq!(cookfile.recipes.len(), 1);
    assert_eq!(cookfile.register_blocks.len(), 1);
}

#[test]
fn test_parse_register_block_interleaved() {
    let source = "register\n    a()\n\nrecipe build\n    cook.log(\"x\")\n\nregister\n    b()\n";
    let cookfile = parse(source).unwrap();
    assert_eq!(cookfile.register_blocks.len(), 2);
    assert_eq!(cookfile.recipes.len(), 1);
    assert!(cookfile.register_blocks[0].body.contains("a()"));
    assert!(cookfile.register_blocks[1].body.contains("b()"));
}

#[test]
fn test_parse_register_with_name_rejected() {
    let source = "register foo\n    a()\n";
    let err = parse(source).unwrap_err();
    let msg = format!("{}", err);
    assert!(msg.contains("register") && msg.contains("no name"),
        "expected 'register'+'no name' diagnostic, got: {}", msg);
}

#[test]
fn test_parse_register_terminates_recipe_body() {
    let source = "recipe build\n    cook.log(\"x\")\nregister\n    a()\n";
    let cookfile = parse(source).unwrap();
    assert_eq!(cookfile.recipes.len(), 1);
    assert_eq!(cookfile.recipes[0].steps.len(), 1);
    assert_eq!(cookfile.register_blocks.len(), 1);
}

// ── Top-level module_call dispatch (SHI-216 §3.7.5) ───────────────────

#[test]
fn test_parse_top_level_module_call_single_line() {
    let source = "use cook_cc\ncook_cc.bin(\"game\", {})\n";
    let cookfile = parse(source).unwrap();
    assert_eq!(cookfile.top_level_module_calls.len(), 1);
    assert!(cookfile.top_level_module_calls[0].code.starts_with("cook_cc.bin"));
}

#[test]
fn test_parse_top_level_module_call_multiline() {
    let source = "use cook_cc\ncook_cc.bin(\"game\", {\n    sources = { \"a.c\" },\n})\n";
    let cookfile = parse(source).unwrap();
    assert_eq!(cookfile.top_level_module_calls.len(), 1);
    let code = &cookfile.top_level_module_calls[0].code;
    assert!(code.contains("cook_cc.bin"));
    assert!(code.contains("sources = { \"a.c\" }"));
}

#[test]
fn test_parse_top_level_module_call_terminates_recipe() {
    let source = "use cook_cc\nrecipe build\n    cook.log(\"x\")\ncook_cc.bin(\"x\", {})\n";
    let cookfile = parse(source).unwrap();
    assert_eq!(cookfile.recipes.len(), 1);
    assert_eq!(cookfile.recipes[0].steps.len(), 1);
    assert_eq!(cookfile.top_level_module_calls.len(), 1);
}

#[test]
fn test_parse_top_level_colon_call_rejected() {
    let source = "use cook_cc\ncook_cc:bin(\"x\", {})\n";
    let err = parse(source).unwrap_err();
    let msg = format!("{}", err);
    assert!(msg.contains("unexpected content"),
        "expected top-level Content rejection, got: {}", msg);
}

// ── In-body module_call dispatch REMOVED (SHI-216 §3.9 amendment) ─────

#[test]
fn test_parse_in_body_bare_module_call_is_inline_lua_after_cs0134() {
    // CS-0134: an indented bare module call auto-classifies as register-phase
    // InlineLua (reverting the CS-0072 shell classification).
    let source = "use cpp\nrecipe build\n    cpp.bin(\"x\", { sources = { \"a.c\" } })\n";
    let cookfile = parse(source).unwrap();
    assert_eq!(cookfile.recipes.len(), 1);
    assert_eq!(cookfile.recipes[0].steps.len(), 1);
    use crate::ast::Step;
    assert!(
        matches!(cookfile.recipes[0].steps[0], Step::InlineLua { .. }),
        "expected InlineLua step (CS-0134), got: {:?}",
        cookfile.recipes[0].steps[0],
    );
}

#[test]
fn test_parse_in_body_register_sigil_rejected() {
    // CS-0134: the register-phase `>>` sigil was removed from the language.
    let source = "use cpp\nrecipe build\n    >> cpp.bin(\"x\", { sources = { \"a.c\" } })\n";
    let err = parse(source).unwrap_err();
    let msg = format!("{}", err);
    assert!(msg.contains("`>>` sigil was removed"), "got: {}", msg);
}

#[test]
fn test_parse_register_inside_recipe_body_rejected() {
    let source = "recipe build\n    register foo\n        cook_cc.bin(\"x\", {})\n";
    let err = parse(source).unwrap_err();
    let msg = format!("{}", err);
    assert!(msg.contains("register") && msg.contains("top-level only"),
        "expected '`register` blocks are top-level only' diagnostic, got: {}", msg);
}

#[test]
fn test_parse_register_bareword_indented_alone_is_shell() {
    // Indented bare `register` (no separator) is a shell_command "register"
    // per (post-CS-0072) rule 6 — exercised in a chore, since CS-0134 bans
    // loose shell in recipe bodies. The `register`+separator rejection rule
    // still fires (see test_parse_register_inside_chore_body_rejected).
    let source = "chore build\n    register\n";
    let cookfile = parse(source).unwrap();
    assert_eq!(cookfile.chores.len(), 1);
    assert_eq!(cookfile.chores[0].steps.len(), 1);
    use crate::ast::Step;
    match &cookfile.chores[0].steps[0] {
        Step::Shell { command, .. } => assert_eq!(command, "register"),
        other => panic!("expected Shell step, got {:?}", other),
    }
}

#[test]
fn test_parse_register_underscore_inside_body_is_shell() {
    // `register_foo` is not a separator-followed `register`; remains a
    // shell_command per rule 6 (exercised in a chore per CS-0134).
    let source = "chore build\n    register_foo\n";
    let cookfile = parse(source).unwrap();
    use crate::ast::Step;
    match &cookfile.chores[0].steps[0] {
        Step::Shell { command, .. } => assert_eq!(command, "register_foo"),
        other => panic!("expected Shell step, got {:?}", other),
    }
}

#[test]
fn test_parse_register_inside_chore_body_rejected() {
    let source = "chore clean\n    register foo\n";
    let err = parse(source).unwrap_err();
    let msg = format!("{}", err);
    assert!(msg.contains("register") && msg.contains("top-level only"),
        "expected diagnostic, got: {}", msg);
}

// ── COOK-63: for_each data-member iteration source (§8.3) ──────────

/// Helper: pull the first `Step::ForEach` out of recipe 0, or panic.
fn first_for_each(c: &Cookfile) -> &ForEachStep {
    c.recipes[0]
        .steps
        .iter()
        .find_map(|s| match s {
            Step::ForEach { step, .. } => Some(step),
            _ => None,
        })
        .expect("recipe should contain a for_each step")
}

#[test]
fn for_each_keyword_no_longer_recognized() {
    // `for_each` is gone; the line is no longer a valid declarative driver.
    let source = "recipe r\n    for_each cards\n    cook \"o\" { y }\n";
    assert!(parse(source).is_err());
}

// ── COOK-67 Task 4 / COOK-174: leading-kind producer typing ─────────

fn probe_of(src: &str) -> crate::ast::Probe {
    crate::parse(src).unwrap().probes.into_iter().next().unwrap()
}

#[test]
fn produce_shell_default_is_string() {
    let p = probe_of("probe x\n    { cat data.json }\n");
    assert!(matches!(p.produce, crate::ast::ProbeProduce::Shell {
        typing: crate::ast::ShellProduceType::String, .. }));
}

#[test]
fn produce_shell_json() {
    let p = probe_of("probe x\n    json { cat data.json }\n");
    assert!(matches!(p.produce, crate::ast::ProbeProduce::Shell {
        typing: crate::ast::ShellProduceType::Json, .. }));
}

#[test]
fn produce_shell_lines() {
    let p = probe_of("probe x\n    lines { git tag --list }\n");
    assert!(matches!(p.produce, crate::ast::ProbeProduce::Shell {
        typing: crate::ast::ShellProduceType::Lines, .. }));
}

#[test]
fn produce_json_on_lua_block_is_error() {
    let err = crate::parse("probe x\n    json >{ return {} }\n").unwrap_err();
    assert!(format!("{err}").contains("shell block"),
        "got: {err}");
}

// ── COOK-164 / COOK-174: tools / envs name-list producers ─────────────

#[test]
fn produce_tools_parses_name_list() {
    let cf = parse("probe toolchain\n    tools { cc, ld }\n").unwrap();
    let p = &cf.probes[0];
    assert_eq!(p.produce, crate::ast::ProbeProduce::Tools(vec!["cc".into(), "ld".into()]));
}

#[test]
fn produce_tools_accepts_whitespace_separators() {
    let cf = parse("probe toolchain\n    tools { cc ld   ar }\n").unwrap();
    let p = &cf.probes[0];
    assert_eq!(p.produce, crate::ast::ProbeProduce::Tools(vec!["cc".into(), "ld".into(), "ar".into()]));
}

#[test]
fn produce_envs_parses_name_list() {
    let cf = parse("probe sdk\n    envs { SDKROOT, CC }\n").unwrap();
    let p = &cf.probes[0];
    assert_eq!(p.produce, crate::ast::ProbeProduce::Envs(vec!["SDKROOT".into(), "CC".into()]));
}

#[test]
fn produce_tools_empty_list_is_error() {
    let err = parse("probe t\n    tools {  }\n").unwrap_err();
    assert!(format!("{err}").contains("at least one"), "got: {err}");
}

#[test]
fn produce_tools_lua_block_is_error() {
    let err = parse("probe t\n    tools >{ return {} }\n").unwrap_err();
    assert!(format!("{err}").contains("NAME LIST"), "got: {err}");
}

#[test]
fn produce_envs_invalid_name_is_error() {
    let err = parse("probe t\n    envs { 1bad }\n").unwrap_err();
    assert!(format!("{err}").contains("name"), "got: {err}");
}

// ── COOK-67 Task 3: probe declaration parser ────────────────────────

#[test]
fn parse_probe_lua_block_with_deps_and_ingredients() {
    let src = "probe services: cards services_raw\n    ingredients \"data/services.json\"\n    >{\n        return {}\n    }\n";
    let cf = crate::parse(src).unwrap();
    assert_eq!(cf.probes.len(), 1);
    let p = &cf.probes[0];
    assert_eq!(p.name, "services");
    assert_eq!(p.deps, vec!["cards", "services_raw"]);
    assert_eq!(p.ingredients, vec!["data/services.json"]);
    assert!(matches!(&p.produce, crate::ast::ProbeProduce::Lua(code) if code.contains("return {}")));
}

#[test]
fn parse_probe_terminates_at_next_recipe() {
    let src = "probe a\n    >{ return 1 }\nrecipe build\n    cook.log(\"hi\")\n";
    let cf = crate::parse(src).unwrap();
    assert_eq!(cf.probes.len(), 1);
    assert_eq!(cf.recipes.len(), 1);
    assert_eq!(cf.recipes[0].name, "build");
}

#[test]
fn parse_probe_shell_block_default() {
    let src = "probe cards\n    ingredients \"data/cards.json\"\n    { cat data/cards.json }\n";
    let cf = crate::parse(src).unwrap();
    let p = &cf.probes[0];
    assert!(matches!(&p.produce, crate::ast::ProbeProduce::Shell { typing: crate::ast::ShellProduceType::String, .. }));
}

#[test]
fn parse_probe_lua_inline_long_string_with_brace() {
    // a `}` inside a [[..]] long string must NOT prematurely close the block
    let src = "probe x\n    >{ return [[a}b]] }\n";
    let cf = crate::parse(src).unwrap();
    assert!(matches!(&cf.probes[0].produce,
        crate::ast::ProbeProduce::Lua(code) if code.contains("[[a}b]]")));
}

#[test]
fn parse_probe_lua_inline_long_string_no_brace_still_works() {
    let src = "probe x\n    >{ return cook.sh([[cat data]]) }\n";
    let cf = crate::parse(src).unwrap();
    assert!(matches!(&cf.probes[0].produce,
        crate::ast::ProbeProduce::Lua(code) if code.contains("cook.sh([[cat data]])")));
}

// ── COOK-67 Task 5: probe negative-case coverage ──────────────────────

fn parse_err(src: &str) -> String {
    format!("{}", crate::parse(src).unwrap_err())
}

#[test]
fn probe_missing_produce_rejected() {
    // Real message: "probe '{name}' has no producer"
    let msg = parse_err("probe x\n    ingredients \"a\"\n");
    assert!(msg.contains("no producer"), "got: {msg}");
}

#[test]
fn probe_two_produce_rejected() {
    // Real message: "probe: at most one producer per probe"
    let msg = parse_err("probe x\n    >{ return 1 }\n    >{ return 2 }\n");
    assert!(msg.contains("at most one producer"), "got: {msg}");
}

#[test]
fn probe_two_ingredients_rejected() {
    // Real message: "probe: at most one `ingredients` per probe"
    let msg = parse_err(
        "probe x\n    ingredients \"a\"\n    ingredients \"b\"\n    >{ return 1 }\n",
    );
    assert!(msg.contains("at most one `ingredients`"), "got: {msg}");
}

#[test]
fn probe_ingredients_after_produce_rejected() {
    // Real message: "probe: `ingredients` must appear before the producer"
    let msg = parse_err("probe x\n    >{ return 1 }\n    ingredients \"a\"\n");
    assert!(msg.contains("must appear before the producer"), "got: {msg}");
}

#[test]
fn probe_triple_colon_name_rejected_at_parse() {
    // MalformedProbeName from the lexer propagates through crate::parse.
    assert!(crate::parse("probe a:b:c\n    >{ return 1 }\n").is_err());
}

#[test]
fn probe_unexpected_step_rejected() {
    // A `cook "out" { true }` content line inside a probe body is parsed as the
    // producer; it is not a valid producer opener (`{`/`>{`/json/lines/...), so
    // it is rejected by the body-payload dispatch.
    let msg = parse_err("probe x\n    cook \"out\" { true }\n");
    assert!(
        msg.contains("shell block"),
        "got: {msg}"
    );
}

#[test]
fn probe_bare_lua_block_is_producer() {
    // COOK-174: a bare `>{ … }` line (no `produce` keyword) is the Lua producer.
    let p = probe_of("probe x\n    >{ return 1 }\n");
    assert!(matches!(&p.produce,
        crate::ast::ProbeProduce::Lua(code) if code.contains("return 1")));
}

#[test]
fn probe_dep_colon_vs_name_colon() {
    // `probe cards: cc:compiler` — name is "cards", dep is "cc:compiler".
    let cf = crate::parse("probe cards: cc:compiler\n    >{ return 1 }\n").unwrap();
    assert_eq!(cf.probes[0].name, "cards");
    assert_eq!(cf.probes[0].deps, vec!["cc:compiler"]);
}

#[test]
fn probe_json_on_lua_block_rejected() {
    // Real message: "probe: `json`/`lines` is only valid on a shell block; …"
    let msg = parse_err("probe x\n    json >{ return {} }\n");
    assert!(msg.contains("shell block"), "got: {msg}");
}

// ── COOK-88: ingredients <probe> member source ──────────────────────

#[test]
fn ingredients_probe_desugars_to_for_each() {
    let source = "recipe render\n    ingredients cardprobe\n    cook \"build/$<in.name>.png\" { gen \"$<in.name>\" $<out> }\n";
    let c = parse(source).unwrap();
    let fe = first_for_each(&c);
    assert_eq!(fe.source, ForEachSource::ProbeKey("cardprobe".to_string()));
    assert!(c.recipes[0].ingredients.is_empty());
}

#[test]
fn ingredients_probe_field_selector_parses() {
    let source = "recipe r\n    ingredients catalog:items\n    cook \"$<in.id>\" { x }\n";
    let c = parse(source).unwrap();
    assert_eq!(
        first_for_each(&c).source,
        ForEachSource::ProbeKey("catalog:items".to_string())
    );
}

#[test]
fn ingredients_two_segment_probe_key_parses_whole_ref() {
    // COOK-190: `ns:name` is the canonical probe naming; the ref must land
    // in the AST verbatim, not truncated at the colon.
    let source = "recipe stamps\n    ingredients cards:list\n    cook \"out/$<in>.stamp\" { echo \"$<in>\" > $<out> }\n";
    let c = parse(source).unwrap();
    assert_eq!(
        first_for_each(&c).source,
        ForEachSource::ProbeKey("cards:list".to_string())
    );
}

#[test]
fn ingredients_three_segment_ref_parses_whole_ref() {
    // Two-segment key + one `:field` selector = three segments, the maximum.
    let source = "recipe r\n    ingredients ns:name:items\n    cook \"$<in.id>\" { x }\n";
    let c = parse(source).unwrap();
    assert_eq!(
        first_for_each(&c).source,
        ForEachSource::ProbeKey("ns:name:items".to_string())
    );
}

#[test]
fn ingredients_four_segment_ref_rejected() {
    let msg = parse_err("recipe r\n    ingredients a:b:c:d\n    cook \"x\" { y }\n");
    assert!(msg.contains("malformed probe ref"), "got: {msg}");
}

#[test]
fn ingredients_trailing_colon_ref_rejected() {
    let msg = parse_err("recipe r\n    ingredients cards:\n    cook \"x\" { y }\n");
    assert!(msg.contains("malformed probe ref"), "got: {msg}");
}

#[test]
fn ingredients_leading_colon_ref_rejected() {
    let msg = parse_err("recipe r\n    ingredients :cards\n    cook \"x\" { y }\n");
    assert!(msg.contains("malformed probe ref"), "got: {msg}");
}

#[test]
fn ingredients_mixing_glob_then_probe_is_rejected() {
    let source = "recipe r\n    ingredients \"a.json\"\n    ingredients cardprobe\n    cook \"x\" { y }\n";
    let err = parse(source).unwrap_err();
    assert!(format!("{err:?}").contains("mix"));
}

#[test]
fn ingredients_probe_with_trailing_content_is_rejected() {
    let source = "recipe r\n    ingredients cardprobe extra\n    cook \"x\" { y }\n";
    let err = parse(source).unwrap_err();
    assert!(format!("{err:?}").contains("trailing"));
}

#[test]
fn ingredients_probe_then_glob_is_rejected() {
    let source = "recipe r\n    ingredients cardprobe\n    ingredients \"*.c\"\n    cook \"x\" { y }\n";
    assert!(parse(source).is_err());
}

#[test]
fn ingredients_probe_declared_twice_is_rejected() {
    let source = "recipe r\n    ingredients cardprobe\n    ingredients other\n    cook \"x\" { y }\n";
    assert!(parse(source).is_err());
}


// ── CS-0099: the `using` keyword is removed; the body opener follows the
// output pattern(s) directly: `cook "out" { … }` / `cook "out" >{ … }`.

#[test]
fn cs0099_cook_shell_block_one_line() {
    let src = "recipe build\n    cook \"build/{in.stem}.o\" { gcc -c {in} -o {out} }\n";
    let result = parse(src).unwrap();
    match &result.recipes[0].steps[0] {
        Step::Cook { step, .. } => {
            assert_eq!(step.outputs[0].as_str(), "build/{in.stem}.o");
            assert_eq!(
                step.body,
                Some(Body::ShellBlock(vec!["gcc -c {in} -o {out}".to_string()]))
            );
        }
        other => panic!("expected Cook step, got {:?}", other),
    }
}

#[test]
fn cs0099_cook_shell_block_multiline() {
    let src = "recipe \"lib\"\n    ingredients \"lib/*.c\"\n    cook \"build/lib.a\" {\n        ar rcs {out} {all}\n    }\n";
    let result = parse(src).unwrap();
    match &result.recipes[0].steps[0] {
        Step::Cook { step, .. } => {
            assert_eq!(
                step.body,
                Some(Body::ShellBlock(vec!["ar rcs {out} {all}".to_string()]))
            );
        }
        other => panic!("expected Cook, got {:?}", other),
    }
}

#[test]
fn cs0099_cook_lua_block() {
    let src = "recipe \"lib\"\n    ingredients \"lib/*.c\"\n    cook \"build/obj/{stem}.o\" >{\n        cook.sh(\"gcc -c \" .. input .. \" -o \" .. output)\n    }\n";
    let result = parse(src).unwrap();
    match &result.recipes[0].steps[0] {
        Step::Cook { step, .. } => match &step.body {
            Some(Body::LuaBlock(code)) => {
                assert!(code.contains("cook.sh"), "code was: {}", code);
            }
            other => panic!("expected LuaBlock, got {:?}", other),
        },
        other => panic!("expected Cook, got {:?}", other),
    }
}

#[test]
fn cs0099_cook_multi_output_lua_block() {
    let src = "recipe \"wasm\"\n    ingredients \"src/*.rs\"\n    cook \"a.js\" \"b.wasm\" >{\n        sh(\"cmd\")\n    }\n";
    let result = parse(src).unwrap();
    match &result.recipes[0].steps[0] {
        Step::Cook { step, .. } => {
            assert_eq!(step.outputs.len(), 2);
            assert!(matches!(step.body, Some(crate::ast::Body::LuaBlock(_))));
        }
        other => panic!("expected Cook, got {:?}", other),
    }
}

#[test]
fn cs0099_cook_multiline_patterns_then_body() {
    // CS-0078 continuation lines still terminate at the body opener.
    let src = "recipe r\n    cook \"a.o\"\n         \"b.o\"\n         \"c.o\" { link {all} }\n";
    let result = parse(src).unwrap();
    match &result.recipes[0].steps[0] {
        Step::Cook { step, .. } => {
            assert_eq!(step.outputs.len(), 3);
            assert_eq!(
                step.body,
                Some(Body::ShellBlock(vec!["link {all}".to_string()]))
            );
        }
        other => panic!("expected Cook, got {:?}", other),
    }
}

#[test]
fn cs0099_cook_lua_expr_output_with_body() {
    let src = "recipe r\n    cook (\"build/\" .. name) >{ return 1 }\n";
    let result = parse(src).unwrap();
    match &result.recipes[0].steps[0] {
        Step::Cook { step, .. } => {
            assert!(matches!(step.outputs[0], OutputPattern::LuaExpr(_)));
            assert!(matches!(step.body, Some(Body::LuaBlock(_))));
        }
        other => panic!("expected Cook, got {:?}", other),
    }
}

#[test]
fn cs0133_cook_no_body_rejected_bare() {
    // CS-0133 supersedes the old CS-0099 "declaration-only still parses"
    // guarantee: a body is now mandatory.
    let src = "recipe \"build\"\n    cook \"bin/app\"\n    gcc src/main.c -o bin/app\n";
    let err = parse(src).expect_err("body-less cook must be rejected");
    assert!(err.to_string().contains("declaration-only cook steps were removed"), "got: {}", err);
}

#[test]
fn cs0099_using_keyword_rejected_with_migration_diagnostic() {
    let src = "recipe r\n    cook \"out\" using { echo hi }\n";
    let err = parse(src).expect_err("the using keyword must be rejected");
    let msg = err.to_string();
    assert!(msg.contains("not supported"), "got: {}", msg);
    assert!(msg.contains("using"), "got: {}", msg);
}

#[test]
fn cs0099_using_lua_form_rejected_with_migration_diagnostic() {
    let src = "recipe r\n    cook \"out\" using >{ return 1 }\n";
    let err = parse(src).expect_err("the using keyword must be rejected");
    let msg = err.to_string();
    assert!(msg.contains("not supported"), "got: {}", msg);
}

#[test]
fn cs0099_lua_expr_using_rejected_with_migration_diagnostic() {
    let src = "recipe r\n    cook (\"x\" .. \"y\") using >{ return 1 }\n";
    let err = parse(src).expect_err("the using keyword must be rejected");
    let msg = err.to_string();
    assert!(msg.contains("not supported"), "got: {}", msg);
}

#[test]
fn cs0099_register_block_body_still_rejected() {
    // App. A.4: `>>{` is not a valid cook body, with or without `using`.
    let src = "recipe \"r\"\n    cook \"out\" >>{\n        cook.add_unit({command = \"x\"})\n    }\n";
    let err = parse(src).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains(">{"), "got: {}", msg);
}

// ── COOK-171: cook-step disposition surface (§8.4.3) ───────────────

/// Helper: extract the first cook step's disposition from a parsed recipe.
#[cfg(test)]
fn first_cook_disposition(cf: &Cookfile) -> &crate::ast::Disposition {
    for step in &cf.recipes[0].steps {
        if let crate::ast::Step::Cook { step, .. } = step {
            return &step.disposition;
        }
    }
    panic!("no cook step found");
}

#[test]
fn disp_recipe_seal_applies_to_cook() {
    let cf = parse("recipe build\n    seal host\n    cook \"x.o\" { cc -c x.c }\n").unwrap();
    let d = first_cook_disposition(&cf);
    assert!(d.seal.contains("host"));
    assert_eq!(d.sharing, cook_contracts::Sharing::Shared);
    assert!(!d.record);
}

#[test]
fn disp_recipe_seal_rejects_test_only_recipe() {
    let err = parse("recipe verify\n    seal host\n    test { true }\n")
        .expect_err("a recipe-level seal requires a cook unit");
    match err {
        ParseError::Parse { line, message } => {
            assert_eq!(line, 1);
            assert!(
                message.contains("seal on recipe verify: no cook units to apply to"),
                "got: {message}"
            );
        }
        e => panic!("expected ParseError::Parse, got {e:?}"),
    }
}

#[test]
fn disp_recipe_seal_rejects_otherwise_empty_recipe() {
    let err = parse("recipe package\n    seal host\n")
        .expect_err("a recipe-level seal requires a cook unit");
    match err {
        ParseError::Parse { line, message } => {
            assert_eq!(line, 1);
            assert!(
                message.contains("seal on recipe package: no cook units to apply to"),
                "got: {message}"
            );
        }
        e => panic!("expected ParseError::Parse, got {e:?}"),
    }
}

#[test]
fn disp_recipe_seal_order_independent() {
    // a recipe-level `seal` AFTER the cook still applies (declarative scope)
    let cf = parse("recipe build\n    cook \"x.o\" { cc -c x.c }\n    seal host\n").unwrap();
    assert!(first_cook_disposition(&cf).seal.contains("host"));
}

#[test]
fn disp_recipe_seal_stacks_additively() {
    let cf = parse("recipe build\n    seal host\n    seal gpu driver\n    cook \"x.o\" { cc }\n").unwrap();
    let got: Vec<&str> = first_cook_disposition(&cf).seal.iter().map(|s| s.as_str()).collect();
    assert_eq!(got, vec!["driver", "gpu", "host"]); // sorted, deduped
}

#[test]
fn disp_trailing_seal_unseal() {
    // base {a,b} ∪ trailing {c} − trailing unseal {a} = {b,c}
    let cf = parse("recipe build\n    seal a b\n    cook \"x.o\" { cc } unseal a seal c\n").unwrap();
    let got: Vec<&str> = first_cook_disposition(&cf).seal.iter().map(|s| s.as_str()).collect();
    assert_eq!(got, vec!["b", "c"]);
}

#[test]
fn disp_trailing_local_pinned_nondet() {
    assert_eq!(
        first_cook_disposition(&parse("recipe r\n    cook \"x\" { c } local\n").unwrap()).sharing,
        cook_contracts::Sharing::Local
    );
    assert_eq!(
        first_cook_disposition(&parse("recipe r\n    cook \"x\" { c } pinned\n").unwrap()).sharing,
        cook_contracts::Sharing::Pinned
    );
    assert!(first_cook_disposition(&parse("recipe r\n    cook \"x\" { c } nondet\n").unwrap()).record);
}

#[test]
fn disp_trailing_seal_then_share_mod() {
    let cf = parse("recipe r\n    cook \"x\" { c } seal rev local\n").unwrap();
    let d = first_cook_disposition(&cf);
    assert!(d.seal.contains("rev"));
    assert_eq!(d.sharing, cook_contracts::Sharing::Local);
}

#[test]
fn cs0133_body_less_cook_with_trailing_mod_rejected() {
    // CS-0133: a clean cook_mods tail (`local`) no longer keeps a body-less
    // cook alive — the modifier annotated a unit that never registered.
    let err = parse("recipe r\n    cook \"x\" local\n").expect_err("body-less cook + mod must be rejected");
    assert!(err.to_string().contains("declaration-only cook steps were removed"), "got: {}", err);
}

#[test]
fn disp_multi_output_unit_one_disposition() {
    // multiple outputs in one cook unit share the one trailing disposition
    let cf = parse("recipe r\n    cook \"a\" \"b\" { c } nondet\n").unwrap();
    let d = first_cook_disposition(&cf);
    assert!(d.record);
}

#[test]
fn disp_unannotated_cook_has_default_disposition() {
    let cf = parse("recipe build\n    cook \"x.o\" { cc -c x.c }\n").unwrap();
    assert_eq!(*first_cook_disposition(&cf), crate::ast::Disposition::default());
}

// negative paths

#[test]
fn disp_bare_recipe_seal_rejected() {
    assert!(parse("recipe r\n    seal\n    cook \"x\" { c }\n").is_err());
}

#[test]
fn disp_recipe_unseal_rejected() {
    let e = parse("recipe r\n    unseal a\n    cook \"x\" { c }\n").unwrap_err();
    assert!(format!("{e:?}").contains("trailing modifier"));
}

#[test]
fn disp_two_share_mods_rejected() {
    assert!(parse("recipe r\n    cook \"x\" { c } local pinned\n").is_err());
    assert!(parse("recipe r\n    cook \"x\" { c } nondet local\n").is_err());
}

#[test]
fn disp_content_after_share_mod_rejected() {
    assert!(parse("recipe r\n    cook \"x\" { c } local seal a\n").is_err());
}

#[test]
fn disp_record_keyword_rejected_with_hint() {
    let e = parse("recipe r\n    cook \"x\" { c } record\n").unwrap_err();
    assert!(format!("{e:?}").contains("nondet"));
}

#[test]
fn disp_bare_trailing_seal_rejected() {
    assert!(parse("recipe r\n    cook \"x\" { c } seal\n").is_err());
    assert!(parse("recipe r\n    cook \"x\" { c } unseal\n").is_err());
}

#[test]
fn disp_trailing_seal_quoted_ref_errors() {
    assert!(parse("recipe r\n    cook \"x\" { c } seal \"host\"\n").is_err());
}

#[test]
fn disp_trailing_seal_triple_colon_ref_errors() {
    assert!(parse("recipe r\n    cook \"x\" { c } seal a:b:c\n").is_err());
}

#[test]
fn disp_recipe_seal_quoted_ref_errors() {
    assert!(parse("recipe r\n    seal \"host\"\n    cook \"x\" { c }\n").is_err());
}

#[test]
fn disp_bare_local_line_is_rejected_loose_shell() {
    // CS-0134: recipe-body `local` on its own line is not a disposition and not
    // a module call, so it is rejected as loose shell.
    let err = parse("recipe r\n    cook \"x\" { c }\n    local\n").unwrap_err();
    let msg = format!("{}", err);
    assert!(msg.contains("loose shell commands are not allowed"), "got: {}", msg);
}

#[test]
fn disp_as_modifier_on_cook_rejected() {
    let e = parse("recipe r\n    cook \"x\" { c } as 'name'\n").unwrap_err();
    assert!(format!("{e:?}").contains("removed in v1.0"));
}

// ── COOK-191 Task 3: config-block bare `NAME "value"` did-you-mean (CS-0126) ──

#[test]
fn test_config_bare_value_quoted_gets_did_you_mean() {
    let src = "config\n    OUTDIR \"build\"\n\nrecipe hello\n    echo hi\n";
    let err = parse(src).unwrap_err();
    let msg = format!("{}", err);
    assert!(msg.contains("line 2"), "got: {msg}");
    assert!(msg.contains("config values are Lua assignments"), "got: {msg}");
    assert!(msg.contains("OUTDIR = \"build\""), "got: {msg}");
}

#[test]
fn test_config_bare_value_unquoted_gets_did_you_mean() {
    let src = "config\n    CC gcc\n\nrecipe hello\n    echo hi\n";
    let err = parse(src).unwrap_err();
    let msg = format!("{}", err);
    assert!(msg.contains("did you mean CC = \"gcc\""), "got: {msg}");
}

#[test]
fn test_config_valid_lua_still_parses() {
    for body in [
        "    env.CC = os.getenv(\"CC\") or \"cc\"",
        "    local x = 1",
        "    OUTDIR = \"build\"",
        "    if true then env.A = \"1\" end",
    ] {
        let src = format!("config\n{body}\n\nrecipe hello\n    cook.log(\"hi\")\n");
        assert!(parse(&src).is_ok(), "should parse: {body}");
    }
}
