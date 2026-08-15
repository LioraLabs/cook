use super::*;

fn captured(command: &str, dir: &Path) -> Outcome {
    run::<&str, &str>(
        &Spawn { command, working_dir: dir, stdio: Stdio::Captured },
        std::iter::empty(),
    )
    .expect("spawn")
}

#[test]
fn both_streams_are_captured_on_success() {
    // The defect this crate exists to remove: the worker's `cook.sh` returned
    // stdout and dropped stderr outright, so a command that succeeded with
    // warnings reported none of them.
    let dir = tempfile::tempdir().unwrap();
    let o = captured("echo OUT; echo ERR >&2", dir.path());
    assert!(o.success());
    assert_eq!(o.stdout_lossy().trim(), "OUT");
    assert_eq!(o.stream_lossy(OutputStream::Stderr).trim(), "ERR");
}

#[test]
fn a_silent_command_contributes_no_chunks() {
    let dir = tempfile::tempdir().unwrap();
    let o = captured("true", dir.path());
    assert!(o.success());
    assert!(o.chunks().is_empty(), "expected no chunks, got {:?}", o.chunks());
}

#[test]
fn one_spawn_yields_at_most_one_chunk_per_stream() {
    // CS-0188's ordering limit, at the level this function is responsible for.
    // A command interleaving writes cannot be reported as interleaved, because
    // the two pipes are buffered separately; it must not pretend otherwise by
    // emitting a chunk per write.
    let dir = tempfile::tempdir().unwrap();
    let o = captured("echo a; echo b >&2; echo c; echo d >&2", dir.path());
    assert_eq!(o.chunks().len(), 2);
    assert_eq!(o.chunks()[0].stream(), OutputStream::Stdout);
    assert_eq!(o.chunks()[1].stream(), OutputStream::Stderr);
    assert_eq!(o.stdout_lossy(), "a\nc\n");
    assert_eq!(o.stream_lossy(OutputStream::Stderr), "b\nd\n");
}

#[test]
fn a_multi_line_block_is_one_spawn() {
    // `cook_contracts::shell_block::compose` joins a block's lines under
    // `set -e` into one command string, so a body with N lines is one process,
    // not N (§{steps.shell-block-invocation}). If that ever changes, the chunk
    // count here changes with it.
    let dir = tempfile::tempdir().unwrap();
    let o = captured("set -e\necho one\necho two\necho three", dir.path());
    assert_eq!(o.chunks().len(), 1);
    assert_eq!(o.stdout_lossy(), "one\ntwo\nthree\n");
}

#[test]
fn set_e_stops_at_the_first_failure_and_reports_its_status() {
    let dir = tempfile::tempdir().unwrap();
    let o = captured("set -e\necho before\nexit 3\necho after", dir.path());
    assert!(!o.success());
    assert_eq!(o.exit_code(), Some(3));
    assert_eq!(o.stdout_lossy(), "before\n");
}

#[test]
fn failure_carries_both_streams_and_none_on_success() {
    let dir = tempfile::tempdir().unwrap();

    let ok = captured("echo fine", dir.path());
    assert!(ok.failure(7, "echo fine").is_none());

    let bad = captured("echo OUT; echo ERR >&2; exit 2", dir.path());
    let f = bad.failure(7, "the command").expect("a failed command has a failure");
    assert_eq!(f.line(), 7);
    assert_eq!(f.exit_code(), 2);
    assert_eq!(f.command(), "the command");
    assert_eq!(f.stdout().as_str().trim(), "OUT");
    assert_eq!(f.stderr().as_str().trim(), "ERR");
}

#[test]
fn the_overlay_reaches_the_child_and_the_ambient_environment_survives() {
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("COOK_SHELL_AMBIENT_PROBE", "ambient");
    let o = run(
        &Spawn {
            command: "printf '%s/%s' \"$COOK_SHELL_AMBIENT_PROBE\" \"$COOK_SHELL_OVERLAY_PROBE\"",
            working_dir: dir.path(),
            stdio: Stdio::Captured,
        },
        [("COOK_SHELL_OVERLAY_PROBE", "overlaid")],
    )
    .expect("spawn");
    assert_eq!(o.stdout_lossy(), "ambient/overlaid");
    std::env::remove_var("COOK_SHELL_AMBIENT_PROBE");
}

#[test]
fn the_overlay_accepts_the_map_types_the_callers_actually_hold() {
    // Five call sites hold a HashMap and one holds a BTreeMap. Both must pass
    // without the caller rebuilding its map.
    let dir = tempfile::tempdir().unwrap();
    let hash: std::collections::HashMap<String, String> =
        [("COOK_SHELL_MAP_PROBE".to_string(), "h".to_string())].into_iter().collect();
    let btree: std::collections::BTreeMap<String, String> =
        [("COOK_SHELL_MAP_PROBE".to_string(), "b".to_string())].into_iter().collect();
    let cmd = "printf '%s' \"$COOK_SHELL_MAP_PROBE\"";

    let a = run(&Spawn { command: cmd, working_dir: dir.path(), stdio: Stdio::Captured }, &hash)
        .expect("spawn");
    assert_eq!(a.stdout_lossy(), "h");

    let b = run(&Spawn { command: cmd, working_dir: dir.path(), stdio: Stdio::Captured }, &btree)
        .expect("spawn");
    assert_eq!(b.stdout_lossy(), "b");
}

#[test]
fn the_command_runs_in_the_given_directory() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("marker.txt"), b"x").unwrap();
    let o = captured("ls marker.txt", dir.path());
    assert!(o.success());
    assert_eq!(o.stdout_lossy().trim(), "marker.txt");
}

#[test]
fn inherited_stdio_captures_nothing_but_still_reports_status() {
    // The interactive path: the child owns the terminal, so there are no chunks
    // to attribute, and the exit status is the whole report.
    let dir = tempfile::tempdir().unwrap();
    let o = run::<&str, &str>(
        &Spawn { command: "exit 5", working_dir: dir.path(), stdio: Stdio::Inherited },
        std::iter::empty(),
    )
    .expect("spawn");
    assert!(!o.success());
    assert_eq!(o.exit_code(), Some(5));
    assert!(o.chunks().is_empty());
}

#[test]
fn invalid_utf8_on_a_stream_survives_to_the_chunk() {
    let dir = tempfile::tempdir().unwrap();
    let o = captured("printf 'ok\\377!'", dir.path());
    assert_eq!(o.chunks().len(), 1);
    assert_eq!(o.chunks()[0].bytes(), b"ok\xff!");
    assert_eq!(o.stdout_lossy(), "ok\u{fffd}!");
}

#[test]
fn a_command_that_cannot_start_is_an_error_not_an_outcome() {
    // Distinguishing "ran and failed" from "never ran" is the caller's cue to
    // report a spawn problem rather than a build failure.
    let missing = Path::new("/definitely/not/a/directory/cook-shell-test");
    let e = run::<&str, &str>(
        &Spawn { command: "true", working_dir: missing, stdio: Stdio::Captured },
        std::iter::empty(),
    );
    assert!(e.is_err());
}
