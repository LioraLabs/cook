//! COOK-306: per-run mtime memo. COOK-414: per-run tool-hash memo.
//!
//! Every test drives its own [`StatMemo`] or [`ToolHashMemo`] rather than the
//! process-wide one: sibling tests in this crate exercise `check_inputs` and
//! `try_restore`, which arm and disarm the global instance, so testing through
//! it would be order-dependent under `cargo test`'s parallel threads.

use super::*;

fn write(path: &std::path::Path, body: &str) {
    std::fs::write(path, body).expect("write");
}

/// Rewrite a file and force its mtime to a stated instant, so a test never
/// depends on the filesystem's timestamp granularity to notice the write.
fn rewrite_at(path: &std::path::Path, body: &str, mtime: std::time::SystemTime) {
    std::fs::write(path, body).expect("write");
    std::fs::File::options()
        .write(true)
        .open(path)
        .expect("open")
        .set_modified(mtime)
        .expect("set mtime");
}

fn mtime_of(path: &std::path::Path) -> std::time::SystemTime {
    std::fs::metadata(path).expect("metadata").modified().expect("mtime")
}

fn sha256_of(body: &str) -> [u8; 32] {
    <sha2::Sha256 as sha2::Digest>::digest(body.as_bytes()).into()
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

// -------------------------------------------------------------------------
// COOK-414: the tool-hash memo
// -------------------------------------------------------------------------

/// The finding this memo was rewritten for. A probe unit is a DAG node
/// evaluated inside `execute_dag`, so a `tools { }` input can name a binary
/// that an upstream node rebuilt minutes earlier in the same process; the same
/// goes for a module calling `cook.tools.id` from an execute-phase body. The
/// old memo answered from its first read forever, so the rebuilt tool was
/// folded into a probe fingerprint — and into a sealed probe VALUE — at the
/// bytes it had BEFORE cook rebuilt it. On a content-addressed store that
/// crosses machines, that is a false hit, not a slow miss.
#[test]
fn a_binary_rebuilt_mid_run_is_hashed_at_its_new_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let tool = dir.path().join("dmap");
    write(&tool, "v1");
    let memo = ToolHashMemo::new();

    assert_eq!(memo.hash(&tool), sha256_of("v1"));
    rewrite_at(&tool, "v2-rebuilt", mtime_of(&tool) + std::time::Duration::from_secs(1));

    assert_eq!(
        memo.hash(&tool),
        sha256_of("v2-rebuilt"),
        "a tool cook rebuilt must not be served at its pre-build hash"
    );
}

/// The rewrite is caught by the file's identity, not by its size, so a rebuild
/// that happens to produce a binary of exactly the same length is caught too.
#[test]
fn a_same_length_rewrite_is_caught() {
    let dir = tempfile::tempdir().unwrap();
    let tool = dir.path().join("cc");
    write(&tool, "aaaa");
    let memo = ToolHashMemo::new();

    assert_eq!(memo.hash(&tool), sha256_of("aaaa"));
    rewrite_at(&tool, "bbbb", mtime_of(&tool) + std::time::Duration::from_secs(1));

    assert_eq!(memo.hash(&tool), sha256_of("bbbb"));
}

/// What "memoised" means here, stated as a test rather than left to the doc: a
/// file whose mtime and length both still read the same is served from the
/// memo without a second read. Proven by making the CONTENT diverge while
/// holding the identity fixed — an implementation that re-read the bytes would
/// return the new digest and fail this.
///
/// This is also the memo's exact residual limitation, and why it is honest to
/// pin it: cook's own writes always move mtime, so the case this test
/// constructs by force is one the build cannot produce.
#[test]
fn a_file_whose_identity_has_not_moved_is_served_from_the_memo() {
    let dir = tempfile::tempdir().unwrap();
    let tool = dir.path().join("node");
    write(&tool, "1111");
    let memo = ToolHashMemo::new();
    let pinned = mtime_of(&tool);

    assert_eq!(memo.hash(&tool), sha256_of("1111"));
    rewrite_at(&tool, "2222", pinned);

    assert_eq!(
        memo.hash(&tool),
        sha256_of("1111"),
        "unchanged mtime and length must not cost a second read of a 60MB binary"
    );
}

/// A tool that VANISHES hashes to all-zero, like any unreadable path, rather
/// than to the digest it had while it existed. The memo must not be the one
/// place in the crate where a missing file keeps its old identity.
#[test]
fn a_vanished_tool_stops_being_served_its_old_digest() {
    let dir = tempfile::tempdir().unwrap();
    let tool = dir.path().join("gone");
    write(&tool, "here");
    let memo = ToolHashMemo::new();

    assert_eq!(memo.hash(&tool), sha256_of("here"));
    std::fs::remove_file(&tool).unwrap();

    assert_eq!(memo.hash(&tool), [0u8; 32]);
}

/// Two tools do not alias, and each keeps its own identity.
#[test]
fn distinct_paths_do_not_alias() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a");
    let b = dir.path().join("b");
    write(&a, "aaa");
    write(&b, "bbb");
    let memo = ToolHashMemo::new();

    assert_eq!(memo.hash(&a), sha256_of("aaa"));
    assert_eq!(memo.hash(&b), sha256_of("bbb"));
    assert_eq!(memo.hash(&a), sha256_of("aaa"));
}

/// The process-wide instance is reachable through the free function and agrees
/// with a direct hash. Unlike the stat memo's global, this one needs no arming,
/// so it is safe to exercise from a test: it holds no run-scoped state that a
/// sibling test could disturb.
#[test]
fn the_global_tool_hash_memo_agrees_with_a_direct_hash() {
    let dir = tempfile::tempdir().unwrap();
    let tool = dir.path().join("global-tool");
    write(&tool, "content");

    assert_eq!(tool_hash_memo(&tool), crate::probe::hash_file_sha256(&tool));
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
