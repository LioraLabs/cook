//! CS-0204 end-to-end: a module is source, and the work that loads it is keyed
//! on it. Against the real `cook` binary, in tmpdirs, because the defect this
//! pins was invisible to every layer below the binary — each crate was doing
//! exactly what it had been asked to do, and no one had asked for the module.
//!
//! Four bars, in the order they matter:
//!
//! 1. A `cook` body that loads a module produces the NEW value after the module
//!    is edited. This is the ticket's repro verbatim.
//! 2. A run with nothing edited is fully cached. Without this the first bar is
//!    equally satisfied by a unit that rebuilds forever, which is not a fix.
//! 3. A probe's `produce` body gets the same treatment, through a fingerprint
//!    that folds no module source until CS-0204 (§{cat.probes.module-source}).
//! 4. Two "machines" sharing one content-addressed store do not serve each
//!    other a result produced under different module content. This is the bar
//!    that makes the bug worse than staleness: the key is portable by design,
//!    so an unkeyed determinant travels.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

fn cook_binary() -> std::path::PathBuf {
    let mut path = std::env::current_exe().unwrap();
    path.pop();
    path.pop();
    path.push("cook");
    assert!(
        path.exists(),
        "cook binary not found at {} — run `cargo build --bin cook` first",
        path.display()
    );
    path
}

const HELPER: &str = "local m = {}\nfunction m.value() return \"%V%\" end\nreturn m\n";

/// Root the shared store inside the test's own tempdir.
///
/// Not optional hygiene: without it these units publish into the developer's
/// real store, and the SECOND run of the suite is served an entry the first run
/// left there — restoring the declared output while the runlog, which is not a
/// declared output, stays empty. The test then fails for a reason that has
/// nothing to do with what it is pinning. (Found by the gate; the first local
/// run passed precisely because the store was cold.)
fn isolate_store(wd: &Path, store: &Path) {
    fs::create_dir_all(wd.join(".cook")).unwrap();
    fs::write(
        wd.join(".cook/cloud.toml"),
        format!("[cache]\ncache_dir = {:?}\n", store.to_string_lossy()),
    )
    .unwrap();
}

fn write_helper(wd: &Path, value: &str) {
    fs::create_dir_all(wd.join("cook_modules")).unwrap();
    fs::write(
        wd.join("cook_modules/helper.lua"),
        HELPER.replace("%V%", value),
    )
    .unwrap();
}

fn run(wd: &Path, recipe: &str) -> Output {
    Command::new(cook_binary())
        .arg(recipe)
        .current_dir(wd)
        .output()
        .expect("cook invocation")
}

fn build(wd: &Path, recipe: &str) {
    let out = run(wd, recipe);
    assert!(
        out.status.success(),
        "cook {recipe} failed:\n{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
}

/// How many times a body has run, counted by what it appended rather than by
/// what the renderer printed: the progress output is a presentation surface
/// and says nothing to a captured, non-tty child.
fn runs(wd: &Path, runlog: &str) -> usize {
    fs::read_to_string(wd.join(runlog))
        .map(|s| s.lines().count())
        .unwrap_or(0)
}

/// A `cook` body calling `cook.load_module` must see the edited module, and a
/// run with no edit must cost nothing.
#[test]
fn editing_a_module_rebuilds_the_body_that_loaded_it() {
    let store = tempfile::tempdir().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let wd = tmp.path();
    isolate_store(wd, store.path());
    write_helper(wd, "ORIGINAL");
    fs::write(
        wd.join("Cookfile"),
        "recipe emit\n\
         \x20   cook \"out.txt\" >{\n\
         \x20       local h = cook.load_module(\"helper\")\n\
         \x20       cook.sh(\"echo ran >> runlog\")\n\
         \x20       fs.write(\"out.txt\", h.value())\n\
         \x20   }\n",
    )
    .unwrap();

    build(wd, "emit");
    assert_eq!(fs::read_to_string(wd.join("out.txt")).unwrap(), "ORIGINAL");
    assert_eq!(runs(wd, "runlog"), 1);

    // Bar 2 first, so a unit that simply never caches cannot pass bar 1.
    build(wd, "emit");
    assert_eq!(
        runs(wd, "runlog"),
        1,
        "a run with nothing edited must be a hit, not a rebuild"
    );

    write_helper(wd, "REVISED");
    build(wd, "emit");
    assert_eq!(runs(wd, "runlog"), 2, "the module moved, so the body must run");
    assert_eq!(
        fs::read_to_string(wd.join("out.txt")).unwrap(),
        "REVISED",
        "the module changed what the body does, so the body must run again"
    );
}

/// The same rule through the probe fingerprint. The probe declares an
/// ingredient so it is keyed at all (CS-0178 keylessness would otherwise make
/// it re-produce every run and prove nothing), and the consumer seals on it so
/// a changed value reaches the consumer's key.
#[test]
fn editing_a_module_reproduces_the_probe_that_loaded_it() {
    let store = tempfile::tempdir().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let wd = tmp.path();
    isolate_store(wd, store.path());
    write_helper(wd, "ORIGINAL");
    fs::write(wd.join("seed.txt"), "seed\n").unwrap();
    fs::write(
        wd.join("Cookfile"),
        "probe mod:answer\n\
         \x20   ingredients \"seed.txt\"\n\
         \x20   >{ cook.sh(\"echo ran >> probelog\"); local h = cook.load_module(\"helper\"); return h.value() }\n\
         \n\
         recipe emit\n\
         \x20   seal mod:answer\n\
         \x20   cook \"out.txt\" { echo \"$<mod:answer>\" > $<out> }\n",
    )
    .unwrap();

    build(wd, "emit");
    assert_eq!(fs::read_to_string(wd.join("out.txt")).unwrap().trim(), "ORIGINAL");
    assert_eq!(runs(wd, "probelog"), 1);

    // The probe's own cache must still work: a settled run re-produces nothing.
    build(wd, "emit");
    assert_eq!(
        runs(wd, "probelog"),
        1,
        "a keyed probe that loads a module must still hit its own cache"
    );

    write_helper(wd, "REVISED");
    build(wd, "emit");
    assert_eq!(runs(wd, "probelog"), 2, "the module moved, so produce must run");
    assert_eq!(
        fs::read_to_string(wd.join("out.txt")).unwrap().trim(),
        "REVISED",
        "the probe's value came from module code that changed"
    );
}

/// The severity bar. Two working trees share one content-addressed store; their
/// modules differ. Neither may be served the other's answer.
#[test]
fn a_shared_store_does_not_carry_a_result_across_differing_modules() {
    let store = tempfile::tempdir().unwrap();
    let cloud = format!(
        "[cache]\ncache_dir = {:?}\n",
        store.path().to_string_lossy()
    );
    let cookfile = "recipe emit\n\
                    \x20   cook \"out.txt\" >{\n\
                    \x20       local h = cook.load_module(\"helper\")\n\
                    \x20       fs.write(\"out.txt\", h.value())\n\
                    \x20   }\n";

    let make = |value: &str| {
        let tmp = tempfile::tempdir().unwrap();
        let wd = tmp.path();
        fs::create_dir_all(wd.join(".cook")).unwrap();
        fs::write(wd.join(".cook/cloud.toml"), &cloud).unwrap();
        let _ = &store;
        fs::write(wd.join("Cookfile"), cookfile).unwrap();
        write_helper(wd, value);
        build(wd, "emit");
        assert_eq!(
            fs::read_to_string(wd.join("out.txt")).unwrap(),
            value,
            "each machine must produce its own module's answer"
        );
        tmp
    };

    // Machine A publishes under its module's content.
    let _a = make("ALPHA");
    // Machine B has the same Cookfile, the same declared inputs, the same
    // command text, and a different module. Before CS-0204 it composed A's key.
    let _b = make("BETA");

    // And a third tree whose module matches A's must be free to reuse A's
    // entry: the fold must key on content, not merely refuse to share.
    let c = make("ALPHA");
    assert_eq!(fs::read_to_string(c.path().join("out.txt")).unwrap(), "ALPHA");
}
