use super::*;

// ── SHI-71: Unquoted recipe names ──────────────────────────────────

#[test]
fn test_bare_recipe_name_parses() {
    let source = "recipe build\n    echo hello\nend\n";
    let result = parse(source).unwrap();
    assert_eq!(result.recipes[0].name, "build");
}

#[test]
fn test_bare_recipe_name_with_deps() {
    let source = "recipe build: lib setup\n    echo hello\nend\n";
    let result = parse(source).unwrap();
    assert_eq!(result.recipes[0].name, "build");
    assert_eq!(result.recipes[0].deps, vec!["lib", "setup"]);
}

#[test]
fn test_bare_dotted_dep() {
    let source = "recipe all: backend.build\nend\n";
    let result = parse(source).unwrap();
    assert_eq!(result.recipes[0].deps, vec!["backend.build"]);
}

#[test]
fn test_bare_use_statement() {
    let source = "use cpp\n\nrecipe build\n    echo hello\nend\n";
    let cookfile = parse(source).unwrap();
    assert_eq!(cookfile.uses[0].module_name, "cpp");
}

#[test]
fn test_bare_config_name() {
    let source = "config debug\n    CFLAGS \"-g\"\nend\n\nrecipe build\n    echo hello\nend\n";
    let result = parse(source).unwrap();
    assert_eq!(result.configs["debug"], vec![("CFLAGS".to_string(), "-g".to_string())]);
}

// ── SHI-72: Implicit recipes ───────────────────────────────────────

#[test]
fn test_implicit_recipe_parses() {
    let source = "build:\n    echo hello\nend\n";
    let result = parse(source).unwrap();
    assert_eq!(result.recipes.len(), 1);
    assert_eq!(result.recipes[0].name, "build");
    assert!(result.recipes[0].deps.is_empty());
}

#[test]
fn test_implicit_recipe_with_deps() {
    let source = "build: lib setup\n    echo hello\nend\n";
    let result = parse(source).unwrap();
    assert_eq!(result.recipes[0].name, "build");
    assert_eq!(result.recipes[0].deps, vec!["lib", "setup"]);
}

#[test]
fn test_implicit_recipe_with_body() {
    let source = "clean:\n    rm -rf build\nend\n";
    let result = parse(source).unwrap();
    assert_eq!(result.recipes[0].steps.len(), 1);
    assert!(matches!(&result.recipes[0].steps[0], Step::Shell { command, .. } if command == "rm -rf build"));
}

// ── SHI-73: Module calls without Lua delimiters ────────────────────

#[test]
fn test_module_call_single_line() {
    let source = "recipe build\n    cpp.compile(\"src/*.cpp\")\nend\n";
    let result = parse(source).unwrap();
    assert_eq!(result.recipes[0].steps.len(), 1);
    match &result.recipes[0].steps[0] {
        Step::Lua { code, .. } => {
            assert_eq!(code, "cpp.compile(\"src/*.cpp\")");
        }
        other => panic!("expected Lua step, got {:?}", other),
    }
}

#[test]
fn test_module_call_multiline() {
    let source = r#"recipe build
    cpp.compile {
        sources = "src/*.cpp",
        output_dir = "build/obj/",
    }
end
"#;
    let result = parse(source).unwrap();
    assert_eq!(result.recipes[0].steps.len(), 1);
    match &result.recipes[0].steps[0] {
        Step::LuaBlock { code, .. } => {
            assert!(code.contains("cpp.compile {"), "code was: {}", code);
            assert!(code.contains("sources"), "code was: {}", code);
            assert!(code.contains("}"), "code was: {}", code);
        }
        other => panic!("expected LuaBlock step, got {:?}", other),
    }
}

#[test]
fn test_non_module_dot_is_shell() {
    // A line starting with `.` is not a module call
    let source = "recipe build\n    ./run.sh\nend\n";
    let result = parse(source).unwrap();
    assert!(matches!(&result.recipes[0].steps[0], Step::Shell { .. }));
}

#[test]
fn test_module_call_no_args() {
    let source = "recipe build\n    cpp.detect_compiler()\nend\n";
    let result = parse(source).unwrap();
    match &result.recipes[0].steps[0] {
        Step::Lua { code, .. } => {
            assert_eq!(code, "cpp.detect_compiler()");
        }
        other => panic!("expected Lua step, got {:?}", other),
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
    let source = "recipe \"build\"\n    gcc -o main main.c\nend\n";
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
    let source = "recipe \"build\": \"setup\" \"lib\"\n    echo building\nend\n";
    let result = parse(source).unwrap();
    let recipe = &result.recipes[0];
    assert_eq!(recipe.deps, vec!["setup".to_string(), "lib".to_string()]);
}

#[test]
fn test_recipe_with_ingredients() {
    let source = "recipe \"lib\"\n    ingredients \"lib/*.c\" \"include/*.h\"\n    echo compiling\nend\n";
    let result = parse(source).unwrap();
    let recipe = &result.recipes[0];
    assert_eq!(
        recipe.ingredients,
        vec!["lib/*.c".to_string(), "include/*.h".to_string()]
    );
}

#[test]
fn test_duplicate_ingredients_error() {
    let source = "recipe \"lib\"\n    ingredients \"lib/*.c\"\n    ingredients \"include/*.h\"\nend\n";
    let result = parse(source);
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(msg.contains("duplicate"), "error was: {}", msg);
}

#[test]
fn test_cook_step_shell() {
    let source = "recipe \"lib\"\n    ingredients \"lib/*.c\"\n    cook \"build/obj/{stem}.o\" using \"gcc -c {in} -o {out}\"\nend\n";
    let result = parse(source).unwrap();
    let recipe = &result.recipes[0];
    assert_eq!(recipe.steps.len(), 1);
    match &recipe.steps[0] {
        Step::Cook { step, line } => {
            assert_eq!(*line, 3);
            assert_eq!(step.output_pattern, "build/obj/{stem}.o");
            assert_eq!(
                step.using_clause,
                Some(UsingClause::Shell("gcc -c {in} -o {out}".to_string()))
            );
        }
        other => panic!("expected Cook step, got {:?}", other),
    }
}

#[test]
fn test_cook_step_many_to_one() {
    let source = "recipe \"lib\"\n    ingredients \"lib/*.c\"\n    cook \"build/lib.a\" using \"ar rcs {out} {all}\"\nend\n";
    let result = parse(source).unwrap();
    match &result.recipes[0].steps[0] {
        Step::Cook { step, .. } => {
            assert_eq!(step.output_pattern, "build/lib.a");
            assert_eq!(
                step.using_clause,
                Some(UsingClause::Shell("ar rcs {out} {all}".to_string()))
            );
        }
        other => panic!("expected Cook, got {:?}", other),
    }
}

#[test]
fn test_cook_step_declaration_only() {
    let source = "recipe \"build\"\n    ingredients \"src/*.c\"\n    cook \"bin/app\"\n    gcc src/main.c -o bin/app\nend\n";
    let result = parse(source).unwrap();
    let recipe = &result.recipes[0];
    assert_eq!(recipe.steps.len(), 2);
    match &recipe.steps[0] {
        Step::Cook { step, .. } => {
            assert_eq!(step.output_pattern, "bin/app");
            assert!(step.using_clause.is_none());
        }
        other => panic!("expected Cook, got {:?}", other),
    }
    assert!(matches!(&recipe.steps[1], Step::Shell { .. }));
}

#[test]
fn test_cook_step_lua_block() {
    let source = "recipe \"lib\"\n    ingredients \"lib/*.c\"\n    cook \"build/obj/{stem}.o\" using >{\n        cook.sh(\"gcc -c \" .. input .. \" -o \" .. output)\n    }\nend\n";
    let result = parse(source).unwrap();
    match &result.recipes[0].steps[0] {
        Step::Cook { step, .. } => {
            assert_eq!(step.output_pattern, "build/obj/{stem}.o");
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
    let source = "recipe \"test\"\n    ingredients \"tests/*.c\"\n    cook \"build/{stem}\" using \"cc {in} -o {out}\"\n    plate \"./{out}\"\nend\n";
    let result = parse(source).unwrap();
    let recipe = &result.recipes[0];
    assert_eq!(recipe.steps.len(), 2);
    match &recipe.steps[1] {
        Step::Plate { step, line } => {
            assert_eq!(*line, 4);
            assert_eq!(step.command, "./{out}");
        }
        other => panic!("expected Plate, got {:?}", other),
    }
}

#[test]
fn test_mixed_steps() {
    let source = "recipe \"lib\": \"setup\"\n    ingredients \"lib/*.c\" \"include/*.h\"\n    cook \"build/obj/{stem}.o\" using \"gcc -c {in} -o {out}\"\n    > print(\"compiled\")\n    cook \"build/libmath.a\" using \"ar rcs {out} {all}\"\nend\n";
    let result = parse(source).unwrap();
    let recipe = &result.recipes[0];
    assert_eq!(recipe.deps, vec!["setup".to_string()]);
    assert_eq!(recipe.ingredients, vec!["lib/*.c".to_string(), "include/*.h".to_string()]);
    assert_eq!(recipe.steps.len(), 3);
    assert!(matches!(&recipe.steps[0], Step::Cook { .. }));
    assert!(matches!(&recipe.steps[1], Step::Lua { .. }));
    assert!(matches!(&recipe.steps[2], Step::Cook { .. }));
}

#[test]
fn test_task_runner_no_metadata() {
    let source = "recipe \"clean\"\n    rm -rf build bin\nend\n";
    let result = parse(source).unwrap();
    let recipe = &result.recipes[0];
    assert!(recipe.deps.is_empty());
    assert!(recipe.ingredients.is_empty());
    assert_eq!(recipe.steps.len(), 1);
}

#[test]
fn test_multiple_recipes() {
    let source = "recipe \"setup\"\n    mkdir -p build\nend\n\nrecipe \"build\": \"setup\"\n    echo building\nend\n";
    let result = parse(source).unwrap();
    assert_eq!(result.recipes.len(), 2);
    assert_eq!(result.recipes[0].name, "setup");
    assert_eq!(result.recipes[1].name, "build");
    assert_eq!(result.recipes[1].deps, vec!["setup".to_string()]);
}

#[test]
fn test_unclosed_recipe() {
    let source = "recipe \"build\"\n    gcc -o main main.c\n";
    assert!(parse(source).is_err());
}

#[test]
fn test_lua_block_in_recipe() {
    let source = "recipe \"build\"\n>{\n    local x = 1\n    print(x)\n}\nend\n";
    let result = parse(source).unwrap();
    assert_eq!(result.recipes[0].steps.len(), 1);
    assert!(matches!(&result.recipes[0].steps[0], Step::LuaBlock { .. }));
}

#[test]
fn test_lua_block_nested_braces() {
    let source = "recipe \"build\"\n>{\n    if true then\n        local t = {1, 2, 3}\n    end\n}\nend\n";
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
    let source = "recipe \"build\"\n    # comment\n    gcc -o main main.c\n\nend\n";
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
    let source = "recipe \"build\"\n>{\n    local s = \"}\"\n    print(s)\n}\nend\n";
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
    let source = "recipe \"build\"\n>{\n    local x = 1 -- }\n    print(x)\n}\nend\n";
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
fn test_bare_vars_parsed() {
    let source = r#"CC "gcc"
CFLAGS "-Wall"

recipe "build"
    echo hello
end
"#;
    let result = parse(source).unwrap();
    assert_eq!(result.vars.len(), 2);
    assert_eq!(result.vars[0], ("CC".to_string(), "gcc".to_string()));
    assert_eq!(result.vars[1], ("CFLAGS".to_string(), "-Wall".to_string()));
    assert_eq!(result.recipes.len(), 1);
}

#[test]
fn test_config_blocks_parsed() {
    let source = r#"config "debug"
    CFLAGS "-g -O0"
end

config "release"
    CFLAGS "-O2"
    LDFLAGS "-s"
end

recipe "build"
    echo hello
end
"#;
    let result = parse(source).unwrap();
    assert_eq!(result.configs.len(), 2);
    assert_eq!(result.configs["debug"], vec![("CFLAGS".to_string(), "-g -O0".to_string())]);
    assert_eq!(result.configs["release"].len(), 2);
    assert_eq!(result.recipes.len(), 1);
}

#[test]
fn test_vars_and_configs_together() {
    let source = r#"CC "gcc"

config "debug"
    CFLAGS "-g"
end

recipe "build"
    echo hello
end
"#;
    let result = parse(source).unwrap();
    assert_eq!(result.vars.len(), 1);
    assert_eq!(result.configs.len(), 1);
    assert_eq!(result.recipes.len(), 1);
}

#[test]
fn test_empty_config_block() {
    let source = r#"config "empty"
end

recipe "build"
    echo hello
end
"#;
    let result = parse(source).unwrap();
    assert_eq!(result.configs["empty"], vec![]);
}

#[test]
fn test_var_after_recipe_is_shell_command() {
    let source = r#"recipe "build"
    CC "gcc"
end
"#;
    let result = parse(source).unwrap();
    assert_eq!(result.vars.len(), 0);
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
end

config "debug"
    CFLAGS "-g"
end
"#;
    let result = parse(source);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("config blocks must appear before recipes"), "got: {err}");
}

#[test]
fn test_duplicate_config_name_last_wins() {
    let source = r#"config "debug"
    CFLAGS "-g"
end

config "debug"
    CFLAGS "-g3 -O0"
end

recipe "build"
    echo hello
end
"#;
    let result = parse(source).unwrap();
    assert_eq!(result.configs["debug"], vec![("CFLAGS".to_string(), "-g3 -O0".to_string())]);
}

#[test]
fn test_interactive_shell_step() {
    let source = "recipe \"run\"\n    @./bin/app\nend\n";
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
    let source = "recipe \"build\"\n    echo hello\nend\n";
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
    let source = "recipe \"run\"\n    @\nend\n";
    let result = parse(source);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("requires a command"), "got: {err}");
}

#[test]
fn test_at_in_cook_using_is_not_interactive() {
    let source = r#"recipe "build"
    ingredients "src/*.c"
    cook "build/{stem}.o" using "@gcc -c {in} -o {out}"
end
"#;
    let result = parse(source).unwrap();
    match &result.recipes[0].steps[0] {
        Step::Cook { step, .. } => {
            match &step.using_clause {
                Some(UsingClause::Shell(cmd)) => {
                    assert!(cmd.starts_with('@'), "@ should be preserved in using clause");
                }
                other => panic!("expected Shell using clause, got {:?}", other),
            }
        }
        other => panic!("expected Cook step, got {:?}", other),
    }
}

#[test]
fn test_unterminated_config_block_errors() {
    let source = r#"config "debug"
    CFLAGS "-g"
"#;
    let result = parse(source);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("not closed"), "got: {err}");
}

#[test]
fn test_parse_use_statement() {
    let source = "use \"cpp\"\n\nrecipe \"build\"\n    echo hello\nend\n";
    let cookfile = crate::parse(source).unwrap();
    assert_eq!(cookfile.uses.len(), 1);
    assert_eq!(cookfile.uses[0].module_name, "cpp");
    assert_eq!(cookfile.uses[0].line, 1);
    assert_eq!(cookfile.recipes.len(), 1);
}

#[test]
fn test_parse_multiple_use_statements() {
    let source = "use \"cpp\"\nuse \"proto\"\n\nrecipe \"build\"\n    echo hello\nend\n";
    let cookfile = crate::parse(source).unwrap();
    assert_eq!(cookfile.uses.len(), 2);
    assert_eq!(cookfile.uses[0].module_name, "cpp");
    assert_eq!(cookfile.uses[1].module_name, "proto");
}

#[test]
fn test_parse_use_with_vars_and_configs() {
    let source = "use \"cpp\"\nCC \"gcc\"\n\nconfig \"debug\"\n    CFLAGS \"-g\"\nend\n\nrecipe \"build\"\n    echo hello\nend\n";
    let cookfile = crate::parse(source).unwrap();
    assert_eq!(cookfile.uses.len(), 1);
    assert_eq!(cookfile.vars.len(), 1);
    assert_eq!(cookfile.configs.len(), 1);
}

#[test]
fn test_test_step_basic() {
    let source = r#"recipe "run-tests"
    ingredients "tests/*.c"
    cook "build/{stem}" using "cc {in} -o {out}"
    test "./{out}"
end
"#;
    let result = parse(source).unwrap();
    let recipe = &result.recipes[0];
    assert_eq!(recipe.steps.len(), 2);
    match &recipe.steps[1] {
        Step::Test { step, line } => {
            assert_eq!(*line, 4);
            assert_eq!(step.command, "./{out}");
            assert_eq!(step.timeout, None);
            assert!(!step.should_fail);
        }
        other => panic!("expected Test, got {:?}", other),
    }
}

#[test]
fn test_test_step_with_timeout() {
    let source = r#"recipe "run-tests"
    test "./{out}" timeout 30
end
"#;
    let result = parse(source).unwrap();
    let recipe = &result.recipes[0];
    match &recipe.steps[0] {
        Step::Test { step, .. } => {
            assert_eq!(step.command, "./{out}");
            assert_eq!(step.timeout, Some(30));
            assert!(!step.should_fail);
        }
        other => panic!("expected Test, got {:?}", other),
    }
}

#[test]
fn test_test_step_with_should_fail() {
    let source = r#"recipe "run-tests"
    test "./{out}" should_fail
end
"#;
    let result = parse(source).unwrap();
    let recipe = &result.recipes[0];
    match &recipe.steps[0] {
        Step::Test { step, .. } => {
            assert_eq!(step.command, "./{out}");
            assert_eq!(step.timeout, None);
            assert!(step.should_fail);
        }
        other => panic!("expected Test, got {:?}", other),
    }
}

#[test]
fn test_test_step_with_timeout_and_should_fail() {
    let source = r#"recipe "run-tests"
    test "./{out}" timeout 60 should_fail
end
"#;
    let result = parse(source).unwrap();
    let recipe = &result.recipes[0];
    match &recipe.steps[0] {
        Step::Test { step, .. } => {
            assert_eq!(step.command, "./{out}");
            assert_eq!(step.timeout, Some(60));
            assert!(step.should_fail);
        }
        other => panic!("expected Test, got {:?}", other),
    }
}

#[test]
fn test_parse_use_after_recipe_fails() {
    let source = "recipe \"build\"\n    echo hello\nend\n\nuse \"cpp\"\n";
    let result = crate::parse(source);
    assert!(result.is_err());
}

#[test]
fn test_parse_import_decl() {
    let source = r#"
import backend ./services/backend
import frontend ./apps/frontend

recipe "all": "backend.build" "frontend.build"
end
"#;
    let cookfile = crate::parse(source).unwrap();
    assert_eq!(cookfile.imports.len(), 2);
    assert_eq!(cookfile.imports[0].name, "backend");
    assert_eq!(cookfile.imports[0].path, "./services/backend");
    assert_eq!(cookfile.imports[1].name, "frontend");
    assert_eq!(cookfile.imports[1].path, "./apps/frontend");
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
