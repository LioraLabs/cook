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

#[cfg(target_os = "linux")]
#[test]
fn process_group_drain_kills_background_descendants() {
    let dir = tempfile::tempdir().unwrap();
    let group = ProcessGroup::new().expect("establish process group");
    let outcome = run_with_group(
        &Spawn { command: "sleep 30 </dev/null >/dev/null 2>&1 & echo $! > child.pid", working_dir: dir.path(), stdio: Stdio::Captured },
        std::iter::empty::<(&str, &str)>(),
        Some(&group),
    )
    .expect("spawn");
    assert!(outcome.success());
    let pid: u32 = std::fs::read_to_string(dir.path().join("child.pid")).unwrap().trim().parse().unwrap();
    group.drain().expect("drain process group");
    let state = std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|stat| stat.rsplit_once(") ").map(|(_, fields)| fields.starts_with('Z')));
    assert!(state.unwrap_or(true), "background child {pid} survived drain");
}

#[cfg(target_os = "linux")]
#[test]
fn dropping_process_group_kills_background_descendants() {
    let dir = tempfile::tempdir().unwrap();
    let pid: u32;
    {
        let group = ProcessGroup::new().expect("establish process group");
        let final_owner = group.clone();
        let outcome = run_with_group(
            &Spawn { command: "sleep 30 </dev/null >/dev/null 2>&1 & echo $! > child.pid", working_dir: dir.path(), stdio: Stdio::Captured },
            std::iter::empty::<(&str, &str)>(),
            Some(&group),
        )
        .expect("spawn");
        assert!(outcome.success());
        pid = std::fs::read_to_string(dir.path().join("child.pid")).unwrap().trim().parse().unwrap();
        drop(group);
        drop(final_owner);
    }
    let state = std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|stat| stat.rsplit_once(") ").map(|(_, fields)| fields.starts_with('Z')));
    assert!(state.unwrap_or(true), "background child {pid} survived ProcessGroup drop");
}

#[cfg(unix)]
#[test]
fn drain_escalates_a_stopped_anchor_without_blocking() {
    use std::sync::mpsc;

    let group = ProcessGroup::new().expect("establish process group");
    assert_eq!(unsafe { libc::kill(-group.inner.pgid, libc::SIGSTOP) }, 0);
    let (done_tx, done_rx) = mpsc::channel();
    let drainer = group.clone();
    let join = std::thread::spawn(move || {
        let _ = done_tx.send(drainer.drain());
    });
    let result = match done_rx.recv_timeout(Duration::from_secs(2)) {
        Ok(result) => result,
        Err(timeout) => {
            let _ = unsafe { libc::kill(-group.inner.pgid, libc::SIGKILL) };
            let _ = join.join();
            panic!("stopped anchor blocked escalation: {timeout}");
        }
    };
    join.join().expect("drain thread panicked");
    result.expect("drain stopped anchor");
    assert!(group.inner.anchor.lock().unwrap().is_none(), "drain left the killed anchor unreaped");
}

#[cfg(all(unix, not(target_os = "linux")))]
#[test]
fn drain_reaps_the_anchor_before_the_posix_group_probe() {
    ProcessGroup::new().expect("establish process group").drain().expect("drain process group");
}

#[cfg(target_os = "linux")]
#[test]
fn inherited_terminal_restores_foreground_and_preserves_signal_state() {
    if let Ok(mode) = std::env::var("COOK_SHELL_TERMINAL_CASE") {
        let root = std::path::PathBuf::from(std::env::var("COOK_SHELL_TERMINAL_ROOT").unwrap());
        let group = ProcessGroup::new().unwrap();
        let previous = unsafe { libc::tcgetpgrp(0) };
        assert!(previous > 0);
        if mode == "cancel" {
            let _ = run_with_group(&Spawn {
                command: "python3 -c 'import os,pathlib,time; assert os.tcgetpgrp(0)==os.getpgrp(); pathlib.Path(\"ready\").write_text(str(os.getpid())); time.sleep(30)'",
                working_dir: &root, stdio: Stdio::Inherited,
            }, std::iter::empty::<(&str, &str)>(), Some(&group));
            // The signal router will terminate the process after draining/restoring.
            std::thread::sleep(Duration::from_secs(5));
            panic!("signal router failed to terminate the process");
        }
        let mut disposition: libc::sigaction = unsafe { std::mem::zeroed() };
        let mut mask: libc::sigset_t = unsafe { std::mem::zeroed() };
        unsafe {
            libc::sigaction(libc::SIGTTOU, std::ptr::null(), &mut disposition);
            libc::pthread_sigmask(libc::SIG_SETMASK, std::ptr::null(), &mut mask);
        }
        for (command, expected) in [
            ("python3 -c 'import os; assert os.tcgetpgrp(0)==os.getpgrp()'", 0),
            ("python3 -c 'import os; assert os.tcgetpgrp(0)==os.getpgrp(); raise SystemExit(37)'", 37),
        ] {
            let result = run_with_group(&Spawn { command, working_dir: &root, stdio: Stdio::Inherited }, std::iter::empty::<(&str, &str)>(), Some(&group)).unwrap();
            assert_eq!(result.exit_code(), Some(expected));
            assert_eq!(unsafe { libc::tcgetpgrp(0) }, previous);
        }
        assert!(run_with_group(
            &Spawn {
                command: "true",
                working_dir: &root.join("missing"),
                stdio: Stdio::Inherited
            },
            std::iter::empty::<(&str, &str)>(),
            Some(&group)
        )
        .is_err());
        assert_eq!(unsafe { libc::tcgetpgrp(0) }, previous);
        let result = run_with_group(
            &Spawn {
                command: "python3 -c 'import os; assert not os.isatty(0)'",
                working_dir: &root,
                stdio: Stdio::Captured,
            },
            std::iter::empty::<(&str, &str)>(),
            Some(&group),
        )
        .unwrap();
        assert!(result.success());
        assert_eq!(unsafe { libc::tcgetpgrp(0) }, previous);
        let mut after: libc::sigaction = unsafe { std::mem::zeroed() };
        let mut after_mask: libc::sigset_t = unsafe { std::mem::zeroed() };
        unsafe {
            libc::sigaction(libc::SIGTTOU, std::ptr::null(), &mut after);
            libc::pthread_sigmask(libc::SIG_SETMASK, std::ptr::null(), &mut after_mask);
            assert_eq!(
                libc::sigismember(&mask, libc::SIGTTOU),
                libc::sigismember(&after_mask, libc::SIGTTOU)
            );
        }
        assert_eq!(disposition.sa_sigaction, after.sa_sigaction);
        group.drain().unwrap();
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let output = Command::new("python3").args(["-c", r#"
import os,pathlib,pty,select,signal,subprocess,sys,time
binary,root=sys.argv[1:]
pid,fd=pty.fork()
if pid==0:
    try:
        original=os.tcgetpgrp(0)
        for mode in ['normal','cancel']:
            env={**os.environ,'COOK_SHELL_TERMINAL_CASE':mode,'COOK_SHELL_TERMINAL_ROOT':root}
            child=subprocess.Popen([binary,'--exact','tests::inherited_terminal_restores_foreground_and_preserves_signal_state','--nocapture'],env=env)
            if mode=='cancel':
                deadline=time.monotonic()+5
                while not pathlib.Path(root,'ready').exists() and time.monotonic()<deadline: time.sleep(.01)
                assert pathlib.Path(root,'ready').exists(), 'foreground command never started'
                os.kill(child.pid,signal.SIGINT)
            status=child.wait(timeout=10)
            assert status==(0 if mode=='normal' else -signal.SIGINT), (mode,status)
            assert os.tcgetpgrp(0)==original, 'foreground ownership not restored: '+mode
        print('foreground restored after success/failure/spawn-error/cancel; captured stdin and signal state preserved',flush=True)
    except BaseException:
        import traceback; traceback.print_exc(); os._exit(1)
    os._exit(0)
output=bytearray();deadline=time.monotonic()+25
while time.monotonic()<deadline:
    if select.select([fd],[],[],.1)[0]:
        try: output.extend(os.read(fd,65536))
        except OSError: break
else:
    os.kill(pid,signal.SIGKILL)
_,status=os.waitpid(pid,0);os.close(fd)
print(output.decode(errors='replace'))
assert os.waitstatus_to_exitcode(status)==0
"#, std::env::current_exe().unwrap().to_str().unwrap(), root.path().to_str().unwrap()]).output().unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
