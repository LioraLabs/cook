use super::*;
use cook_contracts::cache::DeclaredInput;

#[test]
fn empty_dirs_under_reports_only_empty_dirs() {
    let tmp = tempfile::tempdir().unwrap();
    let wd = tmp.path();
    std::fs::create_dir_all(wd.join("out/empty")).unwrap();
    std::fs::create_dir_all(wd.join("out/full")).unwrap();
    std::fs::write(wd.join("out/full/f"), b"x").unwrap();
    let mut got = empty_dirs_under(wd, "out");
    got.sort();
    assert_eq!(got, vec!["out/empty".to_string()]);
}

#[test]
fn test_hash_str_deterministic() {
    let h1 = hash_str("hello");
    let h2 = hash_str("hello");
    assert_eq!(h1, h2);
}

#[test]
fn test_hash_str_differs() {
    let h1 = hash_str("hello");
    let h2 = hash_str("world");
    assert_ne!(h1, h2);
}

// CS-0186: the `compute_test_fingerprint` tests stood here — ten of them, over
// a hash function and an input struct that existed for the one unit kind with
// its own store. The store is gone and so is the function; a test unit is
// judged by `needs_rebuild_cook` over its `CacheMeta`, which the cook-engine
// and cook-register suites cover on the one path every unit shares. Nothing
// here was worth keeping alive by keeping its subject alive.

#[test]
fn glob_meta_literal_paths_return_false() {
    assert!(!has_glob_meta(""));
    assert!(!has_glob_meta("main.c"));
    assert!(!has_glob_meta("build/main.o"));
    assert!(!has_glob_meta("apps/web/.next/BUILD_ID"));
    assert!(!has_glob_meta("a/b/c/d.txt"));
}

#[test]
fn glob_meta_star_returns_true() {
    assert!(has_glob_meta("*"));
    assert!(has_glob_meta("*.c"));
    assert!(has_glob_meta("src/**"));
    assert!(has_glob_meta("src/**/*"));
    assert!(has_glob_meta("apps/web/.next/**"));
}

#[test]
fn glob_meta_question_returns_true() {
    assert!(has_glob_meta("?"));
    assert!(has_glob_meta("file?.txt"));
}

#[test]
fn glob_meta_bracket_returns_true() {
    assert!(has_glob_meta("[abc].txt"));
    assert!(has_glob_meta("src/[ab]/main.c"));
}

#[test]
fn glob_meta_brace_returns_false() {
    // The reference engine's `glob = "0.3"` crate does NOT support
    // brace alternation; `{` is treated as a literal. Per CS-0085
    // the spec excludes `{` from the metacharacter set so that a
    // string like "out/{a,b}.txt" is treated as a LITERAL PATH,
    // not as a glob pattern. Brace expansion may be added in a
    // future CS once the reference engine supports it.
    assert!(!has_glob_meta("{a,b}.txt"));
    assert!(!has_glob_meta("src/{lib,app}/main.c"));
}

#[test]
fn reconcile_dir_output_deletes_strays_keeps_set_prunes_empty() {
    let tmp = tempfile::tempdir().unwrap();
    let wd = tmp.path();
    std::fs::create_dir_all(wd.join("pkg/sub")).unwrap();
    std::fs::write(wd.join("pkg/a.js"), b"a").unwrap();        // kept
    std::fs::write(wd.join("pkg/STRAY.txt"), b"x").unwrap();   // delete
    std::fs::write(wd.join("pkg/sub/old.wasm"), b"o").unwrap();// delete -> sub becomes empty

    let kept: std::collections::BTreeSet<String> =
        ["pkg/a.js".to_string()].into_iter().collect();
    reconcile_dir_output(wd, "pkg", &kept);

    assert!(wd.join("pkg/a.js").exists());
    assert!(!wd.join("pkg/STRAY.txt").exists());
    assert!(!wd.join("pkg/sub/old.wasm").exists());
    assert!(!wd.join("pkg/sub").exists());   // pruned empty dir
    assert!(wd.join("pkg").exists());        // root dir preserved
}

#[test]
fn reconcile_preserves_kept_empty_dir_prunes_unkept() {
    let tmp = tempfile::tempdir().unwrap();
    let wd = tmp.path();
    std::fs::create_dir_all(wd.join("out/keep")).unwrap(); // recorded empty dir
    std::fs::create_dir_all(wd.join("out/stray")).unwrap(); // not recorded
    std::fs::write(wd.join("out/f"), b"x").unwrap();
    let mut kept = std::collections::BTreeSet::new();
    kept.insert("out/f".to_string());
    kept.insert("out/keep".to_string());
    reconcile_dir_output(wd, "out", &kept);
    assert!(wd.join("out/keep").is_dir(), "kept empty dir must survive");
    assert!(
        !wd.join("out/stray").exists(),
        "unrecorded empty dir must be pruned"
    );
    assert!(wd.join("out/f").is_file());
}

#[test]
fn reconcile_dir_output_trailing_slash_root_works_identically() {
    // A caller that passes "pkg/" (with trailing slash) must behave
    // identically to "pkg" — stray deleted, kept file preserved, empty
    // subdirectory pruned.
    let tmp = tempfile::tempdir().unwrap();
    let wd = tmp.path();
    std::fs::create_dir_all(wd.join("pkg/sub")).unwrap();
    std::fs::write(wd.join("pkg/a.js"), b"a").unwrap();        // kept
    std::fs::write(wd.join("pkg/STRAY.txt"), b"x").unwrap();   // delete
    std::fs::write(wd.join("pkg/sub/old.wasm"), b"o").unwrap();// delete -> sub becomes empty

    let kept: std::collections::BTreeSet<String> =
        ["pkg/a.js".to_string()].into_iter().collect();
    // Pass root with trailing slash — must behave the same as "pkg".
    reconcile_dir_output(wd, "pkg/", &kept);

    assert!(wd.join("pkg/a.js").exists());
    assert!(!wd.join("pkg/STRAY.txt").exists());
    assert!(!wd.join("pkg/sub/old.wasm").exists());
    assert!(!wd.join("pkg/sub").exists());   // pruned empty dir
    assert!(wd.join("pkg").exists());        // root dir preserved
}

#[test]
fn terminal_output_covers_globs_and_dir_outputs() {
    assert!(is_dir_output("pkg/"));
    assert!(!is_dir_output("pkg"));
    assert!(!is_dir_output("pkg/file.js"));

    assert!(is_terminal_output("pkg/"));        // directory output (CS-0119)
    assert!(is_terminal_output("out/*.o"));     // glob (CS-0085)
    assert!(is_terminal_output("a/**"));        // glob
    assert!(!is_terminal_output("build/app"));  // literal
}

// ===========================================================================
// resolve_declared_inputs — one resolution, every unit (§17.4 rule 1, CS-0186)
//
// The engine used to reconstruct a test unit's inputs by walking its DAG
// predecessors, in a function that could only serve units it knew to be tests.
// WHICH paths a unit declares is now decided by the lowering; this is the only
// thing left, and it does the same job for a `cook` unit and a `test` unit
// without being told which it has.
// ===========================================================================

fn tree(files: &[&str]) -> tempfile::TempDir {
    let d = tempfile::tempdir().expect("tempdir");
    for f in files {
        let p = d.path().join(f);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, b"x").unwrap();
    }
    d
}

/// A declared path: one file, used as written.
fn f(s: &str) -> DeclaredInput {
    DeclaredInput::path(s)
}

/// A declared pattern: expanded against the tree when the unit is ready.
fn g(s: &str) -> DeclaredInput {
    DeclaredInput::pattern(s)
}

fn resolve(inputs: &[DeclaredInput], consumes: &[&str], dir: &std::path::Path) -> Vec<String> {
    let consumes: Vec<String> = consumes.iter().map(|s| s.to_string()).collect();
    crate::resolve_declared_inputs(inputs, &consumes, dir)
}

/// The overwhelmingly common case, and the one that must not change: a unit
/// whose inputs are literal paths gets them back verbatim, in declaration
/// order. `check_inputs` compares recorded against current ELEMENT-WISE, so
/// reordering here would read as an input-set change on every unit in a
/// project.
#[test]
fn literal_inputs_are_returned_in_declaration_order() {
    let d = tree(&["b.c", "a.c"]);
    assert_eq!(resolve(&[f("b.c"), f("a.c")], &[], d.path()), vec!["b.c", "a.c"]);
}

/// A literal input is NOT checked against the filesystem. A declared input that
/// is missing is a rebuild reason for `check_inputs` to report, not something to
/// silently drop here — dropping it would shrink the set and hand back a hit.
#[test]
fn a_missing_literal_input_is_kept() {
    let d = tree(&[]);
    assert_eq!(resolve(&[f("gone.c")], &[], d.path()), vec!["gone.c"]);
}

/// The case that made this function necessary: a unit declaring a consumed
/// output whose producer had not run at registration. By the time the unit is
/// ready the producer has, and the entry names files.
#[test]
fn a_glob_entry_resolves_to_the_files_it_names() {
    let d = tree(&["dist/index.mjs", "dist/index.mjs.map"]);
    assert_eq!(
        resolve(&[g("dist/**")], &[], d.path()),
        vec!["dist/index.mjs", "dist/index.mjs.map"]
    );
}

/// CS-0085's normalisation, applied on the reading side exactly as the
/// producing side applies it when capturing the same declaration. Without it a
/// bare trailing `**` resolves to nothing — the glob crate treats it as
/// directories-only — and the unit is silently under-keyed against its
/// dependency, which is a stale pass rather than a wasted rebuild.
#[test]
fn a_directory_entry_resolves_to_its_subtree() {
    let d = tree(&["dist/a.js", "dist/nested/b.js"]);
    let got = resolve(&[g("dist/")], &[], d.path());
    assert!(got.contains(&"dist/a.js".to_string()), "got {got:?}");
    assert!(got.contains(&"dist/nested/b.js".to_string()), "got {got:?}");
}

#[test]
fn a_glob_matching_nothing_contributes_nothing() {
    let d = tree(&["a.c"]);
    assert!(resolve(&[g("nope/**")], &[], d.path()).is_empty());
}

/// One path reached two ways is one input. A unit's recorded set must not
/// depend on how many declarations happened to name a file.
#[test]
fn duplicates_are_dropped_keeping_first_position() {
    let d = tree(&["a.c", "b.c"]);
    assert_eq!(
        resolve(&[f("a.c"), g("*.c"), f("b.c")], &[], d.path()),
        vec!["a.c", "b.c"],
        "the glob re-names a.c and b.c; neither may appear twice"
    );
}

// --- consumes (CS-0175) ---

/// The case the surface exists for: an esbuild-family sourcemap the consumer
/// never opens, rewritten byte-for-byte by a comment-only edit upstream.
#[test]
fn consumes_narrows_the_resolved_set() {
    let d = tree(&["dist/index.mjs", "dist/index.mjs.map"]);
    assert_eq!(resolve(&[g("dist/**")], &["*.mjs"], d.path()), vec!["dist/index.mjs"]);
}

/// Narrowing errs toward the UNDER-keyed direction, where a stale hit replays
/// against inputs that have moved. A filter matching nothing is therefore
/// inert, never a set of nothing.
#[test]
fn a_consumes_matching_nothing_is_inert() {
    let d = tree(&["dist/index.mjs", "dist/index.mjs.map"]);
    assert_eq!(
        resolve(&[g("dist/**")], &["*.wasm"], d.path()),
        vec!["dist/index.mjs", "dist/index.mjs.map"]
    );
}

#[test]
fn an_empty_consumes_narrows_nothing() {
    let d = tree(&["dist/a.mjs", "dist/b.map"]);
    assert_eq!(resolve(&[g("dist/**")], &[], d.path()).len(), 2);
}

/// A pattern that cannot compile keeps the full set. Patterns are rejected at
/// register phase so this is unreachable in a real build, and the fallback is
/// the safe direction rather than a silent narrowing.
#[test]
fn an_uncompilable_consumes_keeps_the_full_set() {
    let d = tree(&["dist/a.mjs"]);
    assert_eq!(resolve(&[g("dist/**")], &["["], d.path()), vec!["dist/a.mjs"]);
}

// --- B1: a path is a path, whatever is in its name (§17.1.1.2) ------------

/// The defect this classification exists to prevent, at the layer that would
/// commit it. `pages/[id].tsx` is a FILE — the framework routing convention —
/// and the register phase handed it here as one. A resolver that re-read the
/// string would treat `[id]` as a character class, match nothing, and drop the
/// entry from the unit's key: silently, on this side and on the recording side
/// alike, so the two agree and the unit hits over a file that changed.
#[test]
fn a_path_containing_glob_metacharacters_is_not_expanded() {
    let d = tree(&["pages/[id].tsx", "pages/plain.tsx"]);
    assert_eq!(
        resolve(&[f("pages/[id].tsx"), f("pages/plain.tsx")], &[], d.path()),
        vec!["pages/[id].tsx", "pages/plain.tsx"]
    );
}

/// The same name declared as a pattern IS expanded — the kind decides, and
/// nothing else does. (`[id]` as a class matches no single-character file here,
/// so the expansion is empty, which is exactly what made the defect silent.)
#[test]
fn the_same_name_declared_as_a_pattern_is_expanded() {
    let d = tree(&["pages/[id].tsx"]);
    assert!(resolve(&[g("pages/[id].tsx")], &[], d.path()).is_empty());
}

// --- B4: consumes narrows expansions, never declared paths ---------------

/// `consumes` says "of what this pattern covers, I read only these". It says
/// nothing about a file the unit named outright, and MUST NOT be able to delete
/// one: dropping `src/own.txt` here would leave the unit unkeyed on a file the
/// author declared, so editing it would move nothing.
#[test]
fn consumes_never_removes_a_declared_path() {
    let d = tree(&["src/own.txt", "dist/index.mjs", "dist/index.mjs.map"]);
    assert_eq!(
        resolve(&[f("src/own.txt"), g("dist/**")], &["*.mjs"], d.path()),
        vec!["src/own.txt", "dist/index.mjs"]
    );
}

/// A unit declaring only paths is unaffected by any filter, including one that
/// matches none of them — there is no expansion to narrow.
#[test]
fn consumes_over_paths_alone_is_inert() {
    let d = tree(&["src/own.txt", "src/other.txt"]);
    assert_eq!(
        resolve(&[f("src/own.txt"), f("src/other.txt")], &["*.mjs"], d.path()),
        vec!["src/own.txt", "src/other.txt"]
    );
}

/// The cook_pnpm shape, and the half of CS-0175's withdrawn safeguard that was
/// doing real work: an excluded artifact must not re-enter through a SECOND
/// pattern the unit declares in its own right. A check unit declaring the
/// dependency's whole subtree and consuming `*.mjs` keys on the bundle alone.
#[test]
fn an_excluded_artifact_does_not_re_enter_through_another_pattern() {
    let d = tree(&["src/own.txt", "dist/index.mjs", "dist/index.mjs.map"]);
    assert_eq!(
        resolve(
            &[f("src/own.txt"), g("dist/**"), g("dist/**/*")],
            &["*.mjs"],
            d.path()
        ),
        vec!["src/own.txt", "dist/index.mjs"]
    );
}

/// Narrowing is over the pattern-derived candidates only, so a filter that
/// matches none of THEM is inert even though the unit's declared paths would
/// not have matched either. Without the split, a unit whose only pattern
/// expanded to unmatched files would fall back to the full set and quietly
/// re-admit what the author excluded.
#[test]
fn the_inert_fallback_is_judged_over_pattern_derived_candidates() {
    let d = tree(&["src/own.txt", "dist/index.mjs.map"]);
    assert_eq!(
        resolve(&[f("src/own.txt"), g("dist/**")], &["*.wasm"], d.path()),
        vec!["src/own.txt", "dist/index.mjs.map"],
        "no candidate matched, so the unnarrowed expansion stands"
    );
}
