//! Codegen-phase conformance harness.
//!
//! Walks `standard/conformance/negative/` and processes every fixture that
//! carries a `codegen_error.txt` (instead of, or in addition to, `error.txt`).
//! The fixture MUST parse cleanly but MUST be rejected by
//! `cook_luagen::generate_with_names_checked`, with a diagnostic containing
//! the expected substring.
//!
//! Fixtures shaped this way exist because the rejection lives at codegen
//! time, not parse time. See `standard/conformance/negative/006-.../notes.md`
//! for the first such fixture.
//!
//! Also walks `standard/conformance/positive/` and asserts that every fixture
//! parses cleanly AND passes `generate_with_names_checked` without error. This
//! catches semantic regressions that the parser-only harness misses (e.g. `{in}`
//! in many-to-one mode, which parses fine but fails at codegen time).

use std::fs;
use std::path::PathBuf;

fn corpus_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../standard/conformance")
        .canonicalize()
        .expect("conformance corpus root missing")
}

fn case_dirs(sub: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let dir = corpus_root().join(sub);
    for entry in fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("read {}: {}", dir.display(), e))
    {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.is_dir() {
            out.push(path);
        }
    }
    out.sort();
    out
}

#[test]
fn codegen_negative_conformance_corpus() {
    let mut failures: Vec<String> = Vec::new();
    let mut cases_seen = 0usize;

    for case in case_dirs("negative") {
        let expected_path = case.join("codegen_error.txt");
        if !expected_path.exists() {
            continue;
        }
        cases_seen += 1;

        let name = case.file_name().unwrap().to_string_lossy().into_owned();
        let input_path = case.join("Cookfile");

        let input = fs::read_to_string(&input_path)
            .unwrap_or_else(|e| panic!("read {}: {}", input_path.display(), e));
        let expected_substring = fs::read_to_string(&expected_path)
            .unwrap_or_else(|e| panic!("read {}: {}", expected_path.display(), e))
            .trim()
            .to_string();

        // Step 1: the Cookfile MUST parse cleanly. If it fails to parse, this
        // fixture belongs in the parser-only harness, not this one.
        let cookfile = match cook_lang::parse(&input) {
            Ok(c) => c,
            Err(e) => {
                failures.push(format!(
                    "case {}: expected parse success (codegen fixture), got parse error: {}\n",
                    name, e
                ));
                continue;
            }
        };

        let recipe_names = cook_luagen::dep_ref::extract_recipe_names(&cookfile);
        match cook_luagen::generate_with_names_checked(&cookfile, &recipe_names) {
            Ok(_) => {
                failures.push(format!(
                    "case {}: expected codegen error containing {:?}, got Ok\n",
                    name, expected_substring,
                ));
            }
            Err(e) => {
                let msg = format!("{}", e);
                if !msg.contains(&expected_substring) {
                    failures.push(format!(
                        "case {}: codegen error did not contain expected substring\n  expected substring: {:?}\n  actual message:     {:?}\n",
                        name, expected_substring, msg,
                    ));
                }
            }
        }
    }

    assert!(
        failures.is_empty(),
        "codegen-phase negative conformance failures ({} cases scanned):\n\n{}",
        cases_seen,
        failures.join("\n")
    );
}

/// Sweep every positive fixture through `generate_with_names_checked` and
/// assert `Ok`. This catches semantic regressions that the parser-only harness
/// misses — for example, `{in}` appearing in a many-to-one (literal-output)
/// step parses cleanly but is rejected at codegen time.
#[test]
fn codegen_positive_conformance_corpus() {
    let mut failures: Vec<String> = Vec::new();
    let mut cases_seen = 0usize;

    for case in case_dirs("positive") {
        let input_path = case.join("Cookfile");
        if !input_path.exists() {
            continue;
        }
        cases_seen += 1;

        let name = case.file_name().unwrap().to_string_lossy().into_owned();
        let input = fs::read_to_string(&input_path)
            .unwrap_or_else(|e| panic!("read {}: {}", input_path.display(), e));

        let cookfile = match cook_lang::parse(&input) {
            Ok(c) => c,
            Err(e) => {
                failures.push(format!(
                    "fixture {}: parse failed (positive fixture must parse cleanly): {}\n",
                    name, e
                ));
                continue;
            }
        };

        let recipe_names = cook_luagen::dep_ref::extract_recipe_names(&cookfile);
        if let Err(e) = cook_luagen::generate_with_names_checked(&cookfile, &recipe_names) {
            failures.push(format!(
                "fixture {}: codegen rejected a positive fixture — this is a semantic regression:\n  {}\n",
                name, e
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "codegen-phase positive conformance failures ({} fixtures scanned):\n\n{}",
        cases_seen,
        failures.join("\n")
    );
}
