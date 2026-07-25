//! CS-0171 timing-recovery tests.

use super::*;
use std::io::Write;

/// Write a build directory with the given `events.jsonl` lines. Returns the
/// directory, so callers can stagger mtimes.
fn write_build(root: &Path, id: &str, lines: &[&str]) -> std::path::PathBuf {
    let dir = root.join(".cook").join("logs").join(id);
    fs::create_dir_all(&dir).unwrap();
    let mut f = fs::File::create(dir.join("events.jsonl")).unwrap();
    for l in lines {
        writeln!(f, "{l}").unwrap();
    }
    f.flush().unwrap();
    dir
}

fn completed(recipe: &str, node: &str, key: &str, ms: u64) -> String {
    format!(
        r#"{{"type":"node-completed","recipe":"{recipe}","node":"{node}","elapsed_ms":{ms},"kind":"cooked","cache_key":"{key}"}}"#
    )
}

/// Force `a` to be older than `b` so `build_dirs_newest_first` has a stable
/// order to find. Filesystem mtime granularity makes two directories created in
/// the same test indistinguishable otherwise.
fn make_older(a: &Path, b: &Path) {
    let t = filetime_now();
    set_mtime(a, t - 100);
    set_mtime(b, t);
}

fn filetime_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

fn set_mtime(p: &Path, secs: i64) {
    let t = std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs as u64);
    let f = fs::File::open(p).unwrap();
    f.set_modified(t).unwrap();
}

#[test]
fn recovers_elapsed_by_recipe_and_cache_key() {
    let tmp = tempfile::tempdir().unwrap();
    write_build(tmp.path(), "2026-07-25-aaa", &[&completed("build", "main.o", "step:0", 1500)]);

    let t = Timings::load(tmp.path());
    let o = t.get("build", "step:0").expect("observation recorded");
    assert_eq!(o.elapsed_ms, 1500);
    assert_eq!(o.builds_ago, 0);
    // The join is on the cache key, not the display name.
    assert!(t.get("build", "main.o").is_none());
}

#[test]
fn newest_build_wins_and_reports_distance() {
    let tmp = tempfile::tempdir().unwrap();
    let old = write_build(
        tmp.path(),
        "2026-07-25-aaa",
        &[&completed("build", "main.o", "step:0", 9000)],
    );
    let new = write_build(
        tmp.path(),
        "2026-07-25-bbb",
        &[&completed("build", "main.o", "step:0", 1200)],
    );
    make_older(&old, &new);

    let t = Timings::load(tmp.path());
    let o = t.get("build", "step:0").unwrap();
    assert_eq!(o.elapsed_ms, 1200, "most recent observation wins");
    assert_eq!(o.builds_ago, 0);
}

#[test]
fn older_build_supplies_units_absent_from_the_newest() {
    let tmp = tempfile::tempdir().unwrap();
    let old = write_build(
        tmp.path(),
        "2026-07-25-aaa",
        &[&completed("build", "a.o", "step:0", 700)],
    );
    let new = write_build(
        tmp.path(),
        "2026-07-25-bbb",
        &[&completed("build", "b.o", "step:1", 300)],
    );
    make_older(&old, &new);

    let t = Timings::load(tmp.path());
    assert_eq!(t.get("build", "step:1").unwrap().builds_ago, 0);
    let stale = t.get("build", "step:0").expect("recovered from the older build");
    assert_eq!(stale.elapsed_ms, 700);
    assert_eq!(stale.builds_ago, 1, "reported as one build back, not as current");
}

/// A cache hit emits `node-cache-hit`, which has no `elapsed_ms`. It must
/// contribute no observation rather than an implicit zero.
#[test]
fn cache_hits_contribute_no_observation() {
    let tmp = tempfile::tempdir().unwrap();
    write_build(
        tmp.path(),
        "2026-07-25-aaa",
        &[r#"{"type":"node-cache-hit","recipe":"build","node":"main.o","kind":"cooked"}"#],
    );

    let t = Timings::load(tmp.path());
    assert!(t.is_empty());
    assert!(t.get("build", "step:0").is_none());
}

/// Logs written before CS-0171 carry no `cache_key`. They must be skipped, not
/// misjoined onto some other unit.
#[test]
fn pre_cs0171_records_without_cache_key_are_skipped() {
    let tmp = tempfile::tempdir().unwrap();
    write_build(
        tmp.path(),
        "2026-07-25-aaa",
        &[r#"{"type":"node-completed","recipe":"build","node":"main.o","elapsed_ms":1500,"kind":"cooked"}"#],
    );

    let t = Timings::load(tmp.path());
    assert!(t.is_empty());
}

#[test]
fn null_cache_key_for_a_non_cacheable_node_is_skipped() {
    let tmp = tempfile::tempdir().unwrap();
    write_build(
        tmp.path(),
        "2026-07-25-aaa",
        &[r#"{"type":"node-completed","recipe":"build","node":"lua","elapsed_ms":5,"kind":"cooked","cache_key":null}"#],
    );

    assert!(Timings::load(tmp.path()).is_empty());
}

#[test]
fn malformed_lines_and_output_noise_do_not_derail_the_scan() {
    let tmp = tempfile::tempdir().unwrap();
    write_build(
        tmp.path(),
        "2026-07-25-aaa",
        &[
            r#"{"type":"node-output","recipe":"build","node":"main.o","stream":"stdout","line":"mentions node-completed in output"}"#,
            "not json at all",
            r#"{"type":"node-completed","recipe":"build",BROKEN"#,
            &completed("build", "main.o", "step:0", 42),
        ],
    );

    let t = Timings::load(tmp.path());
    assert_eq!(t.get("build", "step:0").unwrap().elapsed_ms, 42);
    // The output line mentioning the event name must not have been harvested.
    assert_eq!(t.by_unit.len(), 1);
}

#[test]
fn missing_log_root_yields_empty_not_error() {
    let tmp = tempfile::tempdir().unwrap();
    assert!(Timings::load(tmp.path()).is_empty());
}

#[test]
fn durations_render_across_the_range() {
    assert_eq!(render_ms(0), "0ms");
    assert_eq!(render_ms(999), "999ms");
    assert_eq!(render_ms(1000), "1.0s");
    assert_eq!(render_ms(2170), "2.2s");
    assert_eq!(render_ms(59_999), "60.0s");
    assert_eq!(render_ms(60_000), "1m00s");
    assert_eq!(render_ms(134_000), "2m14s");
}
