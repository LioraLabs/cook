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
fn file_ref_with_space_is_literal() {
    assert!(scan("$<file:a b.css>").is_empty());
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
fn probe_ref_key_ends_at_first_dot_or_bracket_after_the_colon() {
    // The colon discriminator is part of the KEY; a dot BEFORE it does not
    // start the path (`demo:cc-version.ver` keys on `demo:cc-version`).
    let r = probe_ref("demo:cc-version.ver", colon_keys_only).expect("probe-shaped");
    assert_eq!(r.key(), "demo:cc-version");
    assert_eq!(r.path(), &[Seg::Field("ver".to_string())]);
}

#[test]
fn probe_ref_rejects_idents_without_a_colon() {
    assert!(probe_ref("in.stem", colon_keys_only).is_none());
    assert!(probe_ref("out_1", colon_keys_only).is_none());
    assert!(probe_ref("HOME", colon_keys_only).is_none());
}

// ─── CS-0240: a probe reference resolves by name, not by colon (COOK-491) ────
//
// The colon stops being the discriminator and becomes one of two ways a base
// can be a probe key. `declares` supplies the other.

/// Declared-keyset predicate for the tests below.
fn declared(keys: &'static [&'static str]) -> impl Fn(&str) -> bool {
    move |name: &str| keys.contains(&name)
}

#[test]
fn colon_free_declared_key_is_a_probe_ref() {
    // COOK-491: `probe keyed_obs` + `$<keyed_obs>` — the ticket's repro. Before
    // CS-0240 this answered None and the sigil fell through to the declared-
    // variable step, which reported "no config block declares 'keyed_obs'".
    let r = probe_ref("keyed_obs", declared(&["keyed_obs"])).expect("declared key");
    assert_eq!(r.key(), "keyed_obs");
    assert!(r.path().is_empty());
}

#[test]
fn colon_free_declared_key_takes_a_field_path() {
    // COOK-491: the base ends at the first dot, so member access reads the same
    // way it does for a colon key.
    let r = probe_ref("keyed_obs.ver", declared(&["keyed_obs"])).expect("declared key");
    assert_eq!(r.key(), "keyed_obs");
    assert_eq!(r.path(), &[Seg::Field("ver".to_string())]);
}

#[test]
fn colon_free_hyphenated_declared_key_splits_at_the_dot() {
    // COOK-491: PROBE_SEG admits `-` and not `.`, so the hyphen stays in the
    // key and the dot starts the path.
    let r = probe_ref("cc-version.ver", declared(&["cc-version"])).expect("declared key");
    assert_eq!(r.key(), "cc-version");
    assert_eq!(r.path(), &[Seg::Field("ver".to_string())]);
}

#[test]
fn colon_free_undeclared_name_is_not_a_probe_ref() {
    // COOK-491: the closed cascade is unchanged for everything that is not a
    // probe — an undeclared bare name must still reach the variable step and
    // its hard error, not become a silent probe miss.
    assert!(probe_ref("HOME", declared(&["keyed_obs"])).is_none());
    assert!(probe_ref("in.stem", declared(&["keyed_obs"])).is_none());
    assert!(probe_ref("out_1", declared(&["keyed_obs"])).is_none());
}

#[test]
fn colon_key_needs_no_keyset() {
    // COOK-491: back-compat. A module key is self-identifying, so a caller
    // holding no keyset resolves it exactly as CS-0074 did.
    let r = probe_ref("demo:cc-version.ver", colon_keys_only).expect("colon key");
    assert_eq!(r.key(), "demo:cc-version");
    assert_eq!(r.path(), &[Seg::Field("ver".to_string())]);
}

#[test]
fn colon_keys_only_rejects_every_bare_name() {
    // COOK-491: the two lexical-only call sites (`cook.add_unit` capture) must
    // keep answering None for a bare name, whatever it is.
    assert!(probe_ref("keyed_obs", colon_keys_only).is_none());
    assert!(probe_ref("keyed_obs.ver", colon_keys_only).is_none());
}
