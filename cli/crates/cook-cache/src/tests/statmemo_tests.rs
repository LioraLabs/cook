//! COOK-306: per-run mtime memo. COOK-414: per-run tool-hash memo.
//!
//! Every test that could observe run-scoped state drives its own [`StatMemo`]
//! or [`ToolHashMemo`] rather than the process-wide one: sibling tests in this
//! crate exercise `check_inputs` and `try_restore`, which arm and disarm the
//! global stat memo, so testing through it would be order-dependent under
//! `cargo test`'s parallel threads. The two tests that do reach a global say in
//! their own doc why they are safe to.

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
/// folded into a probe fingerprint, and into a sealed probe VALUE, at the bytes
/// it had BEFORE cook rebuilt it. On a content-addressed store that crosses
/// machines, that is a false hit, not a slow miss.
#[test]
fn a_binary_rebuilt_mid_run_is_hashed_at_its_new_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let tool = dir.path().join("dmap");
    write(&tool, "v1");
    let memo = ToolHashMemo::new();

    assert_eq!(memo.hash(&tool), sha256_of("v1"));
    rewrite_at(
        &tool,
        "v2-rebuilt",
        mtime_of(&tool) + std::time::Duration::from_secs(1),
    );

    assert_eq!(
        memo.hash(&tool),
        sha256_of("v2-rebuilt"),
        "a tool cook rebuilt must not be served at its pre-build hash"
    );
}

/// The memo's whole purpose is a read it does NOT perform, and that is invisible
/// in the return value: a memo and a plain re-hash answer identically. So it is
/// proven by counting reads, not by pinning a digest.
///
/// The earlier version of this test forced mtime backwards and asserted the
/// STALE digest came back, which pinned the memo's residual staleness window as
/// if it were a requirement: widening `FileIdentity` to close that window (which
/// is what this branch went on to do) would have failed a test whose message
/// called the fix a performance regression.
#[test]
fn an_unchanged_file_is_read_once_however_often_it_is_asked_for() {
    let dir = tempfile::tempdir().unwrap();
    let tool = dir.path().join("node");
    write(&tool, "1111");
    let memo = ToolHashMemo::new();

    for _ in 0..5 {
        assert_eq!(memo.hash(&tool), sha256_of("1111"));
    }

    assert_eq!(memo.reads(), 1, "a 60MB binary must be read once per run");
}

/// The other half: a read the memo MUST perform. Counted, so "it returned the
/// right answer" cannot be satisfied by a memo that never memoised.
#[test]
fn a_rewrite_costs_exactly_one_further_read() {
    let dir = tempfile::tempdir().unwrap();
    let tool = dir.path().join("cc");
    write(&tool, "before");
    let memo = ToolHashMemo::new();

    assert_eq!(memo.hash(&tool), sha256_of("before"));
    rewrite_at(
        &tool,
        "after",
        mtime_of(&tool) + std::time::Duration::from_secs(1),
    );
    assert_eq!(memo.hash(&tool), sha256_of("after"));
    assert_eq!(memo.hash(&tool), sha256_of("after"));

    assert_eq!(memo.reads(), 2);
}

/// The hazard `(mtime, len)` alone cannot see, and the reason [`FileIdentity`]
/// carries more than that pair. A rebuild that reproduces both the modification
/// time and the length is not hypothetical: coarse filesystem timestamp
/// granularity supplies the first (`touch_forward`, above, exists for exactly
/// that) and a relink after a comment-only edit supplies the second. On unix
/// ctime moves anyway, so the memo still re-reads.
///
/// The file is rewritten in place, so inode and device do not move and the
/// discrimination comes from ctime alone. On a filesystem with second-grained
/// ctime this could therefore report a false pass; the tempdir is tmpfs
/// everywhere cook's suite runs, which stamps nanoseconds.
#[cfg(unix)]
#[test]
fn a_rewrite_that_reproduces_mtime_and_length_is_still_caught() {
    let dir = tempfile::tempdir().unwrap();
    let tool = dir.path().join("linker-output");
    write(&tool, "1111");
    let memo = ToolHashMemo::new();
    let pinned = mtime_of(&tool);

    assert_eq!(memo.hash(&tool), sha256_of("1111"));
    rewrite_at(&tool, "2222", pinned);
    assert_eq!(mtime_of(&tool), pinned, "the test must hold mtime fixed");

    assert_eq!(memo.hash(&tool), sha256_of("2222"));
}

/// `which` selects a tool on `X_OK`, so a binary cook cannot READ does reach the
/// memo. It hashes to all-zero like any unreadable path, and the entry must not
/// outlive the permission that caused it: `chmod` moves ctime even though it
/// moves neither mtime nor length.
#[cfg(unix)]
#[test]
fn a_tool_that_becomes_readable_stops_being_served_the_all_zero_digest() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let tool = dir.path().join("execute-only");
    write(&tool, "contents");
    std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o111)).unwrap();
    let memo = ToolHashMemo::new();
    if memo.hash(&tool) != [0u8; 32] {
        // Running as root, where mode 0111 is still readable. Nothing to prove.
        return;
    }

    std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();

    assert_eq!(memo.hash(&tool), sha256_of("contents"));
}

/// Concurrent lookups of the same path may duplicate work, but must never
/// duplicate answers. Deliberately weak: it asserts agreement and NOT
/// `reads() == 1`, because racing threads are allowed to both read a cold path
/// and asserting otherwise is how this test would flake. So it proves no
/// deadlock and no torn answer, and the argument that a racing insert can only
/// cost a redundant read is made where the lock is dropped, not here.
#[test]
fn concurrent_lookups_agree() {
    let dir = tempfile::tempdir().unwrap();
    let tool = dir.path().join("shared");
    write(&tool, "shared bytes");
    let memo = std::sync::Arc::new(ToolHashMemo::new());

    let threads: Vec<_> = (0..8)
        .map(|_| {
            let memo = memo.clone();
            let tool = tool.clone();
            std::thread::spawn(move || memo.hash(&tool))
        })
        .collect();

    for t in threads {
        assert_eq!(t.join().unwrap(), sha256_of("shared bytes"));
    }
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

/// The process-wide instance is reachable through the free function, memoises,
/// and revalidates. Unlike the stat memo's global, this one needs no arming, so
/// it is safe to exercise from a test: it holds no state a sibling test could
/// disturb, only answers it re-checks.
#[test]
fn the_global_tool_hash_memo_memoises_and_revalidates() {
    let dir = tempfile::tempdir().unwrap();
    let tool = dir.path().join("global-tool");
    write(&tool, "content");

    assert_eq!(tool_hash_memo(&tool), crate::probe::hash_file_sha256(&tool));
    assert_eq!(tool_hash_memo(&tool), sha256_of("content"));

    rewrite_at(
        &tool,
        "replaced",
        mtime_of(&tool) + std::time::Duration::from_secs(1),
    );
    assert_eq!(tool_hash_memo(&tool), sha256_of("replaced"));
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
