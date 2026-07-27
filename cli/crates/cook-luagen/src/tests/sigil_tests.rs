use super::*;

fn idents(text: &str) -> Vec<String> {
    scan(text).into_iter().map(|s| s.ident).collect()
}

#[test]
fn matches_simple_ident() {
    assert_eq!(idents("echo $<HOME>"), vec!["HOME"]);
}

#[test]
fn matches_dotted_ident() {
    assert_eq!(idents("$<in.stem>.o"), vec!["in.stem"]);
}

#[test]
fn matches_out_indexed() {
    assert_eq!(idents("cp src $<out_1> $<out_2>"), vec!["out_1", "out_2"]);
}

#[test]
fn matches_out_indexed_accessor() {
    assert_eq!(idents("$<out_1.stem>"), vec!["out_1.stem"]);
}

#[test]
fn matches_multiple_in_one_string() {
    assert_eq!(
        idents("gcc -c $<in> -o $<out>"),
        vec!["in", "out"]
    );
}

#[test]
fn rejects_empty_ident() {
    assert!(scan("$<>").is_empty());
}

#[test]
fn rejects_ident_starting_with_digit() {
    assert!(scan("$<1foo>").is_empty());
}

#[test]
fn rejects_ident_with_space() {
    assert!(scan("$<foo bar>").is_empty());
}

#[test]
fn rejects_ident_with_comma() {
    assert!(scan("$<a,b,c>").is_empty());
}

// CS-0074: both `:` and `-` are now valid IDENT-continue characters.
// `$<HOME:-default>` is now tokenised as a single sigil with
// ident=`HOME:-default`. In practice, Cook authors do not use shell
// parameter-expansion syntax inside `$<...>` — probe keys may legitimately
// contain hyphens (e.g. `$<demo:cc-version.ver>`), so `-` must be admitted.
#[test]
fn ident_with_colon_and_dash_is_accepted() {
    let spans = scan("$<HOME:-default>");
    assert_eq!(spans.len(), 1);
    assert_eq!(spans[0].ident, "HOME:-default");
}

#[test]
fn probe_ref_with_hyphen_in_key_is_accepted() {
    assert_eq!(idents("$<demo:cc-version.version>"), vec!["demo:cc-version.version"]);
}

// CS-0074: probe-ref tokenization tests
#[test]
fn probe_ref_bare_key() {
    assert_eq!(idents("$<cc:zlib>"), vec!["cc:zlib"]);
}

#[test]
fn probe_ref_key_dot_field() {
    assert_eq!(idents("$<cc:zlib.cflags>"), vec!["cc:zlib.cflags"]);
}

#[test]
fn probe_ref_key_field_index() {
    assert_eq!(idents("$<cc:zlib.libs[2]>"), vec!["cc:zlib.libs[2]"]);
}

#[test]
fn probe_ref_does_not_break_bare_ident() {
    assert_eq!(idents("$<in>"), vec!["in"]);
}

#[test]
fn probe_ref_does_not_break_recipe_ident() {
    assert_eq!(idents("$<my_recipe>"), vec!["my_recipe"]);
}

#[test]
fn probe_ref_mixed_with_other_sigils() {
    let result = idents("$<cc:compiler.path> -c $<in> -o $<out>");
    assert_eq!(result, vec!["cc:compiler.path", "in", "out"]);
}

#[test]
fn rejects_unclosed_placeholder() {
    assert!(scan("$<foo").is_empty());
    assert!(scan("$<foo bar baz").is_empty());
}

#[test]
fn does_not_search_forward_for_close() {
    // A `>` appearing later in the string MUST NOT be treated as the close
    // of a malformed `$<...`. Verifies the strict-bail behavior.
    assert!(scan("$<foo bar> baz").is_empty());
}

#[test]
fn literal_dollar_alone_is_not_placeholder() {
    assert!(scan("echo $HOME").is_empty());
    assert!(scan("echo $1").is_empty());
    assert!(scan("price: $5").is_empty());
}

#[test]
fn literal_braces_are_not_placeholders() {
    // The strict rule: only $< triggers the scanner.
    assert!(scan("{a,b,c}").is_empty());
    assert!(scan("${HOME}").is_empty()); // `${` is not `$<`
    assert!(scan("awk '{print $1}'").is_empty());
}

#[test]
fn span_includes_dollar_and_close() {
    let spans = scan("hi $<foo> there");
    assert_eq!(spans.len(), 1);
    assert_eq!(spans[0].range, 3..9);
    assert_eq!(spans[0].ident, "foo");
}

// CS-0101: `file:` prefix admits path characters `/` and `*`.
#[test]
fn file_ref_literal_path() {
    assert_eq!(idents("--tokens $<file:tokens.css>"), vec!["file:tokens.css"]);
}

#[test]
fn file_ref_with_slash_and_star() {
    assert_eq!(idents("$<file:templates/*.html>"), vec!["file:templates/*.html"]);
    assert_eq!(idents("$<file:voice/narrator.wav>"), vec!["file:voice/narrator.wav"]);
}

#[test]
fn file_ref_empty_path_is_literal() {
    assert!(scan("$<file:>").is_empty());
}

#[test]
fn file_ref_with_space_is_literal() {
    assert!(scan("$<file:a b.css>").is_empty());
}

#[test]
fn file_ref_strict_bail_no_forward_search() {
    // out-of-charset byte (`[`) bails; no forward search for `>`
    assert!(scan("$<file:a[0].css> x").is_empty());
}

#[test]
fn file_ref_multibyte_path_char_strict_bails() {
    // The path charset is ASCII-only; a multibyte char (`ü`) is an
    // out-of-charset byte and MUST strict-bail (sequence stays literal),
    // never split a UTF-8 code point or forward-search for `>`.
    assert!(scan("$<file:fü.css>").is_empty());
}

#[test]
fn file_prefix_only_as_whole_token() {
    // `myfile:x` is NOT a file ref — generic charset still applies
    assert_eq!(idents("$<myfile:x.css>"), vec!["myfile:x.css"]);
}

#[test]
fn dollar_lt_followed_by_dollar_lt() {
    // $<$<x>> — outer $< followed by literal $, then identifier-shaped
    // content fails (first char of IDENT is `$`, which is not ALPHA), so
    // outer is literal. The inner $<x> is a valid placeholder at offset 2.
    let spans = scan("$<$<x>>");
    assert_eq!(spans.len(), 1);
    assert_eq!(spans[0].ident, "x");
    assert_eq!(spans[0].range, 2..6);
}

// ─── CS-0074 probe-ref grammar (COOK-357) ───────────────────────────────────
//
// One walker for `$<key:path>` idents, shared with cook-register's
// `cook.add_unit` capture. Before COOK-357 these lived twice: privately in
// `resolver::parse_probe_ref` and again as a closure in
// cook-register/src/unit_api.rs.

#[test]
fn parses_probe_ref_bare_key() {
    let r = probe_ref("cc:zlib").expect("probe-shaped");
    assert_eq!(r.key(), "cc:zlib");
    assert_eq!(r.path(), &[]);
    assert_eq!(r.lua_access(), r#"cook.probes.get("cc:zlib")"#);
}

#[test]
fn parses_probe_ref_field() {
    let r = probe_ref("cc:zlib.cflags").expect("probe-shaped");
    assert_eq!(r.key(), "cc:zlib");
    assert_eq!(r.path(), &[Seg::Field("cflags".to_string())]);
    assert_eq!(r.lua_access(), r#"cook.probes.get("cc:zlib").cflags"#);
}

#[test]
fn parses_probe_ref_field_indexed() {
    let r = probe_ref("cc:zlib.cflags[2]").expect("probe-shaped");
    assert_eq!(r.key(), "cc:zlib");
    assert_eq!(
        r.path(),
        &[Seg::Field("cflags".to_string()), Seg::Index("2".to_string())]
    );
    assert_eq!(r.lua_access(), r#"cook.probes.get("cc:zlib").cflags[2]"#);
}

#[test]
fn probe_ref_key_ends_at_first_dot_or_bracket_after_the_colon() {
    // The colon discriminator is part of the KEY; a dot BEFORE it does not
    // start the path (`demo:cc-version.ver` keys on `demo:cc-version`).
    let r = probe_ref("demo:cc-version.ver").expect("probe-shaped");
    assert_eq!(r.key(), "demo:cc-version");
    assert_eq!(r.path(), &[Seg::Field("ver".to_string())]);
}

#[test]
fn probe_ref_index_directly_on_the_key() {
    let r = probe_ref("list:items[1]").expect("probe-shaped");
    assert_eq!(r.key(), "list:items");
    assert_eq!(r.lua_access(), r#"cook.probes.get("list:items")[1]"#);
}

#[test]
fn probe_ref_rejects_idents_without_a_colon() {
    assert!(probe_ref("in.stem").is_none());
    assert!(probe_ref("out_1").is_none());
    assert!(probe_ref("HOME").is_none());
}

#[test]
fn probe_ref_rejects_the_reserved_file_namespace() {
    // CS-0101: `file:` is dispatched before the colon discriminator. Folding
    // the exclusion into the walker keeps the rule in one place instead of
    // relying on every caller to pre-filter (resolver and unit_api each did).
    assert!(probe_ref("file:src/x.css").is_none());
    assert!(probe_ref("file:dir/*.css").is_none());
}

#[test]
fn probe_ref_escapes_the_key_for_the_lua_literal() {
    // Unreachable through `scan` (the IDENT charset admits neither `"` nor
    // `\`), but `probe_ref` is public: one escape rule, not three. Before
    // COOK-357 the resolver escaped `\` and `"`, unit_api additionally
    // escaped `\n`/`\r`/`\0`, and neither agreed with the other.
    let r = probe_ref(r#"k:a"b"#).expect("probe-shaped");
    assert_eq!(r.lua_access(), r#"cook.probes.get("k:a\"b")"#);
}
