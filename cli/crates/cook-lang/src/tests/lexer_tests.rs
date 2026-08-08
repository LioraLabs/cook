use super::*;

#[test]
fn test_comment_line() {
    let tokens = tokenize("# this is a comment").unwrap();
    assert_eq!(tokens.len(), 1);
    assert_eq!(
        tokens[0].value,
        Token::Comment(" this is a comment".to_string())
    );
}

#[test]
fn test_indented_comment() {
    let tokens = tokenize("   # indented comment").unwrap();
    assert_eq!(tokens.len(), 1);
    assert_eq!(
        tokens[0].value,
        Token::Comment(" indented comment".to_string())
    );
}

#[test]
fn test_recipe_header() {
    let tokens = tokenize(r#"recipe "build""#).unwrap();
    assert_eq!(tokens.len(), 1);
    assert_eq!(tokens[0].value, Token::RecipeHeader { name: "build".to_string(), deps: vec![] });
}

#[test]
fn test_recipe_header_extra_spaces() {
    let tokens = tokenize(r#"recipe   "build""#).unwrap();
    assert_eq!(tokens.len(), 1);
    assert_eq!(tokens[0].value, Token::RecipeHeader { name: "build".to_string(), deps: vec![] });
}

#[test]
fn test_recipe_prefix_is_shell_command() {
    let tokens = tokenize("recipes_cleanup").unwrap();
    assert_eq!(tokens.len(), 1);
    assert_eq!(
        tokens[0].value,
        Token::Content("recipes_cleanup".to_string())
    );
}

#[test]
fn test_bare_end_is_content() {
    let tokens = tokenize("end").unwrap();
    assert_eq!(tokens.len(), 1);
    assert_eq!(tokens[0].value, Token::Content("end".to_string()));
}

#[test]
fn test_indented_end_is_content() {
    let tokens = tokenize("   end").unwrap();
    assert_eq!(tokens.len(), 1);
    assert_eq!(tokens[0].value, Token::Content("end".to_string()));
}

#[test]
fn test_lua_line() {
    let tokens = tokenize("> print('hello')").unwrap();
    assert_eq!(tokens.len(), 1);
    assert_eq!(
        tokens[0].value,
        Token::LuaLine("print('hello')".to_string())
    );
}

#[test]
fn test_empty_lua_line() {
    let tokens = tokenize(">").unwrap();
    assert_eq!(tokens.len(), 1);
    assert_eq!(tokens[0].value, Token::LuaLine("".to_string()));
}

#[test]
fn test_lua_block_open() {
    let tokens = tokenize(">{").unwrap();
    assert_eq!(tokens.len(), 1);
    assert_eq!(tokens[0].value, Token::LuaBlockOpen);
}

#[test]
fn test_taste_is_content() {
    let tokens = tokenize("taste").unwrap();
    assert_eq!(tokens.len(), 1);
    assert_eq!(tokens[0].value, Token::Content("taste".to_string()));
}

#[test]
fn test_taste_with_args_is_content() {
    let tokens = tokenize("taste test").unwrap();
    assert_eq!(tokens.len(), 1);
    assert_eq!(tokens[0].value, Token::Content("taste test".to_string()));
}

#[test]
fn test_blank_line() {
    let tokens = tokenize("").unwrap();
    assert_eq!(tokens.len(), 0); // no lines from empty string
}

#[test]
fn test_whitespace_only_blank() {
    let tokens = tokenize("   ").unwrap();
    assert_eq!(tokens.len(), 1);
    assert_eq!(tokens[0].value, Token::Blank);
}

#[test]
fn test_shell_command() {
    let tokens = tokenize("gcc -o main main.c").unwrap();
    assert_eq!(tokens.len(), 1);
    assert_eq!(
        tokens[0].value,
        Token::Content("gcc -o main main.c".to_string())
    );
}

#[test]
fn test_shell_command_with_double_dash() {
    let tokens = tokenize("cargo test -- --nocapture").unwrap();
    assert_eq!(tokens.len(), 1);
    assert_eq!(
        tokens[0].value,
        Token::Content("cargo test -- --nocapture".to_string())
    );
}

#[test]
fn test_multiline_source() {
    let source = r#"# header comment
recipe "build"
  gcc -o main main.c
"#;
    let tokens = tokenize(source).unwrap();
    assert_eq!(tokens.len(), 3);
    assert_eq!(
        tokens[0].value,
        Token::Comment(" header comment".to_string())
    );
    assert_eq!(tokens[1].value, Token::RecipeHeader { name: "build".to_string(), deps: vec![] });
    assert_eq!(
        tokens[2].value,
        Token::Content("gcc -o main main.c".to_string())
    );
}

#[test]
fn test_indented_recipe_is_content() {
    // CS-0019 (E.5): the `recipe` keyword is recognised only at column 0.
    let tokens = tokenize("    recipe inner").unwrap();
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].value, Token::Content("recipe inner".to_string()));
}

#[test]
fn test_indented_config_is_content() {
    let tokens = tokenize("  config debug").unwrap();
    assert_eq!(tokens.len(), 1);
    assert_eq!(tokens[0].value, Token::Content("config debug".to_string()));
}

#[test]
fn test_indented_use_is_content() {
    let tokens = tokenize("\tuse cpp").unwrap();
    assert_eq!(tokens.len(), 1);
    assert_eq!(tokens[0].value, Token::Content("use cpp".to_string()));
}

#[test]
fn test_indented_import_is_content() {
    let tokens = tokenize("    import backend ./backend").unwrap();
    assert_eq!(tokens.len(), 1);
    assert_eq!(
        tokens[0].value,
        Token::Content("import backend ./backend".to_string()),
    );
}

#[test]
fn test_recipe_bare_name() {
    let tokens = tokenize("recipe build").unwrap();
    assert_eq!(tokens.len(), 1);
    assert_eq!(
        tokens[0].value,
        Token::RecipeHeader { name: "build".to_string(), deps: vec![] }
    );
}

#[test]
fn test_recipe_bare_name_with_deps() {
    let tokens = tokenize("recipe build: lib setup").unwrap();
    assert_eq!(tokens.len(), 1);
    assert_eq!(
        tokens[0].value,
        Token::RecipeHeader {
            name: "build".to_string(),
            deps: vec!["lib".to_string(), "setup".to_string()],
        }
    );
}

#[test]
fn test_recipe_bare_dotted_dep() {
    let tokens = tokenize("recipe bundle: backend.build frontend.build").unwrap();
    assert_eq!(tokens.len(), 1);
    assert_eq!(
        tokens[0].value,
        Token::RecipeHeader {
            name: "bundle".to_string(),
            deps: vec!["backend.build".to_string(), "frontend.build".to_string()],
        }
    );
}

#[test]
fn test_recipe_mixed_quoted_bare_deps() {
    let tokens = tokenize(r#"recipe build: lib "my setup""#).unwrap();
    assert_eq!(tokens.len(), 1);
    assert_eq!(
        tokens[0].value,
        Token::RecipeHeader {
            name: "build".to_string(),
            deps: vec!["lib".to_string(), "my setup".to_string()],
        }
    );
}

#[test]
fn test_missing_recipe_name() {
    let result = tokenize("recipe :");
    assert!(result.is_err());
}

#[test]
fn test_unterminated_recipe_name() {
    let result = tokenize(r#"recipe "build"#);
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(matches!(err, LexError::UnterminatedString { line: 1 }));
}

#[test]
fn test_recipe_header_with_deps() {
    let tokens = tokenize(r#"recipe "build": "setup" "lib""#).unwrap();
    assert_eq!(tokens.len(), 1);
    assert_eq!(
        tokens[0].value,
        Token::RecipeHeader {
            name: "build".to_string(),
            deps: vec!["setup".to_string(), "lib".to_string()],
        }
    );
}

#[test]
fn test_recipe_header_with_one_dep() {
    let tokens = tokenize(r#"recipe "build": "setup""#).unwrap();
    assert_eq!(tokens.len(), 1);
    assert_eq!(
        tokens[0].value,
        Token::RecipeHeader {
            name: "build".to_string(),
            deps: vec!["setup".to_string()],
        }
    );
}

#[test]
fn test_recipe_header_no_deps() {
    let tokens = tokenize(r#"recipe "build""#).unwrap();
    assert_eq!(tokens.len(), 1);
    assert_eq!(
        tokens[0].value,
        Token::RecipeHeader {
            name: "build".to_string(),
            deps: vec![],
        }
    );
}

#[test]
fn test_config_header() {
    let tokens = tokenize(r#"config "debug""#).unwrap();
    assert_eq!(tokens.len(), 1);
    assert_eq!(
        tokens[0].value,
        Token::ConfigHeader {
            name: Some("debug".to_string()),
        }
    );
}

#[test]
fn test_config_header_not_keyword_prefix() {
    // "configure" should be Content, not ConfigHeader
    let tokens = tokenize("configure").unwrap();
    assert_eq!(tokens[0].value, Token::Content("configure".to_string()));
}

#[test]
fn test_use_decl() {
    let tokens = tokenize(r#"use "cpp""#).unwrap();
    assert_eq!(tokens.len(), 1);
    assert_eq!(tokens[0].value, Token::UseDecl { alias: "cpp".to_string(), target: "cpp".to_string() });
}

#[test]
fn test_use_prefix_is_content() {
    let tokens = tokenize("useful").unwrap();
    assert_eq!(tokens[0].value, Token::Content("useful".to_string()));
}

#[test]
fn test_use_bare_name() {
    let tokens = tokenize("use cpp").unwrap();
    assert_eq!(tokens.len(), 1);
    assert_eq!(tokens[0].value, Token::UseDecl { alias: "cpp".to_string(), target: "cpp".to_string() });
}

#[test]
fn test_use_name_with_space_rejected() {
    // CS-0035: `use NAME` becomes `local NAME = cook.load_module(...)`.
    // A name with whitespace is not a valid Lua identifier.
    let result = tokenize(r#"use "has spaces""#);
    assert!(result.is_err(), "expected error for use name with spaces");
    assert!(matches!(
        result.unwrap_err(),
        LexError::InvalidUseName { line: 1, .. }
    ));
}

#[test]
fn test_use_name_with_dash_rejected() {
    // CS-0035: hyphens are rejected — `foo-bar` is not a Lua identifier
    // and avoids the silent `foo-bar` ↔ `foo_bar` collision in codegen.
    let result = tokenize("use foo-bar");
        assert!(result.is_err(), "expected error for use name with dash");
    assert!(matches!(
        result.unwrap_err(),
        LexError::InvalidUseName { line: 1, .. }
    ));
}

#[test]
fn test_use_name_with_dots_rejected() {
    // CS-0035: dotted names like `cpp.bad` are not valid Lua identifiers.
    let result = tokenize("use cpp.bad");
    assert!(result.is_err(), "expected error for dotted use name");
    assert!(matches!(
        result.unwrap_err(),
        LexError::InvalidUseName { line: 1, .. }
    ));
}

#[test]
fn test_use_name_starting_with_digit_rejected() {
    let result = tokenize(r#"use "9lives""#);
    assert!(result.is_err(), "expected error for digit-leading use name");
    assert!(matches!(
        result.unwrap_err(),
        LexError::InvalidUseName { line: 1, .. }
    ));
}

#[test]
fn test_use_name_underscore_accepted() {
    let tokens = tokenize("use my_module").unwrap();
    assert_eq!(
        tokens[0].value,
        Token::UseDecl { alias: "my_module".to_string(), target: "my_module".to_string() }
    );
}

#[test]
fn test_use_name_leading_underscore_accepted() {
    let tokens = tokenize("use _private").unwrap();
    assert_eq!(
        tokens[0].value,
        Token::UseDecl { alias: "_private".to_string(), target: "_private".to_string() }
    );
}

#[test]
fn test_config_bare_name() {
    let tokens = tokenize("config debug").unwrap();
    assert_eq!(tokens.len(), 1);
    assert_eq!(
        tokens[0].value,
        Token::ConfigHeader { name: Some("debug".to_string()) }
    );
}

#[test]
fn test_implicit_form_is_now_content() {
    // CS-0018 (E.6): the bare `name: deps` line at column 0, formerly
    // an implicit recipe header, is now a `Content` token. Inside a
    // recipe body it would dispatch as a `shell_command`; at top level
    // it is rejected as not a valid `toplevel_item`.
    let tokens = tokenize("build: lib setup").unwrap();
    assert_eq!(tokens.len(), 1);
    assert_eq!(
        tokens[0].value,
        Token::Content("build: lib setup".to_string()),
    );
}

#[test]
fn test_bare_colon_line_at_column_0_is_content() {
    let tokens = tokenize("clean:").unwrap();
    assert_eq!(tokens.len(), 1);
    assert_eq!(tokens[0].value, Token::Content("clean:".to_string()));
}

#[test]
fn test_import_decl() {
    let tokens = tokenize("import backend ./services/backend").unwrap();
    assert_eq!(tokens.len(), 1);
    assert_eq!(
        tokens[0].value,
        Token::ImportDecl {
            name: "backend".to_string(),
            path: "./services/backend".to_string(),
        }
    );
}

#[test]
fn test_import_decl_relative_parent() {
    let tokens = tokenize("import proto ../../libs/proto").unwrap();
    assert_eq!(tokens.len(), 1);
    assert_eq!(
        tokens[0].value,
        Token::ImportDecl {
            name: "proto".to_string(),
            path: "../../libs/proto".to_string(),
        }
    );
}

#[test]
fn test_import_prefix_is_content() {
    let tokens = tokenize("important").unwrap();
    assert_eq!(tokens[0].value, Token::Content("important".to_string()));
}

#[test]
fn test_import_missing_path() {
    let result = tokenize("import backend");
    assert!(result.is_err());
}

#[test]
fn test_import_missing_name_and_path() {
    let tokens = tokenize("import").unwrap();
    assert_eq!(tokens[0].value, Token::Content("import".to_string()));
}

#[test]
fn test_bare_config_keyword_tokenizes() {
    let tokens = tokenize("config").unwrap();
    assert_eq!(tokens.len(), 1);
    assert_eq!(tokens[0].value, Token::ConfigHeader { name: None });
}

#[test]
fn test_named_config_keyword_tokenizes() {
    let tokens = tokenize("config release").unwrap();
    assert_eq!(tokens.len(), 1);
    assert_eq!(
        tokens[0].value,
        Token::ConfigHeader { name: Some("release".to_string()) }
    );
}

#[test]
fn test_config_prefix_not_a_token() {
    // "configure" starts with "config" but is a bareword command
    let tokens = tokenize("configure --prefix=/usr").unwrap();
        assert!(!matches!(tokens[0].value, Token::ConfigHeader { .. }));
    }

    #[test]
    fn test_chore_header_bare_name() {
        let tokens = tokenize("chore clean").unwrap();
    assert_eq!(tokens.len(), 1);
    assert_eq!(
        tokens[0].value,
        Token::ChoreHeader { name: "clean".to_string(), params: vec![], deps: vec![] },
    );
}

#[test]
fn test_chore_header_quoted_name() {
    let tokens = tokenize(r#"chore "play""#).unwrap();
    assert_eq!(
        tokens[0].value,
        Token::ChoreHeader { name: "play".to_string(), params: vec![], deps: vec![] },
    );
}

#[test]
fn test_chore_header_with_deps() {
    let tokens = tokenize("chore play: build setup").unwrap();
    assert_eq!(
        tokens[0].value,
        Token::ChoreHeader {
            name: "play".to_string(),
            params: vec![],
            deps: vec!["build".to_string(), "setup".to_string()],
        },
    );
}

#[test]
fn test_chore_prefix_is_content() {
    let tokens = tokenize("chores_cleanup").unwrap();
    assert_eq!(tokens[0].value, Token::Content("chores_cleanup".to_string()));
}

#[test]
fn test_indented_chore_is_content() {
    let tokens = tokenize("    chore inner").unwrap();
        assert_eq!(tokens[0].value, Token::Content("chore inner".to_string()));
}

#[test]
fn former_reserved_words_allowed_as_chore_params() {
    // CS-0132: the reserved-segment ban no longer applies to chore param names.
    for word in &["stem", "name", "ext", "dir", "in", "out", "env"] {
        let input = format!("chore deploy {}\n    > do_thing()\n", word);
        assert!(
            crate::parse(&input).is_ok(),
            "chore param named '{}' must parse (CS-0132), got err",
                word
            );
        }
    }

    #[test]
    fn former_reserved_words_allowed_as_undotted_decl_names() {
        // CS-0132: the reserved-segment ban no longer applies to undotted
        // recipe/chore DECLARATION names.
        for word in &["stem", "name", "ext", "dir", "in", "out", "env"] {
        let recipe = format!("recipe {}\n    ingredients \"src/*.c\"\n    cook \"o/$<in.stem>.o\" {{ cc -c $<in> -o $<out> }}\n", word);
        assert!(crate::parse(&recipe).is_ok(), "recipe named '{}' must parse (CS-0132)", word);
        let chore = format!("chore {}\n    > do_thing()\n", word);
        assert!(crate::parse(&chore).is_ok(), "chore named '{}' must parse (CS-0132)", word);
    }
}

#[test]
fn dotted_env_decl_name_still_reserved_diagnostic() {
    // CS-0132 keeps the reserved check on the dotted path: env.foo at a
    // declaration site keeps the specific "reserved word" diagnostic rather
    // than falling through to the generic dotted-name rejection.
    let input = "recipe \"env.foo\"\n    echo hi\n";
    match crate::parse(input) {
        Err(e) => assert!(
            e.to_string().contains("reserved word"),
            "expected reserved-word diagnostic for 'env.foo', got: {e}"
        ),
        Ok(_) => panic!("recipe env.foo must be rejected"),
    }
}

#[test]
fn recipe_named_all_is_allowed() {
    // all is no longer a reserved recipe segment.
    let src = "recipe all\n    ingredients \"src/*.c\"\n    cook \"out/$<in.stem>.o\" { cc -c $<in> -o $<out> }\n";
    assert!(crate::parse(src).is_ok(), "recipe all must parse");
}

#[test]
fn test_dotted_declared_recipe_name_rejected() {
    let input = "recipe backend.build\n    echo hi\n";
    let result = tokenize(input);
    match result {
        Err(LexError::DottedDeclaredRecipeName { ref name, line: 1 }) if name == "backend.build" => {}
        other => panic!("expected DottedDeclaredRecipeName for 'backend.build', got: {:?}", other),
    }
}

#[test]
fn test_dotted_declared_recipe_name_quoted_rejected() {
    let input = "recipe \"backend.build\"\n    echo hi\n";
    let result = tokenize(input);
    match result {
        Err(LexError::DottedDeclaredRecipeName { ref name, line: 1 }) if name == "backend.build" => {}
        other => panic!("expected DottedDeclaredRecipeName for quoted 'backend.build', got: {:?}", other),
    }
}

#[test]
fn test_dotted_declared_chore_name_rejected() {
    let input = "chore tools.fmt\n    echo hi\n";
    let result = tokenize(input);
    match result {
        Err(LexError::DottedDeclaredChoreName { ref name, line: 1 }) if name == "tools.fmt" => {}
        other => panic!("expected DottedDeclaredChoreName for 'tools.fmt', got: {:?}", other),
    }
}

#[test]
fn test_undotted_recipe_with_dotted_dep_accepted() {
    // The no-dots rule is at the *declaration* site; dotted dep references
    // remain legal because they resolve through `import` aliases.
    let input = "recipe ship: backend.build frontend.build\n    echo deploy\n";
    let result = tokenize(input);
    assert!(result.is_ok(), "expected ok for undotted recipe with dotted deps, got: {:?}", result.err());
}

#[test]
fn test_register_header_bare() {
    let tokens = tokenize("register").unwrap();
        assert_eq!(tokens[0].value, Token::RegisterHeader);
    }

    #[test]
    fn test_register_header_with_trailing_whitespace() {
        let tokens = tokenize("register   ").unwrap();
    assert_eq!(tokens[0].value, Token::RegisterHeader);
}

#[test]
fn test_register_header_followed_by_content_is_still_register() {
    // Lexer admits the RegisterHeader; the parser rejects `register foo`.
    let tokens = tokenize("register foo").unwrap();
    assert_eq!(tokens[0].value, Token::RegisterHeader);
}

#[test]
fn test_register_header_with_tab_separator() {
    let tokens = tokenize("register\tfoo").unwrap();
    assert_eq!(tokens[0].value, Token::RegisterHeader);
}

#[test]
fn test_indented_register_is_content() {
    let tokens = tokenize("    register").unwrap();
        assert_eq!(tokens[0].value, Token::Content("register".to_string()));
}

#[test]
fn test_indented_register_keyword_with_arg_is_content() {
    let tokens = tokenize("    register foo").unwrap();
    assert_eq!(tokens[0].value, Token::Content("register foo".to_string()));
}

#[test]
fn test_register_prefix_is_content() {
    // `registers_cleanup` starts with `register` but is a bareword.
    let tokens = tokenize("registers_cleanup").unwrap();
    assert_eq!(tokens[0].value, Token::Content("registers_cleanup".to_string()));
}

#[test]
fn test_register_underscore_is_content() {
    let tokens = tokenize("register_foo").unwrap();
    assert_eq!(tokens[0].value, Token::Content("register_foo".to_string()));
}

// ── COOK-67: probe header lexing ──────────────────────────────────────────

#[test]
fn probe_header_bare_name() {
    let t = tokenize("probe cards").unwrap();
    assert_eq!(t[0].value, Token::ProbeHeader { name: "cards".into(), deps: vec![] });
}

#[test]
fn probe_header_module_prefixed_name() {
    let t = tokenize("probe cc:zlib").unwrap();
    assert_eq!(t[0].value, Token::ProbeHeader { name: "cc:zlib".into(), deps: vec![] });
}

#[test]
fn probe_header_dep_list() {
    let t = tokenize("probe cards: services_raw other").unwrap();
        assert_eq!(t[0].value, Token::ProbeHeader {
            name: "cards".into(),
        deps: vec!["services_raw".into(), "other".into()],
        });
    }

    #[test]
    fn probe_header_prefixed_name_and_dep() {
        let t = tokenize("probe cc:zlib: cc:compiler").unwrap();
    assert_eq!(t[0].value, Token::ProbeHeader {
        name: "cc:zlib".into(), deps: vec!["cc:compiler".into()],
        });
    }

    #[test]
    fn probe_header_hyphenated_bare_name() {
        // COOK-71 sub-gap 1: a hyphen in a bare probe key must tokenise as one name.
        let t = tokenize("probe demo:cc-version").unwrap();
    assert_eq!(t[0].value, Token::ProbeHeader {
        name: "demo:cc-version".into(), deps: vec![],
    });
}

#[test]
fn probe_header_hyphenated_bare_dep() {
    // COOK-71 sub-gap 2 (bare arm): a hyphenated upstream key in the dep list.
    let t = tokenize("probe x: demo:cc-path").unwrap();
    assert_eq!(t[0].value, Token::ProbeHeader {
        name: "x".into(), deps: vec!["demo:cc-path".into()],
    });
}

#[test]
fn probe_header_dotted_bare_name_stops_at_the_dot() {
    // CS-0201 removed '.' from PROBE_SEG: it is member access in a probe
    // reference, so a dot inside a segment makes `$<cc:zlib.dev>` ambiguous
    // between "field dev of cc:zlib" and "the key cc:zlib.dev". CS-0131 added
    // the dot before references had member access; member access is worth more.
    // The name now ends at the dot, and the remainder is the header's dep list
    // position, which rejects it.
    assert!(
        tokenize("probe cc:zlib.dev").is_err(),
        "a dotted bare probe name must not lex as one key"
    );
    // The quoted form remains the escape hatch for exactly this spelling.
    let t = tokenize("probe \"cc:zlib.dev\"").unwrap();
    assert_eq!(t[0].value, Token::ProbeHeader {
        name: "cc:zlib.dev".into(), deps: vec![],
    });
}

#[test]
fn probe_header_quoted_hyphenated_dep() {
    // COOK-71 sub-gap 2 (quoted arm): the dep list gains the STRING escape hatch
    // already blessed by App. A.3.2 L168 (probe_ref ::= BARE_PROBE_KEY | STRING).
    let t = tokenize("probe x: \"demo:cc-path\"").unwrap();
    assert_eq!(t[0].value, Token::ProbeHeader {
        name: "x".into(), deps: vec!["demo:cc-path".into()],
    });
}

#[test]
fn probe_header_mixed_bare_and_quoted_deps() {
    let t = tokenize("probe x: alpha \"cc:beta-1\" gamma").unwrap();
    assert_eq!(t[0].value, Token::ProbeHeader {
        name: "x".into(),
        deps: vec!["alpha".into(), "cc:beta-1".into(), "gamma".into()],
    });
}

#[test]
fn probe_header_quoted_name() {
    let t = tokenize("probe \"cc:zlib\": cc:compiler").unwrap();
        assert_eq!(t[0].value, Token::ProbeHeader {
            name: "cc:zlib".into(), deps: vec!["cc:compiler".into()],
    });
}

#[test]
fn probe_keyword_only_at_column_zero() {
    let t = tokenize("    probe cards").unwrap();
    assert!(matches!(t[0].value, Token::Content(_)));
}

#[test]
fn probe_name_accepts_three_or_more_segments() {
    // CS-0201: the two-segment cap was enforced here and nowhere else.
    // `cook.probe()` validated nothing, so modules mint `cc:find:raylib` and
    // `cc:compiler:auto` as their ordinary case, and those keys could not be
    // spelled on the surface or sealed. A cap one of two declaration paths
    // enforces is an obstacle rather than a rule.
    let t = tokenize("probe a:b:c").unwrap();
    assert_eq!(t[0].value, Token::ProbeHeader { name: "a:b:c".into(), deps: vec![] });

    let t = tokenize("probe cc:find:raylib").unwrap();
    assert_eq!(t[0].value, Token::ProbeHeader { name: "cc:find:raylib".into(), deps: vec![] });
}

#[test]
fn probe_name_accepts_hyphens_in_every_segment() {
    // The COOK-408 case: declarable and sigil-referenceable, but `seal` and
    // `ingredients` rejected it, so the key could be neither pinned nor
    // consumed.
    let t = tokenize("probe demo:cc-version").unwrap();
    assert_eq!(t[0].value, Token::ProbeHeader { name: "demo:cc-version".into(), deps: vec![] });
}

#[test]
fn probe_keyword_needs_separator() {
    let t = tokenize("probexyz cards").unwrap();
    assert!(matches!(t[0].value, Token::Content(_)));
}

#[test]
fn probe_quoted_name_extra_token_rejected() {
    // quoted name with trailing non-colon garbage -> ProbeExtraTokens
    let err = tokenize("probe \"foo\" extra").unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("probe"), "message should mention probe, got: {msg}");
    assert!(!msg.contains("recipe"), "probe error must not say 'recipe', got: {msg}");
}

#[test]
fn probe_missing_colon_before_deps_rejected() {
    // `probe foo bar` (no colon) -> ProbeExtraTokens, not a recipe error
    let err = tokenize("probe foo bar").unwrap_err();
        let msg = format!("{err}");
    assert!(!msg.contains("recipe"), "probe error must not say 'recipe', got: {msg}");
}

#[test]
fn probe_dep_list_accepts_multi_segment_keys() {
    // CS-0201: a dep names a probe, so it takes the same grammar the
    // declaration does. Modules depend on `cc:find:<name>` routinely.
    let t = tokenize("probe good: a:b:c").unwrap();
    assert_eq!(t[0].value, Token::ProbeHeader {
        name: "good".into(), deps: vec!["a:b:c".into()],
    });
}

// ---------------------------------------------------------------------------
// CS-0206: the `use` path form, at the lexer seam
// ---------------------------------------------------------------------------
//
// The corpus (fixtures 063-070) covers these end to end, but a corpus case is
// a coarse gate: it reports "this Cookfile was rejected" and the substring it
// was rejected with. These pin the token and the error VARIANT, which is where
// a wrong-but-plausible refactor shows up first.

fn use_token(src: &str) -> (String, String) {
    let tokens = tokenize(src).expect("must lex");
    match &tokens[0].value {
        Token::UseDecl { alias, target } => (alias.clone(), target.clone()),
        other => panic!("expected UseDecl, got {other:?}"),
    }
}

fn use_error(src: &str) -> LexError {
    tokenize(src).expect_err("must not lex")
}

#[test]
fn a_bare_path_binds_its_basename_and_normalises_the_target() {
    assert_eq!(
        use_token("use ./lua/helpers.lua\n"),
        ("helpers".to_string(), "lua/helpers.lua".to_string())
    );
}

#[test]
fn a_quoted_path_is_equivalent_to_the_bare_one() {
    assert_eq!(
        use_token("use \"./lua/helpers.lua\"\n"),
        use_token("use ./lua/helpers.lua\n")
    );
}

#[test]
fn an_explicit_alias_wins_over_the_basename() {
    assert_eq!(
        use_token("use fmt ./lua/code-formatting.lua\n"),
        ("fmt".to_string(), "lua/code-formatting.lua".to_string())
    );
    // Quoted on both sides.
    assert_eq!(
        use_token("use \"fmt\" \"./lua/code-formatting.lua\"\n"),
        ("fmt".to_string(), "lua/code-formatting.lua".to_string())
    );
}

#[test]
fn a_hyphenated_basename_derives_an_underscore_alias() {
    // COOK-436: §12.1's rewrite reaching its first live input.
    assert_eq!(
        use_token("use ./lua/my-helpers.lua\n"),
        ("my_helpers".to_string(), "lua/my-helpers.lua".to_string())
    );
}

#[test]
fn the_name_form_is_unchanged() {
    assert_eq!(
        use_token("use cpp\n"),
        ("cpp".to_string(), "cpp".to_string())
    );
    assert_eq!(
        use_token("use \"proto\"\n"),
        ("proto".to_string(), "proto".to_string())
    );
}

#[test]
fn containment_violations_are_refused_with_their_own_reasons() {
    for (src, needle) in [
        ("use ../shared/helpers.lua\n", "'..' segments are not permitted"),
        ("use /opt/cook/helpers.lua\n", "absolute paths are not permitted"),
        // The sigil must report as a sigil, not as the absolute path it also
        // is: the remedy is different.
        ("use //lua/helpers.lua\n", "'//' workspace-root sigil"),
        ("use \"./my helpers.lua\"\n", "whitespace is not permitted"),
    ] {
        let err = use_error(src).to_string();
        assert!(err.contains(needle), "{src:?} -> {err}");
        assert!(matches!(
            use_error(src),
            LexError::InvalidUsePath { .. }
        ));
    }
}

#[test]
fn a_derived_alias_that_is_not_an_identifier_names_the_explicit_form() {
    let err = use_error("use ./lua/9lives.lua\n");
    assert!(matches!(err, LexError::UnusableDerivedAlias { .. }));
    let msg = err.to_string();
    // The author never typed `9lives`; the basename of the file they named
    // did. The remedy has to be the explicit form.
    assert!(msg.contains("'9lives'"), "{msg}");
    assert!(msg.contains("use <alias> ./lua/9lives.lua"), "{msg}");
}

#[test]
fn the_two_argument_form_rejects_a_second_argument_that_is_not_a_path() {
    // Both halves of the two-argument shape, each with its own variant.
    assert!(matches!(
        use_error("use cpp proto\n"),
        LexError::UseNameWithExtraArgument { .. }
    ));
    assert!(matches!(
        use_error("use ./lua/a.lua trailing\n"),
        LexError::UseSecondArgumentNotAPath { .. }
    ));
}

#[test]
fn more_than_two_arguments_is_refused() {
    assert!(matches!(
        use_error("use a b ./lua/c.lua\n"),
        LexError::UseTooManyArguments { .. }
    ));
}

#[test]
fn an_alias_that_is_not_an_identifier_is_reported_as_an_alias() {
    // `a.lua` sits in the ALIAS position here. Calling it a "name" would point
    // the author at the wrong argument.
    let err = use_error("use a.lua b.lua\n");
    match &err {
        LexError::InvalidUseName { position, .. } => {
            assert_eq!(*position, UseNamePosition::Alias)
        }
        other => panic!("expected InvalidUseName, got {other:?}"),
    }
    assert!(err.to_string().contains("'use' alias 'a.lua'"), "{err}");
}

#[test]
fn a_trailing_comment_is_refused_by_name_not_as_an_extra_argument() {
    // CS-0206 TIGHTENED this: before it, the lexer parsed the first token and
    // threw the rest of the line away, so `use cpp # note` silently worked —
    // while `tree-sitter-cook` had always rejected it. The two readers now
    // agree, and the diagnostic says what is actually wrong rather than
    // counting `#` as an argument.
    let err = use_error("use cpp # the C module\n");
    assert!(matches!(err, LexError::UseTrailingComment { .. }));
    assert!(err.to_string().contains("takes no trailing comment"), "{err}");
}
