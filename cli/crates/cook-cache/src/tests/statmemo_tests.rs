//! COOK-306: per-run mtime memo.
//!
//! Every test drives its own [`StatMemo`] rather than the process-wide one:
//! sibling tests in this crate exercise `check_inputs` and `try_restore`,
//! which arm and disarm the global instance, so testing through it would be
//! order-dependent under `cargo test`'s parallel threads.

use super::*;

fn write(path: &std::path::Path, body: &str) {
    std::fs::write(path, body).expect("write");
}

/// Rewrite a file until its observed mtime actually moves — a filesystem with
/// coarse timestamp granularity can report the same mtime for a fast rewrite.
fn touch_forward(path: &std::path::Path) {
    let before = crate::check::stat_mtime(path).expect("mtime");
    for _ in 0..200 {
        write(path, "changed");
        if crate::check::stat_mtime(path) != Some(before) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("mtime never advanced");
}

#[test]
fn disarmed_by_default_reads_through() {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("a.txt");
    write(&f, "one");
    let memo = StatMemo::new();
    assert!(!memo.is_armed(), "a fresh memo must start disarmed");

    let first = memo.stat_mtime(dir.path(), "a.txt").expect("mtime");
    touch_forward(&f);
    let second = memo.stat_mtime(dir.path(), "a.txt").expect("mtime");

    assert_ne!(first, second, "a disarmed memo must not cache anything");
}

#[test]
fn armed_memo_serves_the_first_answer() {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("b.txt");
    write(&f, "one");
    let memo = StatMemo::new();

    memo.arm();
    let first = memo.stat_mtime(dir.path(), "b.txt").expect("mtime");
    touch_forward(&f);
    assert_eq!(
        Some(first),
        memo.stat_mtime(dir.path(), "b.txt"),
        "an armed memo serves the memoised value"
    );

    // ...and the first write disarms it, so the next read sees the truth.
    memo.disarm();
    assert_ne!(
        Some(first),
        memo.stat_mtime(dir.path(), "b.txt"),
        "disarm must expose the real mtime again"
    );
}

#[test]
fn disarm_is_permanent_until_rearmed() {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("c.txt");
    write(&f, "one");
    let memo = StatMemo::new();

    memo.arm();
    memo.disarm();
    let first = memo.stat_mtime(dir.path(), "c.txt").expect("mtime");
    touch_forward(&f);
    let second = memo.stat_mtime(dir.path(), "c.txt").expect("mtime");

    assert_ne!(first, second, "reads after disarm must stay uncached");
}

#[test]
fn missing_paths_memoise_as_missing() {
    let dir = tempfile::tempdir().unwrap();
    let memo = StatMemo::new();

    memo.arm();
    assert_eq!(memo.stat_mtime(dir.path(), "gone.txt"), None);
    // A memoised `None` errs in the conservative direction: callers treat an
    // unreadable input as changed, so a file appearing mid-run can only cause
    // an unnecessary rebuild, never a false cache hit.
    write(&dir.path().join("gone.txt"), "now here");
    assert_eq!(memo.stat_mtime(dir.path(), "gone.txt"), None);

    memo.disarm();
    assert!(memo.stat_mtime(dir.path(), "gone.txt").is_some());
}

#[test]
fn same_relative_path_in_two_working_dirs_does_not_alias() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    write(&a.path().join("same.txt"), "a");
    let memo = StatMemo::new();
    // Distinct mtimes, so aliasing the two working dirs would be visible.
    std::thread::sleep(std::time::Duration::from_millis(20));
    write(&b.path().join("same.txt"), "bb");

    memo.arm();
    let from_a = memo.stat_mtime(a.path(), "same.txt");
    let from_b = memo.stat_mtime(b.path(), "same.txt");

    assert_eq!(from_a, crate::check::stat_mtime(&a.path().join("same.txt")));
    assert_eq!(from_b, crate::check::stat_mtime(&b.path().join("same.txt")));
    assert_ne!(from_a, from_b);
}

/// The engine's arm point must be reachable through the free functions, and
/// the process-wide instance must start disarmed so a consumer that never
/// arms it (the DAG viewer, `cook verify`) is unaffected.
#[test]
fn global_memo_reads_through_when_never_armed() {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("g.txt");
    write(&f, "one");

    // Deliberately does not call `arm()`: that would race sibling tests.
    let first = stat_mtime_memo(dir.path(), "g.txt").expect("mtime");
    touch_forward(&f);
    let second = stat_mtime_memo(dir.path(), "g.txt").expect("mtime");
    if !GLOBAL.is_armed() {
        assert_ne!(first, second, "an unarmed global memo reads through");
    }
}
