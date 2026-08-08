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

/// `helper` is loaded BY NAME, so it is installed where a rock installs it.
/// Through `cook_contracts::layout` so the next move of the tree root does not
/// have to touch this test (CS-0207).
fn write_helper(wd: &Path, value: &str) {
    let share = cook_contracts::layout::modules_dir(wd)
        .join(cook_contracts::layout::MODULES_SHARE_LUA_SUBDIR);
    fs::create_dir_all(&share).unwrap();
    fs::write(share.join("helper.lua"), HELPER.replace("%V%", value)).unwrap();
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
/// modules differ. Neither may be served the other's answer — and a third tree
/// whose module MATCHES must still be served, because the fix has to key on
/// module content rather than merely refuse to share.
///
/// The hit is asserted through a side-effect log, not through the declared
/// output: a restored output and a rebuilt one hold the same bytes, so
/// comparing them proves nothing about whether the body ran.
#[test]
fn a_shared_store_does_not_carry_a_result_across_differing_modules() {
    let store = tempfile::tempdir().unwrap();
    let cookfile = "recipe emit\n\
                    \x20   cook \"out.txt\" >{\n\
                    \x20       local h = cook.load_module(\"helper\")\n\
                    \x20       cook.sh(\"echo ran >> runlog\")\n\
                    \x20       fs.write(\"out.txt\", h.value())\n\
                    \x20   }\n";

    // Each call is a fresh "machine": its own working tree and its own local
    // index, sharing only the CAS.
    let machine = |value: &str| -> (tempfile::TempDir, usize) {
        let tmp = tempfile::tempdir().unwrap();
        let wd = tmp.path();
        isolate_store(wd, store.path());
        fs::write(wd.join("Cookfile"), cookfile).unwrap();
        write_helper(wd, value);
        build(wd, "emit");
        assert_eq!(
            fs::read_to_string(wd.join("out.txt")).unwrap(),
            value,
            "each machine must end up with its own module's answer"
        );
        let executions = runs(wd, "runlog");
        (tmp, executions)
    };

    let (_a, a_runs) = machine("ALPHA");
    assert_eq!(a_runs, 1, "the first machine has nothing to fetch");

    // Same Cookfile, same declared inputs, same command text, different module.
    // Before CS-0204 this composed A's key and was served A's artifact.
    let (_b, b_runs) = machine("BETA");
    assert_eq!(b_runs, 1, "a differing module must not be served A's answer");

    // Same module content as A: the fold must let this one reuse A's entry.
    let (_c, c_runs) = machine("ALPHA");
    assert_eq!(
        c_runs, 0,
        "a matching module must still cold-fetch through the module manifest"
    );
}

/// The observing (output-less) half of the same bar. A `test` body has no
/// artifact to restore, so its shared hit replays a recorded verdict — through
/// a different code path, which is exactly why it gets its own test.
#[test]
fn a_shared_verdict_is_reused_only_across_matching_modules() {
    let store = tempfile::tempdir().unwrap();
    // `ingredients` gives the test unit something to key on: an output-less
    // unit that declares nothing has nothing whose movement could invalidate
    // it, so §17.4 rule 1 refuses it a key entirely (CS-0186).
    let cookfile = "recipe check\n\
                    \x20   ingredients \"seed.txt\"\n\
                    \x20   test >{\n\
                    \x20       local h = cook.load_module(\"helper\")\n\
                    \x20       cook.sh(\"echo ran >> runlog\")\n\
                    \x20       assert(h.value() ~= nil)\n\
                    \x20   }\n";

    let machine = |value: &str| -> (tempfile::TempDir, usize) {
        let tmp = tempfile::tempdir().unwrap();
        let wd = tmp.path();
        isolate_store(wd, store.path());
        fs::write(wd.join("Cookfile"), cookfile).unwrap();
        fs::write(wd.join("seed.txt"), "seed\n").unwrap();
        write_helper(wd, value);
        build(wd, "check");
        let executions = runs(wd, "runlog");
        (tmp, executions)
    };

    let (_a, a_runs) = machine("ALPHA");
    assert_eq!(a_runs, 1);
    let (_b, b_runs) = machine("BETA");
    assert_eq!(b_runs, 1, "a differing module must not replay A's verdict");
    let (_c, c_runs) = machine("ALPHA");
    assert_eq!(
        c_runs, 0,
        "a matching module must still replay the shared verdict"
    );
}
