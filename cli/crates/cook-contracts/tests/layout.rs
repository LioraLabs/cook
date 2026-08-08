use std::fs;
use std::path::Path;

#[test]
fn implementation_files_live_in_directory_modules() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let flat_modules: Vec<_> = fs::read_dir(&src)
        .expect("read src directory")
        .map(|entry| entry.expect("read src entry").path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "rs"))
        .filter(|path| path.file_name().is_some_and(|name| name != "lib.rs"))
        .collect();

    assert!(
        flat_modules.is_empty(),
        "Rust implementation files must live in directory modules: {flat_modules:?}"
    );
}

#[test]
fn crate_root_is_only_an_index() {
    let lib = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/lib.rs"))
        .expect("read src/lib.rs");
    let definitions: Vec<_> = lib
        .lines()
        .map(str::trim_start)
        .filter(|line| {
            [
                "pub const ",
                "pub static ",
                "pub struct ",
                "pub enum ",
                "pub fn ",
                "impl ",
            ]
            .iter()
            .any(|prefix| line.starts_with(prefix))
        })
        .collect();

    assert!(
        definitions.is_empty(),
        "src/lib.rs must contain declarations and re-exports, not definitions: {definitions:?}"
    );
}

#[test]
fn production_source_has_no_stateful_standard_library_access() {
    fn grouped_std_use_contains(source: &str, forbidden: &str) -> bool {
        let compact: String = source.chars().filter(|ch| !ch.is_whitespace()).collect();
        compact.split("usestd::{").skip(1).any(|tail| {
            let members = tail.split_once('}').map_or(tail, |(members, _)| members);
            members.split(',').any(|member| {
                let member = member.split_once("as").map_or(member, |(path, _)| path);
                member == forbidden || member.starts_with(&format!("{forbidden}::"))
            })
        })
    }

    fn visit(dir: &Path, violations: &mut Vec<String>) {
        for entry in fs::read_dir(dir).expect("read production source directory") {
            let path = entry.expect("read production source entry").path();
            if path.is_dir() {
                if path.file_name().is_none_or(|name| name != "tests") {
                    visit(&path, violations);
                }
                continue;
            }
            if path.extension().is_none_or(|extension| extension != "rs") {
                continue;
            }
            let source = fs::read_to_string(&path).expect("read production source file");
            for (index, line) in source.lines().enumerate() {
                let code = line.split("//").next().unwrap_or_default();
                let compact: String = code.chars().filter(|ch| !ch.is_whitespace()).collect();
                for forbidden in ["std::fs", "std::env", "std::process"] {
                    if compact.contains(forbidden) {
                        violations.push(format!(
                            "{}:{} uses {forbidden}",
                            path.display(),
                            index + 1
                        ));
                    }
                }
                for forbidden in [
                    ".canonicalize()",
                    ".exists()",
                    ".is_dir()",
                    ".is_file()",
                    ".metadata()",
                    ".read_dir()",
                    ".symlink_metadata()",
                    ".try_exists()",
                ] {
                    if compact.contains(forbidden) {
                        violations.push(format!(
                            "{}:{} reaches the filesystem via `{forbidden}` — this crate's \
                             admission bar is purity; canonicalise or stat in the shell \
                             and pass the result in",
                            path.display(),
                            index + 1
                        ));
                    }
                }
            }
            for forbidden in ["fs", "env", "process"] {
                if grouped_std_use_contains(&source, forbidden) {
                    violations.push(format!("{} uses grouped std::{forbidden}", path.display()));
                }
            }
        }
    }

    let mut violations = Vec::new();
    visit(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut violations,
    );
    assert!(
        violations.is_empty(),
        "cook-contracts production source must remain state-free:\n{}",
        violations.join("\n")
    );
}

#[test]
fn command_failure_contracts_use_concept_directories_and_nested_tests() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    for concept in ["captured_stream", "command_failure", "lua_error"] {
        assert!(src.join(concept).join("mod.rs").is_file());
        assert!(src
            .join(concept)
            .join("tests")
            .read_dir()
            .expect("read nested tests")
            .any(|entry| entry
                .expect("read test entry")
                .path()
                .extension()
                .is_some_and(|extension| extension == "rs")));
    }
}
