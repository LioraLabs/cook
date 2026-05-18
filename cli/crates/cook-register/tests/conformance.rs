//! Register-phase conformance harness.
//!
//! Walks `standard/conformance/negative/` and processes every fixture that
//! carries a `register_error.txt` (instead of, or in addition to,
//! `error.txt` / `codegen_error.txt`). The fixture MUST parse cleanly,
//! codegen cleanly, and then be rejected by `register_cookfile`,
//! with a diagnostic containing the expected substring.
//!
//! Fixtures shaped this way exist because the rejection lives at register
//! time, not parse or codegen time. The first such fixture is
//! `052-directory-input-rejected/`, asserting the §6.2 path-shape rule
//! that `cook.add_unit` must reject directory inputs.
//!
//! Each fixture directory may carry adjacent files (e.g. `upstream/lib/`)
//! that the register phase needs to inspect. The harness runs the
//! register phase with the fixture directory itself as the working
//! directory, so those files are visible to `cook.add_unit`'s
//! filesystem checks.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use cook_lang::parse;
use cook_luagen::generate;
use cook_register::engine::{register_cookfile, RegisterSessionBuilder};

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
fn register_positive_conformance_corpus() {
    let mut failures: Vec<String> = Vec::new();
    let mut cases_seen = 0usize;

    for case in case_dirs("positive") {
        let marker = case.join("register_ok.txt");
        if !marker.exists() {
            continue;
        }

        let name = case.file_name().unwrap().to_string_lossy().into_owned();
        cases_seen += 1;
        let input_path = case.join("Cookfile");
        let input = fs::read_to_string(&input_path)
            .unwrap_or_else(|e| panic!("read {}: {}", input_path.display(), e));

        // Step 1: parse.
        let cookfile = match parse(&input) {
            Ok(c) => c,
            Err(e) => {
                failures.push(format!("case {}: parse failed: {}", name, e));
                continue;
            }
        };

        // Step 2: codegen.
        let lua_source = generate(&cookfile);

        // Step 3: register. Use the fixture directory as the working directory
        // so any sibling files are visible to the register phase.
        let registry = RegisterSessionBuilder::new(case.clone(), HashMap::new()).with_selected_config(None);

        // register_cookfile registers ALL recipes in the file and invokes
        // every body; positive fixtures must succeed end-to-end without
        // singling out a specific name.
        match register_cookfile(registry, &lua_source, None) {
            Ok(_) => {}
            Err(e) => failures.push(format!("case {}: register failed: {}", name, e)),
        }
    }

    assert!(cases_seen > 0, "no register_ok fixtures found — check that register_ok.txt markers exist");
    assert!(
        failures.is_empty(),
        "register-phase positive conformance failures:\n\n{}",
        failures.join("\n"),
    );
}

#[test]
fn register_negative_conformance_corpus() {
    let mut failures: Vec<String> = Vec::new();
    let mut cases_seen = 0usize;

    for case in case_dirs("negative") {
        let expected_path = case.join("register_error.txt");
        if !expected_path.exists() {
            continue;
        }

        let name = case.file_name().unwrap().to_string_lossy().into_owned();
        cases_seen += 1;
        let input_path = case.join("Cookfile");

        let input = fs::read_to_string(&input_path)
            .unwrap_or_else(|e| panic!("read {}: {}", input_path.display(), e));
        let expected_substring = fs::read_to_string(&expected_path)
            .unwrap_or_else(|e| panic!("read {}: {}", expected_path.display(), e))
            .trim()
            .to_string();

        // Step 1: the Cookfile MUST parse cleanly.
        let cookfile = match parse(&input) {
            Ok(c) => c,
            Err(e) => {
                failures.push(format!(
                    "case {}: expected parse success (register fixture), got parse error: {}\n",
                    name, e
                ));
                continue;
            }
        };

        // Step 2: codegen MUST succeed cleanly. If it doesn't, this fixture
        // belongs in the codegen harness, not here.
        let lua_source = generate(&cookfile);

        // Step 3: drive the register phase with the fixture directory as
        // working_dir. `cook.add_unit`'s directory check will then see
        // any sibling files the fixture set up.
        let registry =
            RegisterSessionBuilder::new(case.clone(), HashMap::new()).with_selected_config(None);
        if cookfile.recipes.first().is_none() {
            failures.push(format!(
                "case {}: register fixture must declare at least one recipe\n",
                name
            ));
            continue;
        }

        match register_cookfile(registry, &lua_source, None) {
            Ok(_) => {
                failures.push(format!(
                    "case {}: expected register-phase error containing {:?}, got Ok\n",
                    name, expected_substring,
                ));
            }
            Err(e) => {
                let msg = format!("{}", e);
                if !msg.contains(&expected_substring) {
                    failures.push(format!(
                        "case {}: register-phase error did not contain expected substring\n  expected substring: {:?}\n  actual message:     {:?}\n",
                        name, expected_substring, msg,
                    ));
                }
            }
        }
    }

    assert!(
        failures.is_empty(),
        "register-phase negative conformance failures ({} cases scanned):\n\n{}",
        cases_seen,
        failures.join("\n")
    );
}
