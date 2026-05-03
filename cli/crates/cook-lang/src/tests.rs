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

// ── SHI-73: Module calls without Lua delimiters ────────────────────

#[test]
fn test_module_call_single_line() {
    // Per §4.11, single-line module-call desugars to InlineLua (register-phase).
    let source = "recipe build\n    cpp.compile(\"src/*.cpp\")\n";
    let result = parse(source).unwrap();
    assert_eq!(result.recipes[0].steps.len(), 1);
    match &result.recipes[0].steps[0] {
        Step::InlineLua { code, .. } => {
            assert_eq!(code, "cpp.compile(\"src/*.cpp\")");
        }
        other => panic!("expected InlineLua step, got {:?}", other),
    }
}

#[test]
fn test_module_call_multiline() {
    // Per §4.11, multi-line module-call desugars to InlineLuaBlock (register-phase).
    let source = r#"recipe build
    cpp.compile {
        sources = "src/*.cpp",
        output_dir = "build/obj/",
    }
"#;
    let result = parse(source).unwrap();
    assert_eq!(result.recipes[0].steps.len(), 1);
    match &result.recipes[0].steps[0] {
        Step::InlineLuaBlock { code, .. } => {
            assert!(code.contains("cpp.compile {"), "code was: {}", code);
            assert!(code.contains("sources"), "code was: {}", code);
            assert!(code.contains("}"), "code was: {}", code);
        }
        other => panic!("expected InlineLuaBlock step, got {:?}", other),
    }
}

#[test]
fn test_non_module_dot_is_shell() {
    // A line starting with `.` is not a module call
    let source = "recipe build\n    ./run.sh\n";
    let result = parse(source).unwrap();
    assert!(matches!(&result.recipes[0].steps[0], Step::Shell { .. }));
}

#[test]
fn test_module_call_no_args() {
    let source = "recipe build\n    cpp.detect_compiler()\n";
    let result = parse(source).unwrap();
    match &result.recipes[0].steps[0] {
        Step::InlineLua { code, .. } => {
            assert_eq!(code, "cpp.detect_compiler()");
        }
        other => panic!("expected InlineLua step, got {:?}", other),
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
    let source = "recipe \"lib\"\n    ingredients \"lib/*.c\"\n    cook \"build/obj/{stem}.o\" using {\n        gcc -c {in} -o {out}\n    }\n";
    let result = parse(source).unwrap();
    let recipe = &result.recipes[0];
    assert_eq!(recipe.steps.len(), 1);
    match &recipe.steps[0] {
        Step::Cook { step, line } => {
            assert_eq!(*line, 3);
            assert_eq!(step.outputs[0], "build/obj/{stem}.o");
            assert_eq!(
                step.using_clause,
                Some(UsingClause::ShellBlock(vec!["gcc -c {in} -o {out}".to_string()]))
            );
        }
        other => panic!("expected Cook step, got {:?}", other),
    }
}

#[test]
fn test_cook_step_many_to_one() {
    let source = "recipe \"lib\"\n    ingredients \"lib/*.c\"\n    cook \"build/lib.a\" using {\n        ar rcs {out} {all}\n    }\n";
    let result = parse(source).unwrap();
    match &result.recipes[0].steps[0] {
        Step::Cook { step, .. } => {
            assert_eq!(step.outputs[0], "build/lib.a");
            assert_eq!(
                step.using_clause,
                Some(UsingClause::ShellBlock(vec!["ar rcs {out} {all}".to_string()]))
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
            assert_eq!(step.outputs[0], "bin/app");
            assert!(step.using_clause.is_none());
        }
        other => panic!("expected Cook, got {:?}", other),
    }
    assert!(matches!(&recipe.steps[1], Step::Shell { .. }));
}

#[test]
fn test_cook_step_lua_block() {
    let source = "recipe \"lib\"\n    ingredients \"lib/*.c\"\n    cook \"build/obj/{stem}.o\" using >{\n        cook.sh(\"gcc -c \" .. input .. \" -o \" .. output)\n    }\n";
    let result = parse(source).unwrap();
    match &result.recipes[0].steps[0] {
        Step::Cook { step, .. } => {
            assert_eq!(step.outputs[0], "build/obj/{stem}.o");
            match &step.using_clause {
                Some(UsingClause::LuaBlock(code)) => {
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
    let source = "recipe test_recipe\n    ingredients \"tests/*.c\"\n    cook \"build/{in.stem}\" using {\n        cc {in} -o {out}\n    }\n    plate {\n        ./{in}\n    }\n";
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
    cook "build/obj/{stem}.o" using {
        gcc -c {in} -o {out}
    }
    >> print("compiled")
    cook "build/libmath.a" using {
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
    cook "a" using {
        echo a
    }
    > print("x")
    cook "b" using {
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
fn test_at_in_cook_using_is_not_interactive() {
    let source = r#"recipe "build"
    ingredients "src/*.c"
    cook "build/{stem}.o" using {
        @gcc -c {in} -o {out}
    }
"#;
    let result = parse(source).unwrap();
    match &result.recipes[0].steps[0] {
        Step::Cook { step, .. } => {
            match &step.using_clause {
                Some(UsingClause::ShellBlock(cmds)) => {
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
    let source = "recipe r\n    ingredients \"tests/*.c\"\n    cook \"build/{in.stem}\" using { cc {in} -o {out} }\n    test { ./{in} }\n";
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
    let source = "recipe r\n    ingredients \"tests/*.c\"\n    cook \"build/{in.stem}\" using { cc {in} -o {out} }\n    test { ./{in} } timeout 60\n";
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
    let source = "recipe r\n    ingredients \"tests/*.c\"\n    cook \"build/{in.stem}\" using { cc {in} -o {out} }\n    test { ./{in} } should_fail\n";
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
    let source = "recipe r\n    ingredients \"tests/*.c\"\n    cook \"build/{in.stem}\" using { cc {in} -o {out} }\n    test { ./{in} } timeout 60 should_fail\n";
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
    let source = "recipe \"wasm\"\n    ingredients \"src/*.rs\"\n    cook \"a.js\" \"b.wasm\" using >{\n        sh(\"cmd\")\n    }\n";
    let result = crate::parse(source).expect("should parse");
    match &result.recipes[0].steps[0] {
        crate::ast::Step::Cook { step, .. } => {
            assert_eq!(step.outputs, vec!["a.js".to_string(), "b.wasm".to_string()]);
            assert!(matches!(step.using_clause, Some(crate::ast::UsingClause::LuaBlock(_))));
        }
        _ => panic!("expected Cook step"),
    }
}

#[test]
fn test_single_output_shell_block() {
    let source = "recipe \"x\"\n    ingredients \"src/*\"\n    cook \"bin/out\" using {\n        cmd1\n        cmd2\n    }\n";
    let result = crate::parse(source).expect("should parse");
    match &result.recipes[0].steps[0] {
        crate::ast::Step::Cook { step, .. } => {
            assert_eq!(step.outputs, vec!["bin/out".to_string()]);
            match &step.using_clause {
                Some(crate::ast::UsingClause::ShellBlock(cmds)) => {
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
    let source = "recipe \"wasm\"\n    ingredients \"src/*.rs\"\n    cook \"a.js\" \"b.wasm\" using {\n        wasm-pack build\n        cp a.js out/a.js\n        cp b.wasm out/b.wasm\n    }\n";
    let result = crate::parse(source).expect("should parse");
    match &result.recipes[0].steps[0] {
        crate::ast::Step::Cook { step, .. } => {
            assert_eq!(step.outputs, vec!["a.js".to_string(), "b.wasm".to_string()]);
            match &step.using_clause {
                Some(crate::ast::UsingClause::ShellBlock(cmds)) => {
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
    assert!(msg.contains("CS-0024"), "expected CS-0024 migration diagnostic, got: {}", msg);
}

#[test]
fn test_using_string_form_rejected_with_migration_diagnostic() {
    let src = r#"recipe build
    cook "out" using "echo hi"
"#;
    let err = parse(src).expect_err("CS-0024: bare-string using form must be rejected");
    match err {
        ParseError::Parse { message, .. } => {
            assert!(message.contains("CS-0024"), "diagnostic should name CS-0024, got: {message}");
            assert!(message.contains("{ cmd }"), "diagnostic should name the new form, got: {message}");
        }
        e => panic!("expected ParseError::Parse, got {:?}", e),
    }
}

// ── CS-0022 Phase G Item 5: one-line shell block ──────────────────

#[test]
fn cs_0022_one_line_shell_block_parses() {
    // `using { cmd }` on one line must parse to a ShellBlock with one command.
    let src = "recipe build\n    cook \"build/{in.stem}.o\" using { gcc -c {in} -o {out} }\n";
    let cookfile = parse(src).expect("one-line shell block should parse");
    match &cookfile.recipes[0].steps[0] {
        Step::Cook { step, .. } => {
            match &step.using_clause {
                Some(UsingClause::ShellBlock(cmds)) => {
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
    let src = "recipe build\n    ingredients \"src/*.c\"\n    cook \"build/{in.stem}.o\" using { {CC} -c {in} -o {out} }\n";
    let cookfile = parse(src).expect("one-line shell block with placeholders should parse");
    match &cookfile.recipes[0].steps[0] {
        Step::Cook { step, .. } => {
            match &step.using_clause {
                Some(UsingClause::ShellBlock(cmds)) => {
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
    let src = "recipe build\n    cook \"build/app\" using { gcc main.c -o {out} }\n    plate { ./{in} }\n";
    let cookfile = parse(src).expect("should parse");
    assert_eq!(cookfile.recipes[0].steps.len(), 2, "should have cook + plate");
    assert!(matches!(&cookfile.recipes[0].steps[0], Step::Cook { .. }));
    assert!(matches!(&cookfile.recipes[0].steps[1], Step::Plate { step, .. } if matches!(step.body, Body::ShellBlock(_))));
}

#[test]
fn test_plate_string_form_rejected() {
    let source = "recipe r\n    ingredients \"tests/*.c\"\n    cook \"build/{in.stem}\" using { cc {in} -o {out} }\n    plate \"./{out}\"\n";
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
    let source = "recipe r\n    ingredients \"tests/*.c\"\n    cook \"build/{in.stem}\" using { cc {in} -o {out} }\n    test \"./{out}\" timeout 60\n";
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
    let source = "recipe r\n    ingredients \"src/*.c\"\n    cook \"build/{in.stem}\" using { cc {in} -o {out} }\n    plate >{\n        cook.sh(\"strip \" .. input)\n    }\n";
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
    cook \"out.txt\" using {
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
            let body = step.using_clause.as_ref().expect("using clause");
            match body {
                UsingClause::ShellBlock(lines) => {
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
    cook \"out.txt\" using {
        cat <<'END' > out.txt
        } stays literal
        END
    }
";
    let result = parse(source).expect("CS-0035: quoted heredoc delim handled");
    match &result.recipes[0].steps[0] {
        Step::Cook { step, .. } => match step.using_clause.as_ref().unwrap() {
            UsingClause::ShellBlock(lines) => {
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
