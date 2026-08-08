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

// The purity budget used to live here, as
// `production_source_has_no_stateful_standard_library_access`. It moved to
// `tests/constitution.rs`, whole, when that file gave the crate's rules one
// home — and it was widened on the way: it now expands `use` trees instead of
// matching their spelling, so `use std::time::{Duration, Instant}` is read as
// two distinct reaches. Leaving a copy here would have been the exact thing
// the constitution refuses.

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
