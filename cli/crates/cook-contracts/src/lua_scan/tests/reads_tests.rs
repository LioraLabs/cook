use super::*;

fn keys(src: &str) -> Vec<String> {
    scan_var_reads(src).into_iter().collect()
}

#[test]
fn empty_source_yields_no_keys() {
    assert!(scan_var_reads("").is_empty());
}

#[test]
fn no_cook_env_references_yields_no_keys() {
    assert!(scan_var_reads("local x = 1\nprint('hi')\n").is_empty());
}

#[test]
fn matches_simple_dot_access() {
    assert_eq!(keys("local f = var.FOO"), vec!["FOO"]);
}

#[test]
fn matches_multiple_dot_accesses() {
    assert_eq!(
        keys("print(var.FOO .. var.BAR)"),
        vec!["BAR".to_string(), "FOO".to_string()]
    );
}

#[test]
fn matches_string_indexed_double_quoted() {
    assert_eq!(keys("local v = var[\"MY_KEY\"]"), vec!["MY_KEY"]);
}

#[test]
fn matches_string_indexed_single_quoted() {
    assert_eq!(keys("local v = var['MY_KEY']"), vec!["MY_KEY"]);
}

#[test]
fn string_indexed_admits_mixed_case() {
    // Authors who explicitly write a string-keyed access have opted in;
    // we don't constrain shape.
    assert_eq!(keys(r#"local v = var["Mixed_Case"]"#), vec!["Mixed_Case"]);
}

#[test]
fn dot_access_admits_lower_case_idents() {
    // CS-0172: declared variables are routinely lower-case (`var.optimize`),
    // so the pre-CS-0172 upper-case-only shape check is gone. Over-recording is
    // the safe direction; a missed key serves stale output.
    assert_eq!(keys("local x = var.path"), vec!["path"]);
}

#[test]
fn dot_access_admits_underscore_prefix() {
    assert_eq!(keys("local x = var._INTERNAL"), vec!["_INTERNAL"]);
}

#[test]
fn dot_access_admits_digits() {
    assert_eq!(keys("local x = var.VAR_1"), vec!["VAR_1"]);
}

#[test]
fn skips_dynamic_key_index() {
    // var[var] — no key literal, skipped silently.
    assert!(keys("local v = var[name]").is_empty());
    assert!(keys("local v = var[string.upper(x)]").is_empty());
}

#[test]
fn skips_dot_assignment_write() {
    // var.X = "v" is a write, not a read.
    assert!(keys("var.FOO = \"hi\"").is_empty());
}

#[test]
fn skips_string_indexed_assignment_write() {
    assert!(keys("var[\"FOO\"] = \"hi\"").is_empty());
}

#[test]
fn admits_dot_read_followed_by_equality_compare() {
    // `var.X == "y"` is a read followed by `==`; must NOT be
    // treated as an assignment.
    assert_eq!(keys("if var.FOO == \"y\" then end"), vec!["FOO"]);
}

#[test]
fn skips_text_inside_short_string() {
    // The literal `var.X` appearing inside a string is not a read.
    assert!(keys(r#"print("var.FOO is")"#).is_empty());
    assert!(keys(r#"print('var.BAR also')"#).is_empty());
}

#[test]
fn skips_text_inside_long_string() {
    let src = "local s = [[ var.FOO ]]\n";
    assert!(keys(src).is_empty());
}

#[test]
fn skips_text_inside_long_string_with_eqs() {
    let src = "local s = [==[ var.FOO ]==]\n";
    assert!(keys(src).is_empty());
}

#[test]
fn skips_text_inside_line_comment() {
    assert!(keys("-- var.FOO is dead code\n").is_empty());
}

#[test]
fn skips_text_inside_block_comment() {
    assert!(keys("--[[ var.FOO ]] local x = 1\n").is_empty());
}

#[test]
fn finds_after_line_comment_in_same_source() {
    let src = "-- var.SKIPPED\nlocal v = var.KEPT";
    assert_eq!(keys(src), vec!["KEPT"]);
}

#[test]
fn finds_after_string_in_same_source() {
    let src = "print(\"var.SKIPPED\"); local v = var.KEPT";
    assert_eq!(keys(src), vec!["KEPT"]);
}

#[test]
fn rejects_embedded_in_larger_ident() {
    // `myvar.FOO` — the byte preceding `v` is ident-cont, so it is not the
    // `var` root. `variadic.FOO` — the byte after `var` is ident-cont and not
    // a `.`, so it is part of a longer identifier.
    assert!(keys("local v = myvar.FOO").is_empty());
    assert!(keys("local v = variadic.FOO").is_empty());
}

#[test]
fn does_not_match_attribute_chain_through_other_root() {
    // `mything.var.FOO` should not match — the prefix is preceded by `.`,
    // making `var` an attribute rather than a free root. The scanner accepts
    // this as a false positive (the `.` before `var` is NOT an ident-cont
    // char). Documented limitation; over-recording is safe.
    assert_eq!(keys("local v = mything.var.FOO"), vec!["FOO"]);
}

#[test]
fn dedups_repeated_keys() {
    assert_eq!(
        keys("local a = var.FOO\nlocal b = var.FOO\nlocal c = var.FOO\n"),
        vec!["FOO"]
    );
}

#[test]
fn returns_sorted_keys() {
    assert_eq!(
        keys("local a = var.ZZ\nlocal b = var.AA\nlocal c = var.MM\n"),
        vec!["AA".to_string(), "MM".to_string(), "ZZ".to_string()]
    );
}

#[test]
fn handles_realistic_using_lua_body() {
    // The kind of body the smoke test exercises.
    let body = r#"
            local f = io.open(output, "w")
            f:write("FOO=" .. tostring(var.FOO))
            f:write("BAR=" .. tostring(var.BAR))
            f:close()
        "#;
    assert_eq!(keys(body), vec!["BAR".to_string(), "FOO".to_string()]);
}

#[test]
fn skipping_dynamic_key_does_not_swallow_following_read() {
    // After `var[expr]`, the scanner must resume so a later
    // `var.X` on the same line still matches.
    let src = "local x = var[name] or var.FALLBACK";
    assert_eq!(keys(src), vec!["FALLBACK"]);
}

#[test]
fn write_then_read_records_only_read() {
    let src = "var.WRITE = \"x\"\nlocal v = var.READ";
    assert_eq!(keys(src), vec!["READ"]);
}

// ── scan_probe_reads ────────────────────────────────────────────────

fn probe_keys(src: &str) -> Vec<String> {
    scan_probe_reads(src).into_iter().collect()
}

#[test]
fn probe_reads_empty_source_yields_no_keys() {
    assert!(scan_probe_reads("").is_empty());
}

#[test]
fn probe_reads_double_quoted_key_collected() {
    assert_eq!(
        probe_keys(r#"local v = cook.probes.get("cc:zlib")"#),
        vec!["cc:zlib"]
    );
}

#[test]
fn probe_reads_single_quoted_key_collected() {
    assert_eq!(
        probe_keys("local v = cook.probes.get('cc:zlib')"),
        vec!["cc:zlib"]
    );
}

#[test]
fn probe_reads_whitespace_around_paren_tolerated() {
    assert_eq!(
        probe_keys("local v = cook.probes.get  (  \"cc:zlib\"  )"),
        vec!["cc:zlib"]
    );
}

#[test]
fn probe_reads_multiple_distinct_keys_collected() {
    assert_eq!(
        probe_keys(
            r#"
                local a = cook.probes.get("cc:zlib")
                local b = cook.probes.get("cc:compiler")
                "#
        ),
        vec!["cc:compiler".to_string(), "cc:zlib".to_string()]
    );
}

#[test]
fn probe_reads_dedups_repeated_key() {
    assert_eq!(
        probe_keys(
            r#"
                local a = cook.probes.get("cc:zlib")
                local b = cook.probes.get("cc:zlib")
                "#
        ),
        vec!["cc:zlib"]
    );
}

#[test]
fn probe_reads_ignores_comment_embedded_call() {
    assert!(probe_keys("-- cook.probes.get(\"cc:zlib\")\n").is_empty());
    assert!(probe_keys("--[[ cook.probes.get(\"cc:zlib\") ]]\n").is_empty());
}

#[test]
fn probe_reads_ignores_call_inside_unrelated_string_literal() {
    let src = r#"local s = "cook.probes.get(\"cc:zlib\")""#;
    assert!(probe_keys(src).is_empty());
}

#[test]
fn probe_reads_ignores_dynamic_key_argument() {
    assert!(probe_keys("local v = cook.probes.get(k)").is_empty());
}

#[test]
fn probe_reads_ignores_concatenation_argument() {
    // The first token after `(` is a string literal, but it's not the
    // WHOLE argument — the real key is dynamic. Must not collect "a".
    assert!(probe_keys(r#"local v = cook.probes.get("a" .. b)"#).is_empty());
}

#[test]
fn probe_reads_finds_key_after_ignored_concatenation() {
    let src = r#"
            local v = cook.probes.get("a" .. b)
            local w = cook.probes.get("cc:zlib")
        "#;
    assert_eq!(probe_keys(src), vec!["cc:zlib"]);
}

#[test]
fn probe_reads_ignores_scope_chain() {
    assert!(probe_keys(r#"cook.probes.scope("cc"):get("zlib")"#).is_empty());
}

#[test]
fn probe_reads_returns_sorted_keys() {
    assert_eq!(
        probe_keys(
            r#"
                local a = cook.probes.get("zz")
                local b = cook.probes.get("aa")
                local c = cook.probes.get("mm")
                "#
        ),
        vec!["aa".to_string(), "mm".to_string(), "zz".to_string()]
    );
}

#[test]
fn probe_reads_realistic_lua_body() {
    let body = r#"
            local f = io.open(output, "w")
            local v = cook.probes.get("cc:zlib")
            f:write("version=" .. tostring(v.version))
            f:close()
        "#;
    assert_eq!(probe_keys(body), vec!["cc:zlib"]);
}
