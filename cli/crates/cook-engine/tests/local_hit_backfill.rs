//! A local-tier hit publishes its recorded entry to the shared store when the
//! store lacks it, so a result built in one checkout is a shared hit from
//! another. Real `cook` binary; every spawn gets its own XDG_CACHE_HOME.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::SystemTime;

fn cook_binary() -> PathBuf {
    let mut path = std::env::current_exe().unwrap();
    path.pop();
    path.pop();
    path.push("cook");
    assert!(path.exists(), "run `cargo build --bin cook` first");
    path
}

struct Rig {
    tmp: tempfile::TempDir,
    store: PathBuf,
}

impl Rig {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let store = tmp.path().join("store");
        fs::create_dir_all(&store).unwrap();
        Rig { tmp, store }
    }

    /// A checkout at `name` under the rig, with `publish` as given.
    fn checkout(&self, name: &str, cookfile: &str, publish: bool) -> PathBuf {
        let wd = self.tmp.path().join(name);
        fs::create_dir_all(wd.join("src")).unwrap();
        fs::create_dir_all(wd.join("out")).unwrap();
        fs::write(wd.join("src/in.txt"), "src-content\n").unwrap();
        fs::write(wd.join("Cookfile"), cookfile).unwrap();
        self.set_publish(&wd, publish);
        wd
    }

    fn set_publish(&self, wd: &Path, publish: bool) {
        fs::create_dir_all(wd.join(".cook")).unwrap();
        let mut body = format!("[cache]\ncache_dir = {:?}\n", self.store.to_string_lossy());
        if !publish {
            body.push_str("\n[cloud]\npublish = false\n");
        }
        fs::write(wd.join(".cook/cloud.toml"), body).unwrap();
    }

    fn run(&self, wd: &Path, args: &[&str], envs: &[(&str, &str)]) -> Output {
        let mut c = Command::new(cook_binary());
        c.args(args)
            .current_dir(wd)
            .env("XDG_CACHE_HOME", self.tmp.path().join("xdg"));
        for (k, v) in envs {
            c.env(k, v);
        }
        let out = c.output().expect("cook invocation");
        assert!(
            out.status.success(),
            "cook {args:?} failed:\n{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        out
    }

    fn files(&self) -> Vec<(PathBuf, SystemTime)> {
        let mut found = Vec::new();
        let mut stack = vec![self.store.clone()];
        while let Some(p) = stack.pop() {
            for e in fs::read_dir(&p).into_iter().flatten().flatten() {
                let t = e.file_type().unwrap();
                if t.is_dir() {
                    stack.push(e.path());
                } else if t.is_file() {
                    found.push((e.path(), e.metadata().unwrap().modified().unwrap()));
                }
            }
        }
        found.sort();
        found
    }
}

fn runs(wd: &Path, log: &str) -> usize {
    fs::read_to_string(wd.join("out").join(log)).map_or(0, |s| s.lines().count())
}

const COOK_STEP: &str = "recipe make\n    gather \"src/in.txt\"\n    cook \"out/art.txt\" {\n        cp $<in> out/art.txt\n        echo ran >> out/art.runlog\n    }\n";

#[test]
fn local_hit_of_producing_unit_is_a_shared_hit_elsewhere() {
    let rig = Rig::new();
    let a = rig.checkout("a", COOK_STEP, false);
    rig.run(&a, &["make"], &[]);
    assert_eq!(runs(&a, "art.runlog"), 1);
    assert!(rig.files().is_empty(), "publish-off build must not publish");

    rig.set_publish(&a, true);
    rig.run(&a, &["make"], &[]);
    assert_eq!(runs(&a, "art.runlog"), 1, "second run is a local hit");
    assert!(!rig.files().is_empty(), "the local hit must backfill the store");

    let b = rig.checkout("b", COOK_STEP, true);
    rig.run(&b, &["make"], &[]);
    assert_eq!(runs(&b, "art.runlog"), 0, "B is a shared hit");
    assert_eq!(
        fs::read(b.join("out/art.txt")).unwrap(),
        fs::read(a.join("out/art.txt")).unwrap()
    );
}

const TEST_STEP: &str = "recipe r\n    gather \"src/in.txt\"\n    test { cat $<in> >/dev/null; echo ran >> out/t.runlog }\n";

#[test]
fn local_hit_of_observing_unit_is_a_shared_hit_elsewhere() {
    let rig = Rig::new();
    let a = rig.checkout("a", TEST_STEP, false);
    rig.run(&a, &["test"], &[]);
    assert_eq!(runs(&a, "t.runlog"), 1);
    assert!(rig.files().is_empty());

    rig.set_publish(&a, true);
    rig.run(&a, &["test"], &[]);
    assert_eq!(runs(&a, "t.runlog"), 1, "local hit");
    assert!(!rig.files().is_empty(), "the local hit must backfill the store");

    let b = rig.checkout("b", TEST_STEP, true);
    let out = rig.run(&b, &["test"], &[]);
    assert_eq!(runs(&b, "t.runlog"), 0, "B's body must not run");
    assert!(String::from_utf8_lossy(&out.stdout).contains("cached"));
}

#[test]
fn local_step_never_backfills() {
    let rig = Rig::new();
    let cf = COOK_STEP.replace("    }\n", "    } local\n");
    let a = rig.checkout("a", &cf, false);
    rig.run(&a, &["make"], &[]);
    rig.set_publish(&a, true);
    rig.run(&a, &["make"], &[]);
    assert_eq!(runs(&a, "art.runlog"), 1, "local hit");
    assert!(rig.files().is_empty(), "a local unit must not reach the store");
}

#[test]
fn no_publish_run_does_not_backfill() {
    let rig = Rig::new();
    let a = rig.checkout("a", COOK_STEP, false);
    rig.run(&a, &["make"], &[]);
    rig.set_publish(&a, true);
    rig.run(&a, &["make"], &[("COOK_NO_PUBLISH", "1")]);
    assert_eq!(runs(&a, "art.runlog"), 1);
    assert!(rig.files().is_empty(), "publish-off invocation must not backfill");
}

#[test]
fn hit_already_in_store_rewrites_nothing() {
    let rig = Rig::new();
    let a = rig.checkout("a", COOK_STEP, true);
    rig.run(&a, &["make"], &[]);
    let before = rig.files();
    assert!(!before.is_empty());
    std::thread::sleep(std::time::Duration::from_millis(1100));
    rig.run(&a, &["make"], &[]);
    assert_eq!(runs(&a, "art.runlog"), 1);
    assert_eq!(rig.files(), before, "present keys must not be re-uploaded");
}

/// A checkout whose root imports `./api`; the unit under test lives in the
/// import and seals a probe declared there, so its probe value is scoped to
/// the imported recipe.
fn import_checkout(rig: &Rig, name: &str, publish: bool) -> PathBuf {
    let root = "import api ./api\n\nrecipe make: api.build\n    cook \"out/top.txt\" { echo top > $<out> }\n";
    let wd = rig.checkout(name, root, publish);
    fs::create_dir_all(wd.join("api")).unwrap();
    fs::write(wd.join("input.txt"), "probe-input\n").unwrap();
    fs::write(
        wd.join("api/Cookfile"),
        "probe ver\n    >{\n        local h = io.open(\"input.txt\")\n        local c = h:read(\"*a\")\n        h:close()\n        return { v = c }\n    }\n\nrecipe build\n    seal ver\n    cook \"out.txt\" {\n        echo run >> marker.log\n        echo $<ver.v> > $<out>\n    }\n",
    )
    .unwrap();
    wd
}

#[test]
fn local_hit_of_imported_sealing_unit_is_a_shared_hit_elsewhere() {
    let rig = Rig::new();
    let a = import_checkout(&rig, "a", false);
    rig.run(&a, &["make"], &[]);
    let marker = |wd: &Path| fs::read_to_string(wd.join("api/marker.log")).map_or(0, |s| s.lines().count());
    assert_eq!(marker(&a), 1);
    assert!(rig.files().is_empty(), "publish-off build must not publish");

    rig.set_publish(&a, true);
    rig.run(&a, &["make"], &[]);
    assert_eq!(marker(&a), 1, "second run is a local hit");
    assert!(!rig.files().is_empty(), "the local hit must backfill the store");

    let b = import_checkout(&rig, "b", true);
    rig.run(&b, &["make"], &[]);
    assert_eq!(marker(&b), 0, "B is a shared hit under the key A published");
}

/// A later unit rewrites an earlier unit's output after the earlier one's
/// cache hit; the backfill must not publish the changed bytes under the old key.
#[test]
fn backfill_refuses_a_file_changed_after_its_hit() {
    let rig = Rig::new();
    let cf = |body: &str| format!("recipe make\n    gather \"src/in.txt\"\n    cook \"out/art.txt\" {{\n        cp $<in> out/art.txt\n    }}\n    cook \"out/other.txt\" {{\n        {body} > out/other.txt\n    }}\n");
    let a = rig.checkout("a", &cf("echo other"), false);
    rig.run(&a, &["make"], &[]);
    assert!(rig.files().is_empty());
    rig.set_publish(&a, true);
    // The second unit re-runs (new command) and clobbers the first's output;
    // the first is still a local hit.
    fs::write(a.join("Cookfile"), cf("echo tampered > out/art.txt; echo other2")).unwrap();
    rig.run(&a, &["make"], &[]);
    assert_eq!(fs::read_to_string(a.join("out/art.txt")).unwrap(), "tampered\n");
    let published_changed = rig
        .files()
        .into_iter()
        .any(|(p, _)| fs::read(&p).is_ok_and(|b| b == b"tampered\n"));
    assert!(!published_changed, "bytes must not go up under a key their file no longer matches");
}

// ── replaying a truncated observation (§17.1.5 Size, §17.1.3) ──────────────

const ELIDED: &str = "bytes of the recorded output elided";

const PRINTING_TEST: &str = "recipe r\n    gather \"src/in.txt\"\n    test { echo RECORDED-LINE; cat $<in> >/dev/null }\n";
const PRINTING_COOK: &str = "recipe make\n    gather \"src/in.txt\"\n    cook \"out/art.txt\" {\n        echo RECORDED-LINE\n        cp $<in> out/art.txt\n    }\n";

fn sidecar(wd: &Path) -> String {
    fs::read_to_string(wd.join(".cook/test-report.json")).unwrap()
}

fn printed(out: &Output) -> String {
    format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr))
}

/// Caps the recorded log of every unit `wd` runs at `cap` bytes.
fn cap_observations(wd: &Path, cap: u64) {
    let p = wd.join(".cook/cloud.toml");
    let body = fs::read_to_string(&p).unwrap();
    fs::write(&p, body.replace("[cache]\n", &format!("[cache]\nobservation_max_bytes = {cap}\n"))).unwrap();
}

#[test]
fn a_backfilled_test_unit_replays_with_the_elision_marker() {
    let rig = Rig::new();
    let a = rig.checkout("a", PRINTING_TEST, false);
    rig.run(&a, &["test"], &[]);
    assert!(sidecar(&a).contains("RECORDED-LINE"), "the cold run printed");

    // Publish-off: the store holds no log, so a replay has only the scalar.
    rig.run(&a, &["--replay-logs", "test"], &[]);
    let local = sidecar(&a);
    assert!(local.contains("\"from_cache\": true") && local.contains(ELIDED), "{local}");

    rig.set_publish(&a, true);
    rig.run(&a, &["test"], &[]);
    assert!(!rig.files().is_empty(), "the local hit must backfill the store");

    rig.run(&a, &["--replay-logs", "test"], &[]);
    let backfilled = sidecar(&a);
    assert!(backfilled.contains(ELIDED), "local hit on a backfilled log: {backfilled}");
    assert!(!backfilled.contains("RECORDED-LINE"), "{backfilled}");

    let b = rig.checkout("b", PRINTING_TEST, true);
    rig.run(&b, &["--replay-logs", "test"], &[]);
    let shared = sidecar(&b);
    assert!(shared.contains("\"from_cache\": true"), "B is a shared hit: {shared}");
    assert!(shared.contains(ELIDED), "shared hit on a backfilled log: {shared}");
}

#[test]
fn a_backfilled_cook_unit_replays_with_the_elision_marker() {
    let rig = Rig::new();
    let a = rig.checkout("a", PRINTING_COOK, false);
    rig.run(&a, &["make"], &[]);
    rig.set_publish(&a, true);
    rig.run(&a, &["make"], &[]);
    assert!(!rig.files().is_empty(), "the local hit must backfill the store");

    let out = printed(&rig.run(&a, &["--replay-logs", "make"], &[]));
    assert!(out.contains("[replayed] ... (") && out.contains(ELIDED), "{out}");
    assert!(!out.contains("RECORDED-LINE"), "{out}");
}

#[test]
fn a_log_truncated_at_record_time_replays_its_fragment_and_the_marker() {
    let rig = Rig::new();
    let long = "x".repeat(200);
    let test_cf = PRINTING_TEST.replace("RECORDED-LINE", &format!("HEAD{long}TAIL"));
    let cook_cf = PRINTING_COOK.replace("RECORDED-LINE", &format!("HEAD{long}TAIL"));

    let t = rig.checkout("t", &test_cf, true);
    cap_observations(&t, 16);
    rig.run(&t, &["test"], &[]);
    rig.run(&t, &["--replay-logs", "test"], &[]);
    let replayed = sidecar(&t);
    assert!(replayed.contains("HEAD") && replayed.contains(ELIDED), "{replayed}");
    assert!(!replayed.contains(&long), "{replayed}");

    let c = rig.checkout("c", &cook_cf, true);
    cap_observations(&c, 16);
    rig.run(&c, &["make"], &[]);
    let out = printed(&rig.run(&c, &["--replay-logs", "make"], &[]));
    assert!(out.contains("[replayed] HEAD") && out.contains(ELIDED), "{out}");
}

#[test]
fn a_complete_log_replays_without_the_marker() {
    let rig = Rig::new();
    let t = rig.checkout("t", PRINTING_TEST, true);
    rig.run(&t, &["test"], &[]);
    rig.run(&t, &["--replay-logs", "test"], &[]);
    let replayed = sidecar(&t);
    assert!(replayed.contains("RECORDED-LINE") && !replayed.contains(ELIDED), "{replayed}");

    let c = rig.checkout("c", PRINTING_COOK, true);
    rig.run(&c, &["make"], &[]);
    let out = printed(&rig.run(&c, &["--replay-logs", "make"], &[]));
    assert!(out.contains("[replayed] RECORDED-LINE") && !out.contains(ELIDED), "{out}");
}
