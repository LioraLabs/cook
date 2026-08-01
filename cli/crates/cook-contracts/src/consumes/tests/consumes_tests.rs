use super::*;

fn filter(pats: &[&str]) -> ConsumesFilter {
    let owned: Vec<String> = pats.iter().map(|s| s.to_string()).collect();
    ConsumesFilter::compile(&owned).expect("patterns compile")
}

#[test]
fn slashless_pattern_matches_basename_at_any_depth() {
    let f = filter(&["*.d.ts"]);
    assert!(f.matches("packages/core/dist/index.d.ts"));
    assert!(f.matches("index.d.ts"));
    assert!(!f.matches("packages/core/dist/index.mjs"));
}

#[test]
fn sourcemap_sidecars_are_excluded_by_an_extension_allowlist() {
    // The case the whole surface exists for: `.mjs` folds, `.mjs.map`
    // does not, so a comment-only upstream edit leaves the key alone.
    let f = filter(&["*.mjs", "*.d.ts"]);
    assert!(f.matches("pkgs/lib/dist/index.mjs"));
    assert!(!f.matches("pkgs/lib/dist/index.mjs.map"));
}

#[test]
fn pattern_with_slash_matches_the_root_relative_path() {
    let f = filter(&["packages/core/dist/**/*.mjs"]);
    assert!(f.matches("packages/core/dist/esm/a.mjs"));
    assert!(f.matches("packages/core/dist/a.mjs"));
    assert!(!f.matches("packages/other/dist/a.mjs"));
}

#[test]
fn star_does_not_cross_a_path_separator() {
    let f = filter(&["dist/*.mjs"]);
    assert!(f.matches("dist/a.mjs"));
    assert!(!f.matches("dist/esm/a.mjs"));
}

#[test]
fn empty_filter_selects_everything() {
    let f = filter(&[]);
    assert!(f.is_empty());
    let items = vec!["a.mjs".to_string(), "a.mjs.map".to_string()];
    assert_eq!(f.select(&items, |s| s.clone()).len(), 2);
}

#[test]
fn a_filter_matching_nothing_keeps_the_unnarrowed_set() {
    // Fail-safe toward over-invalidation. A typo'd pattern must not
    // silently drop the dependency-content determinant and let a stale
    // pass replay.
    let f = filter(&["*.wasm"]);
    let items = vec!["a.mjs".to_string(), "a.mjs.map".to_string()];
    assert_eq!(f.select(&items, |s| s.clone()).len(), 2);
}

#[test]
fn a_partial_match_narrows_normally() {
    let f = filter(&["*.mjs"]);
    let items = vec!["a.mjs".to_string(), "a.mjs.map".to_string()];
    let kept = f.select(&items, |s| s.clone());
    assert_eq!(kept, vec![&"a.mjs".to_string()]);
}

#[test]
fn empty_candidates_stay_empty() {
    // A dep that produced nothing yet is not a mismatch — there is
    // nothing to fail safe about.
    let f = filter(&["*.mjs"]);
    let items: Vec<String> = vec![];
    assert!(f.select(&items, |s: &String| s.clone()).is_empty());
}

#[test]
fn an_unparseable_pattern_is_rejected() {
    let bad = vec!["[".to_string()];
    assert!(ConsumesFilter::compile(&bad).is_err());
    assert!(validate_pattern("[").is_err());
    assert!(validate_pattern("*.d.ts").is_ok());
}
