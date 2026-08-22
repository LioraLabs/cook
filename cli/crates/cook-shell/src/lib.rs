//! Runs a shell command and reports what it did.
//!
//! That is the whole crate. It exists because the same twenty lines were
//! written six times across three crates and the copies disagreed: one dropped
//! stderr on success, one reported only an exit code, one ordered stderr ahead
//! of stdout, and only two of the six built a [`CommandFailure`] the same way.
//! §{lua.cook-sh} says what `cook.sh` means; this is the one place that means
//! it (CS-0188).
//!
//! # What it does not do
//!
//! * **It does not decide what a command is.** Callers pass command text they
//!   have already resolved — sigils substituted, probe values interpolated,
//!   shell blocks joined.
//! * **It does not own the caller's caches.** `cook-fingerprint`'s stat memo
//!   must be disarmed before a command that may write anywhere in the tree
//!   (COOK-306), but that memo belongs to the execute phase and not to every
//!   caller: the register phase does not disarm, deliberately. Making it this
//!   crate's business would also point `cook-shell` at `cook-fingerprint`,
//!   which is the wrong direction. Callers disarm.
//! * **It does not report.** No printing, no events, no progress. It returns an
//!   [`Outcome`] and the caller decides what to say about it.
//!
//! # Why there is no timeout
//!
//! There was one, on the test path, and it never fired: CS-0135 removed the
//! `timeout` modifier, so the field arrived hardcoded at `u64::MAX` and the
//! kill loop that read it was unreachable. The loop is what forced that path to
//! drain both pipes on its own threads rather than using `Command::output()`,
//! which is the only reason the test spawn looked different from the other
//! five. With the timeout gone the difference goes too. A timeout belongs here
//! the day a caller can actually set one, and not before.

use std::path::Path;
use std::process::{Child, Command, Stdio as ProcessStdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cook_contracts::{CapturedStream, CommandFailure, OutputChunk, OutputStream};

/// What the child does with its standard streams.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stdio {
    /// Both streams are piped and captured. Every unit-producing spawn wants
    /// this: the bytes become the unit's output, attributed to it (CS-0188).
    Captured,
    /// Both streams are inherited from this process. The interactive path wants
    /// this and only this: an interactive command owns the terminal, so its
    /// output must reach the user's tty as it happens rather than arriving as a
    /// buffer afterwards. Nothing is captured, and [`Outcome::chunks`] is empty.
    Inherited,
}

/// One shell command to run.
#[derive(Debug, Clone)]
pub struct Spawn<'a> {
    /// Command text, passed to `/bin/sh -c` verbatim. A multi-line shell block
    /// arrives here as one string under a `set -e` preamble, and the shell does
    /// the sequencing: N lines is still one spawn.
    pub command: &'a str,
    /// Directory the child runs in.
    pub working_dir: &'a Path,
    pub stdio: Stdio,
}

/// The command could not be started at all. Distinct from a command that ran
/// and failed, which is a successful [`run`] returning an unsuccessful
/// [`Outcome`].
#[derive(Debug)]
pub struct SpawnError {
    message: String,
}

impl SpawnError {
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl std::fmt::Display for SpawnError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for SpawnError {}

/// A process group held open for one chore body.
///
/// The anchor keeps the group alive between shell steps, so a background child
/// from an earlier step remains owned when a later step runs.
#[derive(Clone)]
pub struct ProcessGroup {
    inner: Arc<ProcessGroupInner>,
}

struct ProcessGroupInner {
    pgid: i32,
    anchor: Mutex<Option<Child>>,
    drained: AtomicBool,
    cleanup: Mutex<()>,
    monitor: Mutex<Option<SignalMonitor>>,
}

struct SignalMonitor {
    handle: signal_hook::iterator::Handle,
    thread: std::thread::JoinHandle<()>,
}

const GRACE: Duration = Duration::from_millis(100);

impl ProcessGroup {
    /// Establish an empty group whose anchor survives until [`Self::drain`].
    pub fn new() -> Result<Self, SpawnError> {
        #[cfg(not(unix))]
        {
            return Err(SpawnError {
                message: "chore process ownership requires Unix process groups".into(),
            });
        }

        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;

            let mut command = Command::new("sleep");
            command
                .arg("2147483647")
                .stdin(ProcessStdio::null())
                .stdout(ProcessStdio::null())
                .stderr(ProcessStdio::null());
            unsafe {
                command.pre_exec(|| {
                    if libc::setpgid(0, 0) == -1 {
                        Err(std::io::Error::last_os_error())
                    } else {
                        Ok(())
                    }
                });
            }
            let anchor = command.spawn().map_err(|e| SpawnError {
                message: format!("failed to establish chore process group: {e}"),
            })?;
            let inner = Arc::new(ProcessGroupInner {
                pgid: anchor.id() as i32,
                anchor: Mutex::new(Some(anchor)),
                drained: AtomicBool::new(false),
                cleanup: Mutex::new(()),
                monitor: Mutex::new(None),
            });
            let mut signals = signal_hook::iterator::Signals::new([libc::SIGINT]).map_err(|e| {
                let _ = drain_inner(&inner, libc::SIGTERM);
                SpawnError {
                    message: format!("failed to monitor Ctrl-C for chore process group: {e}"),
                }
            })?;
            let handle = signals.handle();
            let watched = Arc::downgrade(&inner);
            let thread = std::thread::spawn(move || {
                for _ in signals.forever() {
                    let Some(watched) = watched.upgrade() else {
                        break;
                    };
                    let _ = drain_inner(&watched, libc::SIGINT);
                    let _ = signal_hook::low_level::emulate_default_handler(libc::SIGINT);
                }
            });
            *inner.monitor.lock().expect("process-group monitor lock") =
                Some(SignalMonitor { handle, thread });
            Ok(Self { inner })
        }
    }

    fn configure(&self, command: &mut Command) -> Result<(), SpawnError> {
        #[cfg(not(unix))]
        {
            let _ = command;
            return Err(SpawnError {
                message: "chore process ownership requires Unix process groups".into(),
            });
        }

        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;

            let pgid = self.inner.pgid;
            unsafe {
                command.pre_exec(move || {
                    if libc::setpgid(0, pgid) == -1 {
                        Err(std::io::Error::last_os_error())
                    } else {
                        Ok(())
                    }
                });
            }
            Ok(())
        }
    }

    /// Terminate every member, waiting briefly before escalating to SIGKILL.
    pub fn drain(&self) -> Result<(), SpawnError> {
        let result = drain_inner(&self.inner, libc::SIGTERM);
        if let Some(SignalMonitor { handle, thread }) = self
            .inner
            .monitor
            .lock()
            .expect("process-group monitor lock")
            .take()
        {
            handle.close();
            let _ = thread.join();
            drop(handle);
            restore_sigint_default();
        }
        result
    }
}

impl Drop for ProcessGroup {
    fn drop(&mut self) {
        if Arc::strong_count(&self.inner) == 1 {
            let _ = self.drain();
        }
    }
}

fn drain_inner(inner: &ProcessGroupInner, first_signal: i32) -> Result<(), SpawnError> {
    let _cleanup = inner.cleanup.lock().expect("process-group cleanup lock");
    if inner.drained.load(Ordering::Acquire) {
        return Ok(());
    }

    signal_group(inner.pgid, first_signal)?;
    reap_anchor(inner)?;
    if first_signal == libc::SIGINT && wait_for_group(inner.pgid, GRACE)? {
        signal_group(inner.pgid, libc::SIGTERM)?;
    }
    if wait_for_group(inner.pgid, GRACE)? {
        signal_group(inner.pgid, libc::SIGKILL)?;
        if wait_for_group(inner.pgid, Duration::from_secs(1))? {
            return Err(SpawnError {
                message: "failed to drain chore process group".into(),
            });
        }
    }
    inner.drained.store(true, Ordering::Release);
    Ok(())
}

fn reap_anchor(inner: &ProcessGroupInner) -> Result<(), SpawnError> {
    if let Some(mut anchor) = inner
        .anchor
        .lock()
        .expect("process-group anchor lock")
        .take()
    {
        anchor.wait().map_err(|e| SpawnError {
            message: format!("failed to reap chore process-group anchor: {e}"),
        })?;
    }
    Ok(())
}

fn restore_sigint_default() {
    #[cfg(unix)]
    // The monitor has already stopped and dropped its signal-hook action; this
    // is ordinary teardown, not signal-handler work.
    unsafe {
        libc::signal(libc::SIGINT, libc::SIG_DFL);
    }
}

fn signal_group(pgid: i32, signal: i32) -> Result<(), SpawnError> {
    #[cfg(unix)]
    if unsafe { libc::kill(-pgid, signal) } == -1 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::ESRCH) {
            return Err(SpawnError {
                message: format!("failed to signal chore process group: {error}"),
            });
        }
    }
    Ok(())
}

fn wait_for_group(pgid: i32, grace: Duration) -> Result<bool, SpawnError> {
    let deadline = Instant::now() + grace;
    loop {
        if !group_is_live(pgid)? {
            return Ok(false);
        }
        if Instant::now() >= deadline {
            return Ok(true);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn group_is_live(pgid: i32) -> Result<bool, SpawnError> {
    #[cfg(target_os = "linux")]
    {
        for entry in std::fs::read_dir("/proc").map_err(|e| SpawnError {
            message: format!("failed to inspect chore process group: {e}"),
        })? {
            let entry = entry.map_err(|e| SpawnError {
                message: format!("failed to inspect chore process group: {e}"),
            })?;
            let Some(pid) = entry
                .file_name()
                .to_str()
                .and_then(|name| name.parse::<u32>().ok())
            else {
                continue;
            };
            let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
                continue;
            };
            let Some((_, fields)) = stat.rsplit_once(") ") else {
                continue;
            };
            let mut fields = fields.split_whitespace();
            let state = fields.next();
            let _ppid = fields.next();
            let group = fields.next().and_then(|field| field.parse::<i32>().ok());
            if group == Some(pgid) && state != Some("Z") {
                return Ok(true);
            }
        }
        return Ok(false);
    }

    #[cfg(all(unix, not(target_os = "linux")))]
    {
        if unsafe { libc::kill(-pgid, 0) } == -1 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ESRCH) {
                return Ok(false);
            }
            return Err(SpawnError {
                message: format!("failed to inspect chore process group: {error}"),
            });
        }
        return Ok(true);
    }

    #[cfg(not(unix))]
    Ok(false)
}

/// What running the command was observed to do.
#[derive(Debug, Clone)]
pub struct Outcome {
    chunks: Vec<OutputChunk>,
    exit_code: Option<i32>,
    success: bool,
    duration: Duration,
}

impl Outcome {
    /// The command's output, in the order the streams were read.
    ///
    /// One spawn contributes at most one chunk per stream, because
    /// `Command::output()` buffers the two separately and their true
    /// interleaving is not recoverable. CS-0188 states that limit normatively
    /// rather than papering over it: a *sequence* of spawns preserves its
    /// order, a single spawn's two streams do not interleave.
    ///
    /// Stdout precedes stderr when both are present. That order is arbitrary
    /// and means nothing; it is fixed only so two runs of the same command
    /// produce the same sequence.
    pub fn chunks(&self) -> &[OutputChunk] {
        &self.chunks
    }

    pub fn into_chunks(self) -> Vec<OutputChunk> {
        self.chunks
    }

    pub fn success(&self) -> bool {
        self.success
    }

    /// `None` when the child was terminated by a signal.
    pub fn exit_code(&self) -> Option<i32> {
        self.exit_code
    }

    pub fn duration(&self) -> Duration {
        self.duration
    }

    /// The captured stdout as text, which is what `cook.sh` returns to Lua
    /// (§{lua.cook-sh}). Lossy, because this is a render.
    pub fn stdout_lossy(&self) -> String {
        self.stream_lossy(OutputStream::Stdout)
    }

    /// The captured stderr as text. Lossy, for the same reason.
    pub fn stderr_lossy(&self) -> String {
        self.stream_lossy(OutputStream::Stderr)
    }

    fn stream_lossy(&self, want: OutputStream) -> String {
        let mut out = String::new();
        for c in self.chunks.iter().filter(|c| c.stream() == want) {
            out.push_str(&c.lossy());
        }
        out
    }

    /// The failure this outcome represents, or `None` if the command succeeded.
    ///
    /// The single place a [`CommandFailure`] is built from a spawn. Four call
    /// sites used to build one each, which is how the formatting fix that
    /// opened this milestone reached one twin and not the other.
    pub fn failure(&self, line: usize, command: &str) -> Option<CommandFailure> {
        if self.success {
            return None;
        }
        Some(CommandFailure::new(
            line,
            self.exit_code.unwrap_or(1),
            command,
            CapturedStream::from_bytes(self.stream_lossy(OutputStream::Stdout).as_bytes()),
            CapturedStream::from_bytes(self.stream_lossy(OutputStream::Stderr).as_bytes()),
        ))
    }
}

/// Run `spawn.command` through `/bin/sh -c`, overlaying `env_overlay` onto this
/// process's environment.
///
/// The overlay is an iterator of pairs rather than a concrete map so the five
/// callers holding a `HashMap` and the one holding a `BTreeMap` can each pass
/// what they already have.
pub fn run<K, V>(
    spawn: &Spawn<'_>,
    env_overlay: impl IntoIterator<Item = (K, V)>,
) -> Result<Outcome, SpawnError>
where
    K: AsRef<str>,
    V: AsRef<str>,
{
    run_with_group(spawn, env_overlay, None)
}

/// Run a command, optionally joining a chore's process group.
pub fn run_with_group<K, V>(
    spawn: &Spawn<'_>,
    env_overlay: impl IntoIterator<Item = (K, V)>,
    process_group: Option<&ProcessGroup>,
) -> Result<Outcome, SpawnError>
where
    K: AsRef<str>,
    V: AsRef<str>,
{
    let mut cmd = std::process::Command::new("/bin/sh");
    cmd.arg("-c")
        .arg(spawn.command)
        .current_dir(spawn.working_dir);
    if let Some(process_group) = process_group {
        process_group.configure(&mut cmd)?;
    }
    for (k, v) in env_overlay {
        cmd.env(k.as_ref(), v.as_ref());
    }

    let start = Instant::now();
    match spawn.stdio {
        Stdio::Inherited => {
            let status = cmd.status().map_err(|e| SpawnError {
                message: format!("failed to execute: {e}"),
            })?;
            Ok(Outcome {
                chunks: Vec::new(),
                exit_code: status.code(),
                success: status.success(),
                duration: start.elapsed(),
            })
        }
        Stdio::Captured => {
            let out = cmd.output().map_err(|e| SpawnError {
                message: format!("failed to execute: {e}"),
            })?;
            let mut chunks = Vec::new();
            chunks.extend(OutputChunk::new(OutputStream::Stdout, out.stdout));
            chunks.extend(OutputChunk::new(OutputStream::Stderr, out.stderr));
            Ok(Outcome {
                chunks,
                exit_code: out.status.code(),
                success: out.status.success(),
                duration: start.elapsed(),
            })
        }
    }
}

#[cfg(test)]
#[path = "tests/shell_tests.rs"]
mod tests;
