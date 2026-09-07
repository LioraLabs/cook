use super::{internal_module_tag, is_internal_recipe, qualified_name, qualify_recipe_reference};

#[test]
fn plain_recipe_names_are_not_internal() {
    assert!(!is_internal_recipe("build"));
    assert!(!is_internal_recipe("apps.web.build"));
    assert!(!is_internal_recipe(""));
    assert_eq!(internal_module_tag("build"), None);
}

#[test]
fn bare_internal_names_are_recognised() {
    assert!(is_internal_recipe("__cc_config_header__build_dhewm3_config_h"));
    assert_eq!(
        internal_module_tag("__cc_config_header__build_dhewm3_config_h"),
        Some("cc")
    );
}

/// COOK-411: the disagreement. cook-cli tested the last segment and hid this
/// from completion; cook-progress tested the whole string and rendered it raw.
#[test]
fn a_qualified_internal_name_is_internal_too() {
    assert!(is_internal_recipe("game.__cc_config_header__x"));
    assert_eq!(internal_module_tag("game.__cc_config_header__x"), Some("cc"));

    assert!(is_internal_recipe("apps.web.__pnpm_install"));
    assert_eq!(internal_module_tag("apps.web.__pnpm_install"), Some("pnpm"));
}

#[test]
fn a_dunder_with_nothing_after_it_has_no_tag() {
    assert!(is_internal_recipe("__"));
    assert_eq!(internal_module_tag("__"), None);
    assert!(is_internal_recipe("ns.__"));
    assert_eq!(internal_module_tag("ns.__"), None);
}

/// A single leading underscore is a user's choice of name, not the tooling
/// convention.
#[test]
fn one_underscore_is_not_the_convention() {
    assert!(!is_internal_recipe("_private"));
    assert_eq!(internal_module_tag("_private"), None);
}

// ---------------------------------------------------------------------------
// The bare-name character class (COOK-421)
// ---------------------------------------------------------------------------

use super::{is_bare_name, is_bare_name_char, is_bare_name_start};

/// The class stated once as data, against App. A's
/// `BARE_IDENTIFIER ::= /[A-Za-z_][A-Za-z0-9_.\-]*/`. Written out rather than
/// derived from the predicate, so this test disagrees with the code when the
/// code changes.
#[test]
fn the_bare_name_class_is_the_grammars_class() {
    for c in "abzABZ_".chars() {
        assert!(is_bare_name_start(c), "{c:?} may start a bare name");
        assert!(is_bare_name_char(c), "{c:?} may continue one");
    }
    for c in "09-.".chars() {
        assert!(!is_bare_name_start(c), "{c:?} may NOT start a bare name");
        assert!(is_bare_name_char(c), "{c:?} may continue one");
    }
    for c in " \t/:*?\"'\\@$(){}[]#!,+=~%^&|<>;".chars() {
        assert!(!is_bare_name_start(c), "{c:?} is outside the class");
        assert!(!is_bare_name_char(c), "{c:?} is outside the class");
    }
    // Non-ASCII is outside it too: the production is spelled in ASCII ranges.
    for c in "éü漢".chars() {
        assert!(!is_bare_name_start(c));
        assert!(!is_bare_name_char(c));
    }
}

#[test]
fn a_whole_bare_name_needs_a_start_character_and_is_never_empty() {
    for good in ["a", "_x", "build", "rust.build", "cc-version", "node.js"] {
        assert!(is_bare_name(good), "{good:?} is a bare name");
    }
    for bad in ["", "1build", "-x", ".x", "a b", "a/b", "cc:zlib"] {
        assert!(!is_bare_name(bad), "{bad:?} is not a bare name");
    }
}

// ---------------------------------------------------------------------------
// Import-qualified names (§11, COOK-526)
// ---------------------------------------------------------------------------

use super::import_prefix;

/// `import_prefix` reads the same off a qualified RECIPE name and a qualified
/// PROBE key, because both are stamped by the same §11 composition and split
/// on the same final `.`.
#[test]
fn prefix_reads_the_same_off_a_recipe_name_and_a_probe_key() {
    assert_eq!(import_prefix("backend.proto.generate"), "backend.proto");
    assert_eq!(import_prefix("backend.cc:version"), "backend");
    assert_eq!(import_prefix("build"), "");
    assert_eq!(import_prefix("cc:version"), "");
}

/// Materializer identity (COOK-553): prefix and local key join at an
/// unambiguous boundary, so distinct (prefix, key) pairs never collide.
#[test]
fn prefix_and_local_key_have_an_unambiguous_boundary() {
    let left = qualified_name("a", "bc");
    let right = qualified_name("ab", "c");
    assert_eq!(left, "a.bc");
    assert_eq!(right, "ab.c");
    assert_ne!(left, right);
}

#[test]
fn recipe_reference_qualification_preserves_local_alias_and_canonical_names() {
    let aliases = [("alias".into(), "canonical".into())].into();
    let local = ["target".into()].into();
    assert_eq!(
        qualify_recipe_reference("target", "member", &aliases, &local),
        "member.target"
    );
    assert_eq!(
        qualify_recipe_reference("alias.target", "member", &aliases, &local),
        "canonical.target"
    );
    assert_eq!(
        qualify_recipe_reference("elsewhere.target", "member", &aliases, &local),
        "elsewhere.target"
    );
}
