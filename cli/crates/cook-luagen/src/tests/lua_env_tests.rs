use super::*;

fn keys(src: &str) -> Vec<String> {
    scan_env_reads(src).into_iter().collect()
}

#[test]
fn empty_source_yields_no_keys() {
    assert!(scan_env_reads("").is_empty());
}

#[test]
fn no_cook_env_references_yields_no_keys() {
    assert!(scan_env_reads("local x = 1\nprint('hi')\n").is_empty());
}

#[test]
fn matches_simple_dot_access() {
    assert_eq!(keys("local f = cook.env.FOO"), vec!["FOO"]);
}

#[test]
fn matches_multiple_dot_accesses() {
    assert_eq!(
        keys("print(cook.env.FOO .. cook.env.BAR)"),
        vec!["BAR".to_string(), "FOO".to_string()]
    );
}

#[test]
fn matches_string_indexed_double_quoted() {
    assert_eq!(keys("local v = cook.env[\"MY_KEY\"]"), vec!["MY_KEY"]);
}

#[test]
fn matches_string_indexed_single_quoted() {
    assert_eq!(keys("local v = cook.env['MY_KEY']"), vec!["MY_KEY"]);
}

#[test]
fn string_indexed_admits_mixed_case() {
    // Authors who explicitly write a string-keyed access have opted in;
    // we don't constrain shape.
    assert_eq!(keys(r#"local v = cook.env["Mixed_Case"]"#), vec!["Mixed_Case"]);
}

#[test]
fn dot_access_skips_lower_case_idents() {
    // `cook.env.path` is unlikely to be an env key — by convention env
    // keys are upper-case. Skipping cuts noise from idioms like
    // `cook.env.path` that aren't env-keys at all.
    assert!(keys("local x = cook.env.path").is_empty());
}

#[test]
fn dot_access_admits_underscore_prefix() {
    assert_eq!(keys("local x = cook.env._INTERNAL"), vec!["_INTERNAL"]);
}

#[test]
fn dot_access_admits_digits() {
    assert_eq!(keys("local x = cook.env.VAR_1"), vec!["VAR_1"]);
}

#[test]
fn skips_dynamic_key_index() {
    // cook.env[var] — no key literal, skipped silently.
    assert!(keys("local v = cook.env[name]").is_empty());
    assert!(keys("local v = cook.env[string.upper(x)]").is_empty());
}

#[test]
fn skips_dot_assignment_write() {
    // cook.env.X = "v" is a write, not a read.
    assert!(keys("cook.env.FOO = \"hi\"").is_empty());
}

#[test]
fn skips_string_indexed_assignment_write() {
    assert!(keys("cook.env[\"FOO\"] = \"hi\"").is_empty());
}

#[test]
fn admits_dot_read_followed_by_equality_compare() {
    // `cook.env.X == "y"` is a read followed by `==`; must NOT be
    // treated as an assignment.
    assert_eq!(keys("if cook.env.FOO == \"y\" then end"), vec!["FOO"]);
}

#[test]
fn skips_text_inside_short_string() {
    // The literal `cook.env.X` appearing inside a string is not a read.
    assert!(keys(r#"print("cook.env.FOO is")"#).is_empty());
    assert!(keys(r#"print('cook.env.BAR also')"#).is_empty());
}

#[test]
fn skips_text_inside_long_string() {
    let src = "local s = [[ cook.env.FOO ]]\n";
    assert!(keys(src).is_empty());
}

#[test]
fn skips_text_inside_long_string_with_eqs() {
    let src = "local s = [==[ cook.env.FOO ]==]\n";
    assert!(keys(src).is_empty());
}

#[test]
fn skips_text_inside_line_comment() {
    assert!(keys("-- cook.env.FOO is dead code\n").is_empty());
}

#[test]
fn skips_text_inside_block_comment() {
    assert!(keys("--[[ cook.env.FOO ]] local x = 1\n").is_empty());
}

#[test]
fn finds_after_line_comment_in_same_source() {
    let src = "-- cook.env.SKIPPED\nlocal v = cook.env.KEPT";
    assert_eq!(keys(src), vec!["KEPT"]);
}

#[test]
fn finds_after_string_in_same_source() {
    let src = "print(\"cook.env.SKIPPED\"); local v = cook.env.KEPT";
    assert_eq!(keys(src), vec!["KEPT"]);
}

#[test]
fn rejects_embedded_in_larger_ident() {
    // `_cook.env.X` would still hit the literal — the leading char is
    // not an ident-cont char for the byte preceding `c`. But
    // `xcook.env.X` — the byte preceding `c` IS `x`, ident-cont — must
    // be skipped.
    assert!(keys("local v = xcook.env.FOO").is_empty());
}

#[test]
fn does_not_match_attribute_chain_through_other_root() {
    // `mything.cook.env.X` should not match — the prefix is preceded
    // by `.` which makes `cook` an attribute, not a free root. The
    // current scanner accepts this as a false positive (the `.` before
    // `cook` is NOT an ident-cont char). Documented limitation.
    assert_eq!(keys("local v = mything.cook.env.FOO"), vec!["FOO"]);
}

#[test]
fn dedups_repeated_keys() {
    assert_eq!(
        keys("local a = cook.env.FOO\nlocal b = cook.env.FOO\nlocal c = cook.env.FOO\n"),
        vec!["FOO"]
    );
}

#[test]
fn returns_sorted_keys() {
    assert_eq!(
        keys("local a = cook.env.ZZ\nlocal b = cook.env.AA\nlocal c = cook.env.MM\n"),
        vec!["AA".to_string(), "MM".to_string(), "ZZ".to_string()]
    );
}

#[test]
fn handles_realistic_using_lua_body() {
    // The kind of body the smoke test exercises.
    let body = r#"
            local f = io.open(output, "w")
            f:write("FOO=" .. tostring(cook.env.FOO))
            f:write("BAR=" .. tostring(cook.env.BAR))
            f:close()
        "#;
    assert_eq!(keys(body), vec!["BAR".to_string(), "FOO".to_string()]);
}

#[test]
fn skipping_dynamic_key_does_not_swallow_following_read() {
    // After `cook.env[expr]`, the scanner must resume so a later
    // `cook.env.X` on the same line still matches.
    let src = "local x = cook.env[name] or cook.env.FALLBACK";
    assert_eq!(keys(src), vec!["FALLBACK"]);
}

#[test]
fn write_then_read_records_only_read() {
    let src = "cook.env.WRITE = \"x\"\nlocal v = cook.env.READ";
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
