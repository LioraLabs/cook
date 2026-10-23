//! Isolated view of the project for re-runs that must not touch the real tree.
//!
//! The project is overlay-mounted over ITSELF inside private user and mount
//! namespaces: same path, so there is zero copy and no path rebasing, and tools
//! that walk up from the cwd still see one consistent tree. Writes (including
//! `.cook`) land in a scratch upper dir and vanish with it; the real tree is
//! never written.

use std::path::{Path, PathBuf};

/// Set (to the project root) in the isolated child; checked against the mount table.
pub const ROOT_ENV: &str = "COOK_SANDBOX_ROOT";
/// Set (to a per-run directory the parent removes, ^C included) in the isolated child.
pub const SCRATCH_ENV: &str = "COOK_SANDBOX_SCRATCH";

pub enum Isolation {
    /// This process launched the isolated child, which exited with this code.
    Parent(i32),
    /// This process is the isolated child.
    Inside,
}

/// The project root when this process runs inside an isolated view: [`ROOT_ENV`]
/// is set AND that path is an overlay mount point. The env var alone is not trusted.
pub fn isolated_root() -> Option<PathBuf> {
    let root = PathBuf::from(std::env::var_os(ROOT_ENV)?);
    let info = std::fs::read_to_string("/proc/self/mountinfo").ok()?;
    info.lines()
        .any(|line| {
            let Some((head, tail)) = line.split_once(" - ") else { return false };
            let mount_point = head.split(' ').nth(4).map(unescape_mountinfo);
            mount_point.as_deref() == Some(root.as_path().to_string_lossy().as_ref())
                && tail.split(' ').next() == Some("overlay")
        })
        .then_some(root)
}

/// The per-run scratch directory, inside an isolated view only: whatever an
/// isolated run writes outside the project (a throwaway store, trace files)
/// belongs here, where the parent removes it however the child ends.
pub fn scratch_dir() -> Option<PathBuf> {
    isolated_root()?;
    std::env::var_os(SCRATCH_ENV).map(PathBuf::from)
}

/// Decode mountinfo's `\040`-style octal escapes.
fn unescape_mountinfo(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let oct = b.get(i + 1..i + 4).filter(|d| b[i] == b'\\' && d.iter().all(|c| (b'0'..=b'7').contains(c)));
        if let Some(d) = oct {
            out.push(d.iter().fold(0u8, |a, c| a.wrapping_mul(8).wrapping_add(c - b'0')));
            i += 4;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Overlay mount options are `,`-separated and `:`-separated lists with `\\`
/// escapes; a path holding any of them cannot be passed safely.
fn mount_safe(path: &Path) -> Result<(), String> {
    let s = path.to_string_lossy();
    if s.contains([',', ':', '\\']) {
        return Err(format!("cannot isolate `{s}`: a ',', ':' or '\\' in the path breaks overlay mount options"));
    }
    Ok(())
}

/// Give every directory under `dir` u+rwx so the tree can be removed (overlayfs
/// makes `work/work` mode 000; build tools leave read-only dirs).
#[cfg(target_os = "linux")]
fn make_removable(dir: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let Ok(m) = std::fs::symlink_metadata(dir) else { return };
    if !m.is_dir() {
        return;
    }
    let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(m.permissions().mode() | 0o700));
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            make_removable(&e.path());
        }
    }
}

/// Hold an exclusive `flock` on `path` (created if missing), or `None` when
/// another process holds it. Released when the file is closed.
#[cfg(target_os = "linux")]
fn try_lock(path: &Path) -> Option<std::fs::File> {
    use std::os::fd::AsRawFd;
    let f = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(path).ok()?;
    (unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0).then_some(f)
}

/// Remove scratch dirs left by runs that died without cleaning up (SIGKILL,
/// a crash). Each live run holds `<dir>.lock`; a dir whose lock can be taken
/// has no owner. The lock is created before its dir, so a run being set up is
/// never swept.
#[cfg(target_os = "linux")]
fn sweep_stale(scratch_root: &Path) {
    let Ok(entries) = std::fs::read_dir(scratch_root) else { return };
    for e in entries.flatten() {
        let dir = e.path();
        let name = e.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with(".tmp") || name.ends_with(".lock") {
            continue;
        }
        let lock = scratch_root.join(format!("{name}.lock"));
        if let Some(_held) = try_lock(&lock) {
            make_removable(&dir);
            let _ = std::fs::remove_dir_all(&dir);
            let _ = std::fs::remove_file(&lock);
        }
    }
}

#[cfg(not(target_os = "linux"))]
pub fn isolate(project_root: &Path) -> Result<Isolation, String> {
    if isolated_root().as_deref() == Some(project_root) {
        return Ok(Isolation::Inside);
    }
    Err("isolation needs Linux user and mount namespaces".to_string())
}

/// Re-run this very command line inside the isolated view and report its exit
/// code, or recognise that we already are the isolated child.
#[cfg(target_os = "linux")]
pub fn isolate(project_root: &Path) -> Result<Isolation, String> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::process::CommandExt;

    let project_root = &project_root
        .canonicalize()
        .map_err(|e| format!("cannot isolate the project: {}: {e}", project_root.display()))?;
    if isolated_root().as_deref() == Some(project_root) {
        return Ok(Isolation::Inside);
    }
    if std::env::var_os(ROOT_ENV).is_some() {
        return Err(format!("{ROOT_ENV} is set but no overlay is mounted there; refusing to run unisolated"));
    }
    let root = project_root.to_string_lossy();

    let err = |what: &str, e: std::io::Error| format!("cannot isolate the project: {what}: {e}");
    let scratch_root = cook_cache::cloud_config::user_cache_root().join("sandbox");
    std::fs::create_dir_all(&scratch_root).map_err(|e| err("scratch", e))?;
    sweep_stale(&scratch_root);
    // The lock first, then the dir it guards (see `sweep_stale`).
    let lock = tempfile::Builder::new()
        .prefix(".tmp")
        .suffix(".lock")
        .tempfile_in(&scratch_root)
        .map_err(|e| err("scratch", e))?;
    let _held = try_lock(lock.path()).ok_or_else(|| "cannot isolate the project: scratch lock is taken".to_string())?;
    let scratch = scratch_root.join(lock.path().file_stem().unwrap_or_default());
    std::fs::create_dir(&scratch).map_err(|e| err("scratch", e))?;
    let (upper, work, run) = (scratch.join("upper"), scratch.join("work"), scratch.join("run"));
    for d in [&upper, &work, &run] {
        std::fs::create_dir(d).map_err(|e| err("scratch", e))?;
    }
    for p in [project_root, upper.as_path(), work.as_path()] {
        mount_safe(p)?;
    }
    let cwd = std::env::current_dir().map_err(|e| err("cwd", e))?;

    // Everything the child needs is built here: pre_exec must not allocate.
    let cstr = |b: &[u8]| CString::new(b).map_err(|_| "cannot isolate the project: path contains NUL".to_string());
    let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
    let root_c = cstr(root.as_bytes())?;
    let cwd_c = cstr(cwd.as_os_str().as_bytes())?;
    let opts = cstr(
        format!("lowerdir={root},upperdir={},workdir={},userxattr", upper.display(), work.display()).as_bytes(),
    )?;
    let uid_map = format!("{uid} {uid} 1");
    let gid_map = format!("{gid} {gid} 1");

    let mut cmd = std::process::Command::new("/proc/self/exe");
    cmd.arg0(std::env::args_os().next().unwrap_or_default())
        .args(std::env::args_os().skip(1))
        .env(ROOT_ENV, project_root)
        .env(SCRATCH_ENV, &run)
        // A unit that runs cook itself must not publish to the user's store
        // (its reads there are allowed: §17.9).
        .env(cook_cache::NO_PUBLISH_ENV, "1");
    let parent = unsafe { libc::getpid() };
    // SAFETY: the closure runs between fork and exec and only makes raw libc
    // calls on buffers built above (no allocation, no panics). It forks twice:
    // each child is single-threaded and glibc resets its atfork-guarded locks
    // in it, and the children only make the same raw calls before exec/_exit.
    unsafe {
        cmd.pre_exec(move || {
            fn check(r: libc::c_int) -> std::io::Result<()> {
                if r < 0 { Err(std::io::Error::last_os_error()) } else { Ok(()) }
            }
            fn write_file(path: &[u8], data: &[u8]) -> std::io::Result<()> {
                // `path` is a NUL-terminated literal.
                let fd = unsafe { libc::open(path.as_ptr().cast(), libc::O_WRONLY) };
                check(fd)?;
                let n = unsafe { libc::write(fd, data.as_ptr().cast(), data.len()) };
                let res = if n < 0 { Err(std::io::Error::last_os_error()) } else { Ok(()) };
                unsafe { libc::close(fd) };
                res
            }
            /// Pass `r` through, first saying on stderr which step failed: the
            /// parent only sees a bare errno.
            fn noted<T>(r: std::io::Result<T>, msg: &[u8]) -> std::io::Result<T> {
                if r.is_err() {
                    unsafe { libc::write(2, msg.as_ptr().cast(), msg.len()) };
                }
                r
            }
            fn code(status: libc::c_int) -> libc::c_int {
                if libc::WIFEXITED(status) { libc::WEXITSTATUS(status) } else { 128 + libc::WTERMSIG(status) }
            }
            /// Wait for `pid`, reaping every other child on the way (PID 1's
            /// job), and exit with `pid`'s code. Never returns.
            fn wait_and_exit(pid: libc::pid_t) -> ! {
                // Close the spawn error pipe (and all else): only the exec'd
                // grandchild may hold it, or the parent's spawn blocks.
                unsafe { libc::syscall(libc::SYS_close_range, 3u32, u32::MAX, 0u32) };
                let mut status = 0;
                loop {
                    let r = unsafe { libc::waitpid(-1, &mut status, 0) };
                    if r == pid {
                        unsafe { libc::_exit(code(status)) }
                    }
                    if r < 0 && std::io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
                        unsafe { libc::_exit(1) }
                    }
                }
            }
            // This relay must not outlive the parent (SIGKILL included), and
            // dies of TERM/HUP, which the parent forwards: its death takes
            // init, and with init the whole namespace, down with it.
            libc::signal(libc::SIGTERM, libc::SIG_DFL);
            libc::signal(libc::SIGHUP, libc::SIG_DFL);
            check(libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL))?;
            if libc::getppid() != parent {
                libc::_exit(1);
            }
            noted(
                check(libc::unshare(libc::CLONE_NEWUSER | libc::CLONE_NEWNS | libc::CLONE_NEWPID)),
                b"cook audit: creating user, mount and PID namespaces (unshare) failed; EPERM usually means unprivileged user namespaces are disabled (kernel.unprivileged_userns_clone, user.max_user_namespaces, or an AppArmor/SELinux restriction)\n",
            )?;
            noted(
                write_file(b"/proc/self/setgroups\0", b"deny")
                    .and_then(|()| write_file(b"/proc/self/uid_map\0", uid_map.as_bytes()))
                    .and_then(|()| write_file(b"/proc/self/gid_map\0", gid_map.as_bytes())),
                b"cook audit: writing the user namespace's uid_map/gid_map failed\n",
            )?;
            noted(
                check(libc::mount(
                    std::ptr::null(),
                    c"/".as_ptr(),
                    std::ptr::null(),
                    libc::MS_REC | libc::MS_PRIVATE,
                    std::ptr::null(),
                )),
                b"cook audit: making the namespace's mounts private failed\n",
            )?;
            noted(
                check(libc::mount(
                    c"overlay".as_ptr(),
                    root_c.as_ptr(),
                    c"overlay".as_ptr(),
                    0,
                    opts.as_ptr().cast(),
                )),
                b"cook audit: mounting the overlay over the project failed (overlayfs in a user namespace needs Linux 5.11+ and a filesystem it can stack on)\n",
            )?;
            noted(
                check(libc::chdir(cwd_c.as_ptr())),
                b"cook audit: entering the working directory inside the overlay (chdir) failed\n",
            )?;
            // A PID namespace: its first process is its init, and when init
            // exits the kernel kills every process left in it, so nothing a
            // unit starts (a daemon, a watcher) outlives the audit. This
            // process stays outside and only relays the exit code.
            let init = libc::fork();
            check(init)?;
            if init > 0 {
                wait_and_exit(init);
            }
            // init dies with the relay (its exit kills the namespace).
            // shortcut: a relay killed between this fork and the prctl leaves init orphaned; close it with a pidfd if it is ever seen.
            check(libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL))?;
            // init: a /proc of its own, so `/proc/<pid>` (strace's fd paths)
            // names this namespace's processes.
            noted(
                check(libc::mount(
                    c"proc".as_ptr(),
                    c"/proc".as_ptr(),
                    c"proc".as_ptr(),
                    libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC,
                    std::ptr::null(),
                )),
                b"cook audit: mounting /proc for the PID namespace failed (is /proc masked, as in a container?)\n",
            )?;
            let cook = libc::fork();
            check(cook)?;
            if cook > 0 {
                wait_and_exit(cook);
            }
            // Ignored dispositions survive exec: give cook the defaults back
            // (the parent and both relays ignore SIGINT while they wait).
            libc::signal(libc::SIGINT, libc::SIG_DFL);
            libc::signal(libc::SIGTERM, libc::SIG_DFL);
            libc::signal(libc::SIGHUP, libc::SIG_DFL);
            Ok(())
        });
    }
    // The parent outlives ^C / SIGTERM / SIGHUP just long enough to clean the
    // scratch dir: ^C reaches the whole process group; TERM and HUP (a CI
    // timeout signals this process alone) are forwarded to the relay.
    use std::sync::atomic::{AtomicI32, Ordering::SeqCst};
    static CHILD: AtomicI32 = AtomicI32::new(0);
    static PENDING: AtomicI32 = AtomicI32::new(0);
    extern "C" fn forward(sig: libc::c_int) {
        match CHILD.load(SeqCst) {
            0 => PENDING.store(sig, SeqCst),
            pid => unsafe {
                libc::kill(pid, sig);
            },
        }
    }
    let forward = forward as extern "C" fn(libc::c_int) as libc::sighandler_t;
    let (old_int, old_term, old_hup) = unsafe {
        (
            libc::signal(libc::SIGINT, libc::SIG_IGN),
            libc::signal(libc::SIGTERM, forward),
            libc::signal(libc::SIGHUP, forward),
        )
    };
    let status = cmd.spawn().and_then(|mut child| {
        let pid = child.id() as i32;
        CHILD.store(pid, SeqCst);
        if let sig @ 1.. = PENDING.swap(0, SeqCst) {
            unsafe { libc::kill(pid, sig) };
        }
        let waited = child.wait();
        CHILD.store(0, SeqCst); // before the pid can be reused
        waited
    });
    let status = status.map_err(|e| format!("cannot isolate the project: {e}"));
    unsafe {
        libc::signal(libc::SIGINT, old_int);
        libc::signal(libc::SIGTERM, old_term);
        libc::signal(libc::SIGHUP, old_hup);
    }

    make_removable(&scratch);
    if let Err(e) = std::fs::remove_dir_all(&scratch) {
        tracing::warn!("sandbox scratch not removed: {e}");
    }
    drop(lock);
    let status = status?;
    use std::os::unix::process::ExitStatusExt;
    Ok(Isolation::Parent(status.code().or(status.signal().map(|s| 128 + s)).unwrap_or(1)))
}

#[cfg(test)]
#[path = "tests/sandbox_tests.rs"]
mod tests;
