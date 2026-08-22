#![cfg(target_os = "linux")]

use std::fs;
use std::path::Path;
use std::process::{Child, Command};
use std::thread;
use std::time::{Duration, Instant};

use tempfile::TempDir;

fn cook() -> &'static str {
    env!("CARGO_BIN_EXE_cook")
}

fn write_cookfile(dir: &Path, body: &str) {
    fs::write(dir.join("Cookfile"), format!("chore own\n{body}")).expect("write Cookfile");
}

fn child_pid(dir: &Path) -> u32 {
    let path = dir.join("child.pid");
    let deadline = Instant::now() + Duration::from_secs(3);
    while !path.exists() {
        assert!(
            Instant::now() < deadline,
            "background descendant never wrote {path:?}"
        );
        thread::sleep(Duration::from_millis(10));
    }
    fs::read_to_string(path)
        .expect("read child pid")
        .trim()
        .parse()
        .expect("numeric child pid")
}

fn alive(pid: u32) -> bool {
    let Ok(stat) = fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return false;
    };
    let Some((_, fields)) = stat.rsplit_once(") ") else {
        return false;
    };
    if fields.starts_with('Z') {
        return false;
    }
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .status()
        .expect("run kill -0")
        .success()
}

fn reap(pid: u32) {
    let _ = Command::new("kill")
        .args(["-KILL", &pid.to_string()])
        .status();
}

fn wait_for_exit(pid: u32) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while alive(pid) {
        assert!(
            Instant::now() < deadline,
            "descendant {pid} survived the chore"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn start_target(dir: &Path, target: &str) -> Child {
    Command::new(cook())
        .arg(target)
        .current_dir(dir)
        .spawn()
        .expect("start cook")
}

fn start(dir: &Path) -> Child {
    start_target(dir, "own")
}

fn wait_for_cook_exit(cook: &mut Child) -> std::process::ExitStatus {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(status) = cook.try_wait().expect("poll cook") {
            return status;
        }
        if Instant::now() >= deadline {
            let _ = cook.kill();
            let _ = cook.wait();
            panic!("Ctrl-C left the pure Lua chore running");
        }
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn normal_completion_drains_a_background_descendant_across_steps() {
    let dir = TempDir::new().unwrap();
    write_cookfile(
        dir.path(),
        "    sh -c '(sleep 30 & echo $! > child.pid) &'\n    >{ cook.sh(\"test -s child.pid\") }\n    sh -c 'test -s child.pid'\n",
    );

    let out = start(dir.path()).wait_with_output().expect("wait for cook");
    assert!(out.status.success(), "normal chore failed: {out:?}");
    let pid = child_pid(dir.path());
    if alive(pid) {
        reap(pid);
        panic!("descendant {pid} survived normal chore completion");
    }
}

#[test]
fn ctrl_c_drains_a_background_descendant() {
    let dir = TempDir::new().unwrap();
    write_cookfile(
        dir.path(),
        "    sh -c '(sleep 30 & echo $! > child.pid) &'\n    >{ cook.sh(\"test -s child.pid\") }\n    sh -c 'while :; do sleep 1; done'\n",
    );

    let cook = start(dir.path());
    let pid = child_pid(dir.path());
    Command::new("kill")
        .args(["-INT", &cook.id().to_string()])
        .status()
        .expect("send SIGINT to cook");
    let out = cook.wait_with_output().expect("wait for interrupted cook");
    assert!(
        !out.status.success(),
        "interrupted chore unexpectedly succeeded: {out:?}"
    );
    if alive(pid) {
        reap(pid);
        panic!("descendant {pid} survived Ctrl-C");
    }
    wait_for_exit(pid);
}

#[test]
fn ctrl_c_stops_a_pure_lua_chore() {
    let dir = TempDir::new().unwrap();
    write_cookfile(
        dir.path(),
        "    >{ local ready = assert(io.open(\"ready\", \"w\")); ready:write(\"ready\"); ready:close(); while true do end }\n",
    );

    let mut cook = start(dir.path());
    let ready = dir.path().join("ready");
    let deadline = Instant::now() + Duration::from_secs(3);
    while !ready.exists() {
        assert!(Instant::now() < deadline, "pure Lua chore never became ready");
        thread::sleep(Duration::from_millis(10));
    }
    Command::new("kill")
        .args(["-INT", &cook.id().to_string()])
        .status()
        .expect("send SIGINT to cook");
    assert!(!wait_for_cook_exit(&mut cook).success());
}

#[test]
fn a_completed_chore_restores_ctrl_c_for_later_work() {
    let dir = TempDir::new().unwrap();
    fs::write(
        dir.path().join("Cookfile"),
        "chore own\n    true\nrecipe later: own\n    cook \"out\" { touch ready; while :; do sleep 1; done }\n",
    )
    .expect("write Cookfile");

    let mut cook = start_target(dir.path(), "later");
    let ready = dir.path().join("ready");
    let deadline = Instant::now() + Duration::from_secs(3);
    while !ready.exists() {
        assert!(Instant::now() < deadline, "later work never became ready");
        thread::sleep(Duration::from_millis(10));
    }
    Command::new("kill")
        .args(["-INT", &cook.id().to_string()])
        .status()
        .expect("send SIGINT to cook");
    assert!(!wait_for_cook_exit(&mut cook).success());
}
