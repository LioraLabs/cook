use super::*;

// ── SHI-71: Unquoted recipe names ──────────────────────────────────

#[test]
fn test_bare_recipe_name_parses() {
    let source = "recipe build\n    echo hello\n";
    let result = parse(source).unwrap();
    assert_eq!(result.recipes[0].name, "build");
}

#[test]
fn test_bare_recipe_name_with_deps() {
    let source = "recipe build: lib setup\n    echo hello\n";
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
    let source = "use cpp\n\nrecipe build\n    echo hello\n";
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

// ── SHI-73 / CS-0072: Module calls in recipe bodies ────────────────
//
// CS-0072 (Cook Standard v0.9) removes the in-body module_call dispatch.
// A bare `<id>.<id>(...)` line inside a recipe body now classifies as
// shell_command (Step::Shell), not InlineLua. Authors must use `>>` for
// register-phase Lua inside a recipe body.

#[test]
fn test_module_call_single_line_is_shell_after_cs0072() {
    // CS-0072: bare module-call in recipe body becomes Shell, not InlineLua.
    let source = "recipe build\n    cpp.compile(\"src/*.cpp\")\n";
    let result = parse(source).unwrap();
    assert_eq!(result.recipes[0].steps.len(), 1);
    match &result.recipes[0].steps[0] {
        Step::Shell { command, .. } => {
            assert_eq!(command, "cpp.compile(\"src/*.cpp\")");
        }
        other => panic!("expected Shell step (CS-0072), got {:?}", other),
    }
}

#[test]
fn test_module_call_multiline_is_multiple_shell_after_cs0072() {
    // CS-0072: a bare module-call spanning multiple lines in a recipe body
    // becomes individual Shell steps (one per content line) since the
    // multi-line collection only runs for top-level module calls now.
    let source = "recipe build\n    cpp.compile(\"src/*.cpp\")\n    echo done\n";
    let result = parse(source).unwrap();
    assert_eq!(result.recipes[0].steps.len(), 2);
    assert!(matches!(&result.recipes[0].steps[0], Step::Shell { .. }));
    assert!(matches!(&result.recipes[0].steps[1], Step::Shell { .. }));
}

#[test]
fn test_non_module_dot_is_shell() {
    // A line starting with `.` is not a module call
    let source = "recipe build\n    ./run.sh\n";
    let result = parse(source).unwrap();
    assert!(matches!(&result.recipes[0].steps[0], Step::Shell { .. }));
}

#[test]
fn test_module_call_no_args_is_shell_after_cs0072() {
    // CS-0072: bare module-call with no args in recipe body becomes Shell.
    let source = "recipe build\n    cpp.detect_compiler()\n";
    let result = parse(source).unwrap();
    match &result.recipes[0].steps[0] {
        Step::Shell { command, .. } => {
            assert_eq!(command, "cpp.detect_compiler()");
        }
        other => panic!("expected Shell step (CS-0072), got {:?}", other),
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
    let source = "recipe \"build\"\n    gcc -o main main.c\n";
    let result = parse(source).unwrap();
    assert_eq!(result.recipes.len(), 1);
    assert_eq!(result.recipes[0].name, "build");
    assert!(result.recipes[0].deps.is_empty());
    assert!(result.recipes[0].ingredients.is_empty());
    assert_eq!(result.recipes[0].steps.len(), 1);
    assert_eq!(
        result.recipes[0].steps[0],
        Step::Shell {
            command: "gcc -o main main.c".to_string(),
            line: 2,
            interactive: false,
        }
    );
}

#[test]
fn test_recipe_with_deps() {
    let source = "recipe \"build\": \"setup\" \"lib\"\n    echo building\n";
    let result = parse(source).unwrap();
    let recipe = &result.recipes[0];
    assert_eq!(recipe.deps, vec!["setup".to_string(), "lib".to_string()]);
}

#[test]
fn test_recipe_with_ingredients() {
    let source = "recipe \"lib\"\n    ingredients \"lib/*.c\" \"include/*.h\"\n    echo compiling\n";
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
fn test_cook_step_declaration_only() {
    let source = "recipe \"build\"\n    ingredients \"src/*.c\"\n    cook \"bin/app\"\n    gcc src/main.c -o bin/app\n";
    let result = parse(source).unwrap();
    let recipe = &result.recipes[0];
    assert_eq!(recipe.steps.len(), 2);
    match &recipe.steps[0] {
        Step::Cook { step, .. } => {
            assert_eq!(step.outputs[0].as_str(), "bin/app");
            assert!(step.body.is_none());
        }
        other => panic!("expected Cook, got {:?}", other),
    }
    assert!(matches!(&recipe.steps[1], Step::Shell { .. }));
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
fn test_plate_step() {
    let source = "recipe test_recipe\n    ingredients \"tests/*.c\"\n    cook \"build/{in.stem}\" {\n        cc {in} -o {out}\n    }\n    plate {\n        ./{in}\n    }\n";
    let cookfile = parse(source).expect("should parse");
    let recipe = &cookfile.recipes[0];
    assert_eq!(recipe.steps.len(), 2);
    match &recipe.steps[1] {
        Step::Plate { step, .. } => match &step.body {
            Body::ShellBlock(lines) => {
                assert_eq!(lines.len(), 1);
                assert_eq!(lines[0].trim(), "./{in}");
            }
            other => panic!("expected ShellBlock, got {:?}", other),
        },
        other => panic!("expected Plate step, got {:?}", other),
    }
}

#[test]
fn test_mixed_steps() {
    // Note 4.4.2 region rule: imperative-region steps (> shell @) must come
    // after all declarative-region steps. The middle step here uses `>>`
    // (register-phase InlineLua) so it can sit between two cook steps.
    let source = r#"recipe "lib": "setup"
    ingredients "lib/*.c" "include/*.h"
    cook "build/obj/{stem}.o" {
        gcc -c {in} -o {out}
    }
    >> print("compiled")
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
fn test_imperative_then_declarative_rejected() {
    // App. A.3 "Region ordering rule": once the imperative region begins,
    // no declarative-region step may follow.
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
    assert!(msg.contains("imperative region"), "got: {}", msg);
}

#[test]
fn test_inline_lua_line_register_phase() {
    let source = "recipe \"r\"\n    >> local x = 1\n    >>{\n        cook.env.K = \"v\"\n    }\n";
    let result = parse(source).unwrap();
    let recipe = &result.recipes[0];
    assert_eq!(recipe.steps.len(), 2);
    assert!(matches!(&recipe.steps[0], Step::InlineLua { .. }));
    assert!(matches!(&recipe.steps[1], Step::InlineLuaBlock { .. }));
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
    let source = "recipe \"clean\"\n    rm -rf build bin\n";
    let result = parse(source).unwrap();
    let recipe = &result.recipes[0];
    assert!(recipe.deps.is_empty());
    assert!(recipe.ingredients.is_empty());
    assert_eq!(recipe.steps.len(), 1);
}

#[test]
fn test_multiple_recipes() {
    let source = "recipe \"setup\"\n    mkdir -p build\n\nrecipe \"build\": \"setup\"\n    echo building\n";
    let result = parse(source).unwrap();
    assert_eq!(result.recipes.len(), 2);
    assert_eq!(result.recipes[0].name, "setup");
    assert_eq!(result.recipes[1].name, "build");
    assert_eq!(result.recipes[1].deps, vec!["setup".to_string()]);
}

#[test]
fn test_lua_block_in_recipe() {
    let source = "recipe \"build\"\n>{\n    local x = 1\n    print(x)\n}\n";
    let result = parse(source).unwrap();
    assert_eq!(result.recipes[0].steps.len(), 1);
    assert!(matches!(&result.recipes[0].steps[0], Step::LuaBlock { .. }));
}

#[test]
fn test_lua_block_nested_braces() {
    let source = "recipe \"build\"\n>{\n    if true then\n        local t = {1, 2, 3}\n    end\n}\n";
    let result = parse(source).unwrap();
    match &result.recipes[0].steps[0] {
        Step::LuaBlock { code, .. } => {
            assert!(code.contains("local t = {1, 2, 3}"));
        }
        other => panic!("expected LuaBlock, got {:?}", other),
    }
}

#[test]
fn test_comments_and_blanks_skipped() {
    let source = "recipe \"build\"\n    # comment\n    gcc -o main main.c\n";
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
    let source = "recipe \"build\"\n>{\n    local s = \"}\"\n    print(s)\n}\n";
    let result = parse(source).unwrap();
    match &result.recipes[0].steps[0] {
        Step::LuaBlock { code, .. } => {
            assert!(code.contains("local s = \"}\""));
        }
        other => panic!("expected LuaBlock, got {:?}", other),
    }
}

#[test]
fn test_lua_block_brace_in_comment() {
    let source = "recipe \"build\"\n>{\n    local x = 1 -- }\n    print(x)\n}\n";
    let result = parse(source).unwrap();
    match &result.recipes[0].steps[0] {
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
    echo hello
"#;
    let result = parse(source).unwrap();
    assert_eq!(result.config_blocks.len(), 1);
    assert_eq!(result.config_blocks[0].body, "");
}

#[test]
fn test_indented_quoted_pair_is_shell_command() {
    let source = r#"recipe "build"
    CC "gcc"
"#;
    let result = parse(source).unwrap();
    assert_eq!(result.recipes.len(), 1);
    assert!(matches!(
        &result.recipes[0].steps[0],
        Step::Shell { command, .. } if command.contains("CC")
    ));
}

#[test]
fn test_config_after_recipe_errors() {
    let source = r#"recipe "build"
    echo hello

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
fn test_interactive_shell_step() {
    let source = "recipe \"run\"\n    @./bin/app\n";
    let result = parse(source).unwrap();
    let step = &result.recipes[0].steps[0];
    match step {
        Step::Shell { command, interactive, .. } => {
            assert!(interactive, "expected interactive=true");
            assert_eq!(command, "./bin/app", "@ should be stripped from command");
        }
        other => panic!("expected Shell step, got {:?}", other),
    }
}

#[test]
fn test_non_interactive_shell_step() {
    let source = "recipe \"build\"\n    echo hello\n";
    let result = parse(source).unwrap();
    match &result.recipes[0].steps[0] {
        Step::Shell { interactive, .. } => {
            assert!(!interactive, "expected interactive=false for normal shell step");
        }
        other => panic!("expected Shell step, got {:?}", other),
    }
}

#[test]
fn test_empty_interactive_step_errors() {
    let source = "recipe \"run\"\n    @\n";
    let result = parse(source);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("requires a command"), "got: {err}");
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
    let source = "use \"cpp\"\n\nrecipe \"build\"\n    echo hello\n";
    let cookfile = crate::parse(source).unwrap();
    assert_eq!(cookfile.uses.len(), 1);
    assert_eq!(cookfile.uses[0].module_name, "cpp");
    assert_eq!(cookfile.uses[0].line, 1);
    assert_eq!(cookfile.recipes.len(), 1);
}

#[test]
fn test_parse_multiple_use_statements() {
    let source = "use \"cpp\"\nuse \"proto\"\n\nrecipe \"build\"\n    echo hello\n";
    let cookfile = crate::parse(source).unwrap();
    assert_eq!(cookfile.uses.len(), 2);
    assert_eq!(cookfile.uses[0].module_name, "cpp");
    assert_eq!(cookfile.uses[1].module_name, "proto");
}

#[test]
fn test_parse_use_with_configs() {
    let source = "use \"cpp\"\n\nconfig \"debug\"\n    env.CFLAGS = \"-g\"\n\nrecipe \"build\"\n    @echo hello\n";
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
            assert!(!step.should_fail);
            assert_eq!(step.timeout, None);
        }
        other => panic!("expected Test, got {:?}", other),
    }
}

#[test]
fn test_test_step_with_timeout() {
    let source = "recipe r\n    ingredients \"tests/*.c\"\n    cook \"build/{in.stem}\" { cc {in} -o {out} }\n    test { ./{in} } timeout 60\n";
    let cookfile = parse(source).expect("should parse");
    match &cookfile.recipes[0].steps[1] {
        Step::Test { step, .. } => {
            assert!(matches!(step.body, Body::ShellBlock(_)));
            assert!(!step.should_fail);
            assert_eq!(step.timeout, Some(60));
        }
        other => panic!("expected Test, got {:?}", other),
    }
}

#[test]
fn test_test_step_with_should_fail() {
    let source = "recipe r\n    ingredients \"tests/*.c\"\n    cook \"build/{in.stem}\" { cc {in} -o {out} }\n    test { ./{in} } should_fail\n";
    let cookfile = parse(source).expect("should parse");
    match &cookfile.recipes[0].steps[1] {
        Step::Test { step, .. } => {
            assert!(matches!(step.body, Body::ShellBlock(_)));
            assert!(step.should_fail);
            assert_eq!(step.timeout, None);
        }
        other => panic!("expected Test, got {:?}", other),
    }
}

#[test]
fn test_test_step_with_timeout_and_should_fail() {
    let source = "recipe r\n    ingredients \"tests/*.c\"\n    cook \"build/{in.stem}\" { cc {in} -o {out} }\n    test { ./{in} } timeout 60 should_fail\n";
    let cookfile = parse(source).expect("should parse");
    match &cookfile.recipes[0].steps[1] {
        Step::Test { step, .. } => {
            assert!(matches!(step.body, Body::ShellBlock(_)));
            assert!(step.should_fail);
            assert_eq!(step.timeout, Some(60));
        }
        other => panic!("expected Test, got {:?}", other),
    }
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
    echo compiling
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
    echo compiling
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
    echo compiling
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
fn test_chore_at_prefix_no_op() {
    let input = "chore deploy\n    @rsync -av out/\n";
    let cookfile = parse(input).expect("chore should parse");
    match &cookfile.chores[0].steps[0] {
        Step::Shell { command, interactive, .. } => {
            // `@` is stripped (preserving symmetry with recipe bodies);
            // chore default-interactive remains.
            assert_eq!(command, "rsync -av out/");
            assert!(*interactive);
        }
        _ => panic!("expected Shell step"),
    }
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
fn test_chore_with_plate_rejected() {
    let input = "chore deploy\n    plate { ./{in} }\n";
    assert!(parse(input).is_err());
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
    let input = "chore clean\n    rm -rf build\nrecipe build\n    cook \"out\"\n";
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
    assert!(msg.contains("CS-0099"), "expected CS-0099 migration diagnostic, got: {}", msg);
}

#[test]
fn test_using_string_form_rejected_with_migration_diagnostic() {
    let src = r#"recipe build
    cook "out" using "echo hi"
"#;
    let err = parse(src).expect_err("CS-0099: the using keyword must be rejected");
    match err {
        ParseError::Parse { message, .. } => {
            assert!(message.contains("CS-0099"), "diagnostic should name CS-0099, got: {message}");
            assert!(message.contains("removed"), "diagnostic should say the keyword was removed, got: {message}");
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
    let src = "recipe build\n    cook \"build/app\" { gcc main.c -o {out} }\n    plate { ./{in} }\n";
    let cookfile = parse(src).expect("should parse");
    assert_eq!(cookfile.recipes[0].steps.len(), 2, "should have cook + plate");
    assert!(matches!(&cookfile.recipes[0].steps[0], Step::Cook { .. }));
    assert!(matches!(&cookfile.recipes[0].steps[1], Step::Plate { step, .. } if matches!(step.body, Body::ShellBlock(_))));
}

#[test]
fn test_plate_string_form_rejected() {
    let source = "recipe r\n    ingredients \"tests/*.c\"\n    cook \"build/{in.stem}\" { cc {in} -o {out} }\n    plate \"./{out}\"\n";
    let err = parse(source).unwrap_err();
    let msg = format!("{}", err);
    assert!(
        msg.contains("plate") && msg.contains("CS-0024") && msg.contains("{ cmd }"),
        "expected migration diagnostic for plate string form, got: {}",
        msg
    );
}

#[test]
fn test_test_string_form_rejected() {
    let source = "recipe r\n    ingredients \"tests/*.c\"\n    cook \"build/{in.stem}\" { cc {in} -o {out} }\n    test \"./{out}\" timeout 60\n";
    let err = parse(source).unwrap_err();
    let msg = format!("{}", err);
    assert!(
        msg.contains("test") && msg.contains("CS-0024") && msg.contains("{ cmd }"),
        "expected migration diagnostic for test string form, got: {}",
        msg
    );
}

#[test]
fn test_plate_lua_block_parses() {
    let source = "recipe r\n    ingredients \"src/*.c\"\n    cook \"build/{in.stem}\" { cc {in} -o {out} }\n    plate >{\n        cook.sh(\"strip \" .. input)\n    }\n";
    let cookfile = parse(source).expect("should parse");
    match &cookfile.recipes[0].steps[1] {
        Step::Plate { step, .. } => assert!(matches!(step.body, Body::LuaBlock(_))),
        other => panic!("expected Plate Lua, got {:?}", other),
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
recipe build
    >{
        local s = [[
            this string contains a } brace
            and another } here
        ]]
        print(s)
    }
";
    let result = parse(source).expect("CS-0035: long string should not close block");
    let step = &result.recipes[0].steps[0];
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
recipe build
    >{
        local s = [==[
            this is opaque text
            with } and ]] inside
        ]==]
        print(s)
    }
";
    let result = parse(source).expect("CS-0035: leveled long string should not close block");
    match &result.recipes[0].steps[0] {
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
recipe build
    >{
        --[[
            here is a } in a block comment
            and another } here
        ]]
        local x = 1
    }
";
    let result = parse(source).expect("CS-0035: block comment should not close block");
    match &result.recipes[0].steps[0] {
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

// ── Task 2.4: `as` rejected on cook_step and plate_step ───────────

#[test]
fn test_as_rejected_on_cook_step() {
    let src = r#"
recipe r
    cook "out.txt" { echo > $<out> } as 'foo'
"#;
    let err = parse(src).expect_err("must reject");
    let msg = err.to_string();
    assert!(msg.contains("`as`"));
    assert!(msg.contains("test_step") || msg.contains("test-step") || msg.contains("test"));
}

#[test]
fn test_as_rejected_on_plate_step() {
    let src = r#"
recipe r
    cook "out.txt" { echo > $<out> }
    plate { cp $<in> /tmp } as 'foo'
"#;
    let err = parse(src).expect_err("must reject");
    assert!(err.to_string().contains("`as`"));
}

// ── Task 2.3: out-of-order modifier rejection ─────────────────────

#[test]
fn test_step_rejects_as_after_timeout() {
    let src = r#"
recipe r
    test { foo } timeout 30 as 'name'
"#;
    let err = parse(src).expect_err("must reject");
    assert!(
        err.to_string().contains("`as`") && err.to_string().contains("must precede"),
        "diagnostic should name `as` and `must precede`, got: {err}"
    );
}

#[test]
fn test_step_rejects_should_fail_before_timeout() {
    let src = r#"
recipe r
    test { foo } should_fail timeout 30
"#;
    let err = parse(src).expect_err("must reject");
    assert!(err.to_string().contains("`timeout`"));
}

#[test]
fn test_step_rejects_should_fail_before_as() {
    let src = r#"
recipe r
    test { foo } should_fail as 'name'
"#;
    let err = parse(src).expect_err("must reject");
    let msg = err.to_string();
    assert!(msg.contains("`as`") || msg.contains("`should_fail`"));
}

// ── Task 2.2: as STRING modifier ───────────────────────────────────

#[test]
fn test_step_parses_as_modifier() {
    let src = r#"
recipe r
    test { foo $<in> } as 'name' timeout 30 should_fail
"#;
    let recipes = parse(src).expect("parse").recipes;
    let step = match &recipes[0].steps[0] {
        Step::Test { step, .. } => step,
        other => panic!("expected test_step, got {:?}", other),
    };
    assert_eq!(step.as_name.as_deref(), Some("name"));
    assert_eq!(step.timeout, Some(30));
    assert!(step.should_fail);
}

#[test]
fn test_step_as_only() {
    let src = r#"
recipe r
    test { foo } as 'just-as'
"#;
    let recipes = parse(src).expect("parse").recipes;
    let step = match &recipes[0].steps[0] {
        Step::Test { step, .. } => step,
        _ => panic!(),
    };
    assert_eq!(step.as_name.as_deref(), Some("just-as"));
    assert_eq!(step.timeout, None);
    assert!(!step.should_fail);
}

#[test]
fn test_step_as_with_substitution_string() {
    let src = r#"
recipe r
    ingredients "src/*.txt"
    cook "build/$<in.stem>.out" { echo > $<out> }
    test { foo $<in> } as '$<in.stem>-roundtrip' timeout 10
"#;
    let recipes = parse(src).expect("parse").recipes;
    let test = recipes[0].steps.iter().find_map(|s| match s {
        Step::Test { step, .. } => Some(step),
        _ => None,
    }).unwrap();
    // Parser preserves the literal string; substitution happens at codegen.
    assert_eq!(test.as_name.as_deref(), Some("$<in.stem>-roundtrip"));
}

// ── App. A.2: duplicate recipe / chore declaration name rule ───────
//
// Recipes and chores share a single callable namespace. Two declarations
// of either kind that share a name MUST be rejected at parse time.

#[test]
fn test_duplicate_recipe_name_rejected() {
    let src = "\
recipe foo
    echo first

recipe foo
    echo second
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
    echo r

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
    echo first
recipe build
    echo second
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
    let source = "recipe build\n    @ ./build\n\nregister\n    cook_cc.bin(\"x\", {})\n";
    let cookfile = parse(source).unwrap();
    assert_eq!(cookfile.recipes.len(), 1);
    assert_eq!(cookfile.register_blocks.len(), 1);
}

#[test]
fn test_parse_register_block_interleaved() {
    let source = "register\n    a()\n\nrecipe build\n    @ ./build\n\nregister\n    b()\n";
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
    let source = "recipe build\n    @ ./build\nregister\n    a()\n";
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
    let source = "use cook_cc\nrecipe build\n    @ ./build\ncook_cc.bin(\"x\", {})\n";
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
fn test_parse_in_body_bare_module_call_is_shell_after_cs0072() {
    let source = "use cpp\nrecipe build\n    cpp.bin(\"x\", { sources = { \"a.c\" } })\n";
    let cookfile = parse(source).unwrap();
    assert_eq!(cookfile.recipes.len(), 1);
    assert_eq!(cookfile.recipes[0].steps.len(), 1);
    use crate::ast::Step;
    assert!(
        matches!(cookfile.recipes[0].steps[0], Step::Shell { .. }),
        "expected Shell step (CS-0072 amendment), got: {:?}",
        cookfile.recipes[0].steps[0],
    );
}

#[test]
fn test_parse_in_body_explicit_register_prefix_still_inline_lua() {
    let source = "use cpp\nrecipe build\n    >> cpp.bin(\"x\", { sources = { \"a.c\" } })\n";
    let cookfile = parse(source).unwrap();
    use crate::ast::Step;
    assert!(matches!(cookfile.recipes[0].steps[0], Step::InlineLua { .. }));
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
    // per (post-CS-0072) rule 6 — not rejected. The rejection rule fires
    // only when `register` is followed by a separator.
    let source = "recipe build\n    register\n";
    let cookfile = parse(source).unwrap();
    assert_eq!(cookfile.recipes.len(), 1);
    assert_eq!(cookfile.recipes[0].steps.len(), 1);
    use crate::ast::Step;
    match &cookfile.recipes[0].steps[0] {
        Step::Shell { command, .. } => assert_eq!(command, "register"),
        other => panic!("expected Shell step, got {:?}", other),
    }
}

#[test]
fn test_parse_register_underscore_inside_body_is_shell() {
    // `register_foo` is not a separator-followed `register`; remains a
    // shell_command per rule 6.
    let source = "recipe build\n    register_foo\n";
    let cookfile = parse(source).unwrap();
    use crate::ast::Step;
    match &cookfile.recipes[0].steps[0] {
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

// ── COOK-67 Task 4: produce as json|lines typing ───────────────────

fn probe_of(src: &str) -> crate::ast::Probe {
    crate::parse(src).unwrap().probes.into_iter().next().unwrap()
}

#[test]
fn produce_shell_default_is_string() {
    let p = probe_of("probe x\n    produce { cat data.json }\n");
    assert!(matches!(p.produce, crate::ast::ProbeProduce::Shell {
        typing: crate::ast::ShellProduceType::String, .. }));
}

#[test]
fn produce_shell_as_json() {
    let p = probe_of("probe x\n    produce as json { cat data.json }\n");
    assert!(matches!(p.produce, crate::ast::ProbeProduce::Shell {
        typing: crate::ast::ShellProduceType::Json, .. }));
}

#[test]
fn produce_shell_as_lines() {
    let p = probe_of("probe x\n    produce as lines { git tag --list }\n");
    assert!(matches!(p.produce, crate::ast::ProbeProduce::Shell {
        typing: crate::ast::ShellProduceType::Lines, .. }));
}

#[test]
fn produce_as_on_lua_block_is_error() {
    let err = crate::parse("probe x\n    produce as json >{ return {} }\n").unwrap_err();
    assert!(format!("{err}").contains("`as` is only valid on a shell-block"),
        "got: {err}");
}

// ── COOK-164: produce as tools / produce as env ───────────────────────

#[test]
fn produce_as_tools_parses_name_list() {
    let cf = parse("probe toolchain\n    produce as tools { cc, ld }\n").unwrap();
    let p = &cf.probes[0];
    assert_eq!(p.produce, crate::ast::ProbeProduce::Tools(vec!["cc".into(), "ld".into()]));
}

#[test]
fn produce_as_tools_accepts_whitespace_separators() {
    let cf = parse("probe toolchain\n    produce as tools { cc ld   ar }\n").unwrap();
    let p = &cf.probes[0];
    assert_eq!(p.produce, crate::ast::ProbeProduce::Tools(vec!["cc".into(), "ld".into(), "ar".into()]));
}

#[test]
fn produce_as_env_parses_name_list() {
    let cf = parse("probe sdk\n    produce as env { SDKROOT, CC }\n").unwrap();
    let p = &cf.probes[0];
    assert_eq!(p.produce, crate::ast::ProbeProduce::Env(vec!["SDKROOT".into(), "CC".into()]));
}

#[test]
fn produce_as_tools_empty_list_is_error() {
    let err = parse("probe t\n    produce as tools {  }\n").unwrap_err();
    assert!(format!("{err}").contains("at least one"), "got: {err}");
}

#[test]
fn produce_as_tools_lua_block_is_error() {
    let err = parse("probe t\n    produce as tools >{ return {} }\n").unwrap_err();
    assert!(format!("{err}").contains("NAME LIST"), "got: {err}");
}

#[test]
fn produce_as_env_invalid_name_is_error() {
    let err = parse("probe t\n    produce as env { 1bad }\n").unwrap_err();
    assert!(format!("{err}").contains("name"), "got: {err}");
}

// ── COOK-67 Task 3: probe declaration parser ────────────────────────

#[test]
fn parse_probe_lua_block_with_deps_and_ingredients() {
    let src = "probe services: cards services_raw\n    ingredients \"data/services.json\"\n    produce >{\n        return {}\n    }\n";
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
    let src = "probe a\n    produce >{ return 1 }\nrecipe build\n    echo hi\n";
    let cf = crate::parse(src).unwrap();
    assert_eq!(cf.probes.len(), 1);
    assert_eq!(cf.recipes.len(), 1);
    assert_eq!(cf.recipes[0].name, "build");
}

#[test]
fn parse_probe_shell_block_default() {
    let src = "probe cards\n    ingredients \"data/cards.json\"\n    produce { cat data/cards.json }\n";
    let cf = crate::parse(src).unwrap();
    let p = &cf.probes[0];
    assert!(matches!(&p.produce, crate::ast::ProbeProduce::Shell { typing: crate::ast::ShellProduceType::String, .. }));
}

#[test]
fn parse_probe_lua_inline_long_string_with_brace() {
    // a `}` inside a [[..]] long string must NOT prematurely close the block
    let src = "probe x\n    produce >{ return [[a}b]] }\n";
    let cf = crate::parse(src).unwrap();
    assert!(matches!(&cf.probes[0].produce,
        crate::ast::ProbeProduce::Lua(code) if code.contains("[[a}b]]")));
}

#[test]
fn parse_probe_lua_inline_long_string_no_brace_still_works() {
    let src = "probe x\n    produce >{ return cook.sh([[cat data]]) }\n";
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
    // Real message: "probe '{name}' has no `produce` block"
    let msg = parse_err("probe x\n    ingredients \"a\"\n");
    assert!(msg.contains("no `produce`"), "got: {msg}");
}

#[test]
fn probe_two_produce_rejected() {
    // Real message: "probe: at most one `produce` per probe"
    let msg = parse_err("probe x\n    produce >{ return 1 }\n    produce >{ return 2 }\n");
    assert!(msg.contains("at most one `produce`"), "got: {msg}");
}

#[test]
fn probe_two_ingredients_rejected() {
    // Real message: "probe: at most one `ingredients` per probe"
    let msg = parse_err(
        "probe x\n    ingredients \"a\"\n    ingredients \"b\"\n    produce >{ return 1 }\n",
    );
    assert!(msg.contains("at most one `ingredients`"), "got: {msg}");
}

#[test]
fn probe_ingredients_after_produce_rejected() {
    // Real message: "probe: `ingredients` must appear before `produce`"
    let msg = parse_err("probe x\n    produce >{ return 1 }\n    ingredients \"a\"\n");
    assert!(msg.contains("must appear before `produce`"), "got: {msg}");
}

#[test]
fn probe_triple_colon_name_rejected_at_parse() {
    // MalformedProbeName from the lexer propagates through crate::parse.
    assert!(crate::parse("probe a:b:c\n    produce >{ return 1 }\n").is_err());
}

#[test]
fn probe_unexpected_step_rejected() {
    // A `cook "out" { true }` content line inside a probe body hits the
    // "expected `ingredients` or `produce`, found: …" branch.
    let msg = parse_err("probe x\n    cook \"out\" { true }\n    produce >{ return 1 }\n");
    assert!(
        msg.contains("expected `ingredients` or `produce`, found"),
        "got: {msg}"
    );
}

#[test]
fn probe_bare_lua_line_in_body_rejected() {
    // A `>{` without the `produce` keyword tokenizes as LuaBlockOpen (the
    // `_other` arm), triggering the "only `ingredients` and `produce`" message.
    let msg = parse_err("probe x\n    >{ return 1 }\n");
    assert!(
        msg.contains("only `ingredients` and `produce` are allowed here"),
        "got: {msg}"
    );
}

#[test]
fn probe_dep_colon_vs_name_colon() {
    // `probe cards: cc:compiler` — name is "cards", dep is "cc:compiler".
    let cf = crate::parse("probe cards: cc:compiler\n    produce >{ return 1 }\n").unwrap();
    assert_eq!(cf.probes[0].name, "cards");
    assert_eq!(cf.probes[0].deps, vec!["cc:compiler"]);
}

#[test]
fn probe_as_on_lua_block_rejected() {
    // Real message: "produce: `as` is only valid on a shell-block produce; …"
    let msg = parse_err("probe x\n    produce as json >{ return {} }\n");
    assert!(msg.contains("shell-block"), "got: {msg}");
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
fn cs0099_cook_declaration_only_still_parses() {
    let src = "recipe \"build\"\n    cook \"bin/app\"\n    gcc src/main.c -o bin/app\n";
    let result = parse(src).unwrap();
    let recipe = &result.recipes[0];
    match &recipe.steps[0] {
        Step::Cook { step, .. } => assert!(step.body.is_none()),
        other => panic!("expected Cook, got {:?}", other),
    }
    assert!(matches!(&recipe.steps[1], Step::Shell { .. }));
}

#[test]
fn cs0099_using_keyword_rejected_with_migration_diagnostic() {
    let src = "recipe r\n    cook \"out\" using { echo hi }\n";
    let err = parse(src).expect_err("CS-0099: the using keyword must be rejected");
    let msg = err.to_string();
    assert!(msg.contains("removed"), "got: {}", msg);
    assert!(msg.contains("CS-0099"), "got: {}", msg);
}

#[test]
fn cs0099_using_lua_form_rejected_with_migration_diagnostic() {
    let src = "recipe r\n    cook \"out\" using >{ return 1 }\n";
    let err = parse(src).expect_err("CS-0099: the using keyword must be rejected");
    let msg = err.to_string();
    assert!(msg.contains("removed"), "got: {}", msg);
}

#[test]
fn cs0099_lua_expr_using_rejected_with_migration_diagnostic() {
    let src = "recipe r\n    cook (\"x\" .. \"y\") using >{ return 1 }\n";
    let err = parse(src).expect_err("CS-0099: the using keyword must be rejected");
    let msg = err.to_string();
    assert!(msg.contains("removed"), "got: {}", msg);
}

#[test]
fn cs0099_register_block_body_still_rejected() {
    // App. A.4: `>>{` is not a valid cook body, with or without `using`.
    let src = "recipe \"r\"\n    cook \"out\" >>{\n        cook.add_unit({command = \"x\"})\n    }\n";
    let err = parse(src).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains(">{"), "got: {}", msg);
}

// ── COOK-160: cook-step disposition (§8.4.3) ───────────────────────

/// Helper: extract the single cook step's disposition from a parsed recipe.
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
fn disp_seal_line_above_cook() {
    let src = "recipe build\n    seal host\n    cook \"x.o\" { cc -c x.c }\n";
    let cf = parse(src).unwrap();
    let d = first_cook_disposition(&cf);
    assert!(d.seal.contains("host"));
    assert_eq!(d.sharing, cook_contracts::Sharing::Shared);
    assert!(!d.record);
}

#[test]
fn disp_seal_lines_stack_additively_with_record() {
    let src = "recipe build\n    seal host\n    seal gpu driver\n    record\n    cook \"x.o\" { cc -c x.c }\n";
    let cf = parse(src).unwrap();
    let d = first_cook_disposition(&cf);
    let got: Vec<&str> = d.seal.iter().map(|s| s.as_str()).collect();
    assert_eq!(got, vec!["driver", "gpu", "host"]); // sorted, deduped
    assert!(d.record);
}

#[test]
fn disp_local_line_above_cook() {
    let src = "recipe build\n    local\n    cook \"x.o\" { cc -c x.c }\n";
    let cf = parse(src).unwrap();
    assert_eq!(first_cook_disposition(&cf).sharing, cook_contracts::Sharing::Local);
}

#[test]
fn disp_pinned_line_above_cook() {
    let src = "recipe build\n    pinned\n    cook \"x.o\" { cc -c x.c }\n";
    let cf = parse(src).unwrap();
    assert_eq!(first_cook_disposition(&cf).sharing, cook_contracts::Sharing::Pinned);
}

#[test]
fn disp_seal_block_applies_to_all_enclosed_cooks() {
    let src = "recipe build\n    seal host {\n        cook \"a.o\" { cc -c a.c }\n        cook \"b.o\" { cc -c b.c }\n    }\n";
    let cf = parse(src).unwrap();
    let mut n = 0;
    for step in &cf.recipes[0].steps {
        if let crate::ast::Step::Cook { step, .. } = step {
            assert!(step.disposition.seal.contains("host"), "cook missing seal host");
            n += 1;
        }
    }
    assert_eq!(n, 2);
}

#[test]
fn disp_local_block_inside_seal_overrides() {
    // Block openers are multi-line: `local {` then the cook on its own line(s),
    // then a closing `}` line (App. A.4 `disposition "{" NEWLINE …`).
    let src = "recipe build\n    seal host {\n        local {\n            cook \"s.o\" { cc -c s.c }\n        }\n    }\n";
    let cf = parse(src).unwrap();
    let d = first_cook_disposition(&cf);
    assert_eq!(d.sharing, cook_contracts::Sharing::Local);
    assert!(d.seal.is_empty(), "local must drop inherited seal refs");
}

#[test]
fn disp_nested_seal_blocks_union() {
    let src = "recipe build\n    seal a {\n        seal b {\n            cook \"x.o\" { cc -c x.c }\n        }\n    }\n";
    let cf = parse(src).unwrap();
    let d = first_cook_disposition(&cf);
    let got: Vec<&str> = d.seal.iter().map(|s| s.as_str()).collect();
    assert_eq!(got, vec!["a", "b"]);
}

#[test]
fn disp_decorator_line_inside_block_adds_to_inherited() {
    let src = "recipe build\n    seal host {\n        seal gpu\n        cook \"x.o\" { cc -c x.c }\n    }\n";
    let cf = parse(src).unwrap();
    let d = first_cook_disposition(&cf);
    let got: Vec<&str> = d.seal.iter().map(|s| s.as_str()).collect();
    assert_eq!(got, vec!["gpu", "host"]);
}

#[test]
fn disp_unannotated_cook_has_default_disposition() {
    let src = "recipe build\n    cook \"x.o\" { cc -c x.c }\n";
    let cf = parse(src).unwrap();
    let d = first_cook_disposition(&cf);
    assert_eq!(*d, crate::ast::Disposition::default());
}

// negative paths

#[test]
fn disp_dangling_decorator_without_cook_errors() {
    let src = "recipe r\n    seal host\n    plate { echo hi }\n";
    assert!(parse(src).is_err());
}

#[test]
fn disp_dangling_decorator_at_eof_errors() {
    let src = "recipe r\n    seal host\n";
    assert!(parse(src).is_err());
}

#[test]
fn disp_local_and_pinned_mutually_exclusive_errors() {
    let src = "recipe r\n    local\n    pinned\n    cook \"x.o\" { cc -c x.c }\n";
    assert!(parse(src).is_err());
}

#[test]
fn disp_seal_quoted_ref_errors() {
    let src = "recipe r\n    seal \"host\"\n    cook \"x.o\" { cc -c x.c }\n";
    assert!(parse(src).is_err());
}

#[test]
fn disp_seal_triple_colon_ref_errors() {
    let src = "recipe r\n    seal a:b:c\n    cook \"x.o\" { cc -c x.c }\n";
    assert!(parse(src).is_err());
}

#[test]
fn disp_unterminated_block_errors() {
    let src = "recipe r\n    seal host {\n        cook \"a.o\" { cc -c a.c }\n";
    assert!(parse(src).is_err());
}

#[test]
fn disp_empty_block_errors() {
    let src = "recipe r\n    seal host {\n    }\n";
    assert!(parse(src).is_err());
}

#[test]
fn disp_block_with_non_cook_step_errors() {
    let src = "recipe r\n    seal host {\n        echo nope\n    }\n";
    assert!(parse(src).is_err());
}

#[test]
fn disp_local_keyword_with_trailing_content_is_shell() {
    // `local foo` is NOT a disposition line — falls through to shell_command.
    let src = "recipe r\n    local foo\n";
    let cf = parse(src).unwrap();
    // it parsed as a shell step, not a disposition error
    assert!(matches!(cf.recipes[0].steps[0], crate::ast::Step::Shell { .. }));
}

#[test]
fn disp_inline_single_line_block_is_rejected() {
    // The block opener `{` must end its line (App. A.4 `disposition "{" NEWLINE`).
    // A fully-inline `local { cook … }` is not admitted — the `{`-suffix check
    // fails (line ends with `}`), so it is neither a block opener nor a bare
    // decorator, and is rejected.
    let src = "recipe r\n    seal host {\n        local { cook \"s.o\" { cc -c s.c } }\n    }\n";
    assert!(parse(src).is_err());
}

#[test]
fn disp_inline_block_at_recipe_top_level_is_shell() {
    // §8.4.3 rule 13: at recipe-body top level (not inside a block), an inline
    // `local { … }` line is non-reserved and falls through to shell_command —
    // it is NOT a disposition and NOT an error.
    let src = "recipe r\n    local { cook \"s.o\" { cc -c s.c } }\n";
    let cf = parse(src).unwrap();
    assert!(matches!(cf.recipes[0].steps[0], crate::ast::Step::Shell { .. }));
}

#[test]
fn disp_decorator_line_override_inside_seal_block() {
    // A `local` DECORATOR LINE (not an inner block) inside a `seal` block
    // overrides the inherited seal on the following cook (§8.4.3 rule 10).
    let src = "recipe build\n    seal host {\n        local\n        cook \"s.o\" { cc -c s.c }\n    }\n";
    let cf = parse(src).unwrap();
    let d = first_cook_disposition(&cf);
    assert_eq!(d.sharing, cook_contracts::Sharing::Local);
    assert!(d.seal.is_empty(), "decorator-line local must drop inherited seal");
}

#[test]
fn disp_seal_block_with_multiline_cook_body() {
    // The block's closing `}` is distinguishable from a multi-line cook body's
    // own `}`: parse_cook_line consumes the body's brace, so the block `}` is a
    // separate line. Pins the `}`-distinguishability path.
    let src = "recipe build\n    seal host {\n        cook \"a.o\" {\n            cc -c a.c -o a.o\n        }\n    }\n";
    let cf = parse(src).unwrap();
    let d = first_cook_disposition(&cf);
    assert!(d.seal.contains("host"));
    // exactly one cook, with a multi-line shell body
    let cooks: Vec<_> = cf.recipes[0]
        .steps
        .iter()
        .filter(|s| matches!(s, crate::ast::Step::Cook { .. }))
        .collect();
    assert_eq!(cooks.len(), 1);
}
