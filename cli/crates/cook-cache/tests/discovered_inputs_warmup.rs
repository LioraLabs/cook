//! Three-run warmup scenario at the cache layer (no engine).
//! Asserts: Run 1 = miss, Run 2 = hit (after augmentation), Run 3 = hit
//! after a header content edit triggers InputChanged.

use cook_cache::{parse_make_depfile, store::{FileRecord, StepEntry}};
use cook_contracts::DiscoveredInputs;
use cook_fingerprint::{install_depfile_parser, needs_rebuild_cook, RebuildReason, RebuildResult};
use std::sync::Once;

fn install_parser_once() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        install_depfile_parser(|p, src, wd, fmt| {
            if fmt != "make" { return Err(()); }
            parse_make_depfile(p, src, wd).map_err(|_| ())
        });
    });
}

fn fr(wd: &std::path::Path, rel: &str) -> FileRecord {
    FileRecord {
        path: rel.into(),
        mtime: cook_fingerprint::stat_mtime(&wd.join(rel)).unwrap_or(0),
        hash: cook_fingerprint::hash_file(&wd.join(rel)).unwrap(),
    }
}

#[test]
fn warmup_collapses_to_two_runs() {
    install_parser_once();

    let dir = tempfile::tempdir().expect("tempdir");
    let wd = dir.path();
    std::fs::write(wd.join("a.c"), b"int main(){return 0;}").expect("a.c");
    std::fs::write(wd.join("a.h"), b"#pragma once\n").expect("a.h");
    std::fs::write(wd.join("a.o"), b"obj-bytes").expect("a.o");
    std::fs::create_dir_all(wd.join(".cook/deps")).expect("mkdir");
    std::fs::write(wd.join(".cook/deps/a.d"), b"a.o: a.c a.h\n").expect("d");

    let di = DiscoveredInputs {
        from: ".cook/deps/a.d".into(),
        format: "make".into(),
    };

    // ---- Run 1: NoCacheEntry, simulated execute, store fat StepEntry ----
    let (r1, _) = needs_rebuild_cook(
        None,
        &["a.c"],
        &["a.o"],
        0xc0de,
        0,
        0,
        wd,
        None,
        Some(&di),
        false,
    );
    assert!(matches!(r1, RebuildResult::Rebuild(RebuildReason::NoCacheEntry)),
        "fresh check returns NoCacheEntry");

    // Engine post-execution augmentation: build a fat StepEntry.
    // Use mtime=0 for inputs so the mtime fast-path always fires the
    // content check, regardless of filesystem mtime resolution. The hash
    // is the real content hash so Run 2 (unchanged) hits correctly.
    let stored_entry = StepEntry {
        inputs: vec![
            FileRecord {
                path: "a.c".into(),
                mtime: 0,
                hash: cook_fingerprint::hash_file(&wd.join("a.c")).unwrap(),
            },
            FileRecord {
                path: "a.h".into(),
                mtime: 0,
                hash: cook_fingerprint::hash_file(&wd.join("a.h")).unwrap(),
            },
        ],
        outputs: vec![fr(wd, "a.o"), fr(wd, ".cook/deps/a.d")],
        command_hash: 0xc0de,
        env_contribution: 0,
        seal_contribution: 0,
    };

    // ---- Run 2: pre-check augments current_inputs, equality check skips ----
    let (r2, _) = needs_rebuild_cook(
        Some(&stored_entry),
        &["a.c"],
        &["a.o"],
        0xc0de,
        0,
        0,
        wd,
        None,
        Some(&di),
        false,
    );
    assert!(matches!(r2, RebuildResult::Skip),
        "Run 2 should hit (augmented current matches fat entry); got {r2:?}");

    // ---- Run 3: edit header content; expect InputChanged ----
    std::fs::write(wd.join("a.h"), b"#pragma once\n#define X 1\n").expect("a.h v2");

    let (r3, _) = needs_rebuild_cook(
        Some(&stored_entry),
        &["a.c"],
        &["a.o"],
        0xc0de,
        0,
        0,
        wd,
        None,
        Some(&di),
        false,
    );
    assert!(matches!(&r3, RebuildResult::Rebuild(RebuildReason::InputsChanged { changed, .. })
            if changed.contains(&"a.h".to_string())),
        "Run 3 should rebuild because a.h content changed; got {r3:?}");
}
