//! Sticky bottom-of-terminal status line — threading + I/O.

use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use arc_swap::ArcSwap;

use crate::render::snapshot::{render_status_line, StatusLineOptions, StatusSnapshot};

/// Tick interval for the status-line repaint thread. ~10 Hz.
const TICK_INTERVAL: Duration = Duration::from_millis(100);

/// Terminal-width fallback when `terminal_size` is unavailable.
const FALLBACK_COLS: usize = 80;

/// Resolves terminal width on each tick. Falls back to 80 if unavailable.
fn detect_cols() -> usize {
    terminal_size::terminal_size()
        .map(|(w, _)| w.0 as usize)
        .unwrap_or(FALLBACK_COLS)
}

/// Public handle to the sticky status line. Drops cleanly via `shutdown()`.
pub struct StatusLine {
    snapshot: Arc<ArcSwap<StatusSnapshot>>,
    visible: Arc<AtomicBool>,
    shutdown: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl StatusLine {
    /// Spawn the tick thread. The line is initially hidden — call `show()`
    /// once you have a meaningful snapshot.
    pub fn spawn(opts: StatusLineOptions, initial: StatusSnapshot) -> Self {
        Self::spawn_with_writer::<TermStderr>(opts, initial, TermStderr)
    }

    /// Test entry point — inject a writer.
    pub(crate) fn spawn_with_writer<W: StatusWriter + 'static>(
        opts: StatusLineOptions,
        initial: StatusSnapshot,
        mut writer: W,
    ) -> Self {
        let snapshot = Arc::new(ArcSwap::from_pointee(initial));
        let visible = Arc::new(AtomicBool::new(false));
        let shutdown = Arc::new(AtomicBool::new(false));

        let snap = Arc::clone(&snapshot);
        let vis = Arc::clone(&visible);
        let halt = Arc::clone(&shutdown);
        let thread = thread::spawn(move || {
            loop {
                thread::sleep(TICK_INTERVAL);
                if halt.load(Ordering::Relaxed) { break; }
                if !vis.load(Ordering::Relaxed) { continue; }
                let s = snap.load();
                let line = render_status_line(&*s, opts, detect_cols());
                if line.is_empty() {
                    let _ = writer.clear_line();
                    continue;
                }
                let _ = writer.write_status_line(&line);
            }
            // On shutdown, leave a clean trailing line.
            let _ = writer.clear_line();
        });

        Self { snapshot, visible, shutdown, thread: Some(thread) }
    }

    pub fn update(&self, snap: StatusSnapshot) {
        self.snapshot.store(Arc::new(snap));
    }
    pub fn show(&self) { self.visible.store(true, Ordering::Relaxed); }
    pub fn hide(&self) { self.visible.store(false, Ordering::Relaxed); }

    pub fn shutdown(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for StatusLine {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// I/O surface; default impl writes to stderr. Tests provide a capture buffer.
pub trait StatusWriter: Send {
    fn write_status_line(&mut self, line: &str) -> io::Result<()>;
    fn clear_line(&mut self) -> io::Result<()>;
}

pub struct TermStderr;

impl StatusWriter for TermStderr {
    fn write_status_line(&mut self, line: &str) -> io::Result<()> {
        let mut out = io::stderr().lock();
        write!(out, "\r\x1b[2K{line}")?;
        out.flush()
    }
    fn clear_line(&mut self) -> io::Result<()> {
        let mut out = io::stderr().lock();
        write!(out, "\r\x1b[2K")?;
        out.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::time::Instant;

    use crate::render::snapshot::RunningEntry;

    #[derive(Default, Clone)]
    struct CaptureWriter {
        buf: Arc<Mutex<Vec<String>>>,
    }

    impl StatusWriter for CaptureWriter {
        fn write_status_line(&mut self, line: &str) -> io::Result<()> {
            self.buf.lock().unwrap().push(format!("LINE:{line}"));
            Ok(())
        }
        fn clear_line(&mut self) -> io::Result<()> {
            self.buf.lock().unwrap().push("CLEAR".into());
            Ok(())
        }
    }

    fn snap(total: usize, done: usize) -> StatusSnapshot {
        StatusSnapshot {
            total_nodes: total,
            done_nodes: done,
            running: vec![RunningEntry { started_at: Instant::now(), display: "x.o".into() }],
            started_at: Instant::now() - Duration::from_secs(1),
        }
    }

    #[test]
    fn hidden_status_line_does_not_write_lines() {
        let writer = CaptureWriter::default();
        let buf = writer.buf.clone();
        let mut s = StatusLine::spawn_with_writer(
            StatusLineOptions { colored: false, ..Default::default() },
            snap(47, 0),
            writer,
        );
        thread::sleep(Duration::from_millis(250));
        s.shutdown();
        let out = buf.lock().unwrap();
        assert!(!out.iter().any(|l| l.starts_with("LINE:")), "got: {:?}", *out);
    }

    #[test]
    fn show_then_hide_writes_then_stops() {
        let writer = CaptureWriter::default();
        let buf = writer.buf.clone();
        let mut s = StatusLine::spawn_with_writer(
            StatusLineOptions { colored: false, ..Default::default() },
            snap(47, 0),
            writer,
        );
        s.show();
        thread::sleep(Duration::from_millis(250));
        let count_after_show = buf.lock().unwrap().iter().filter(|l| l.starts_with("LINE:")).count();
        assert!(count_after_show >= 1, "expected at least 1 paint, got {count_after_show}");
        s.hide();
        thread::sleep(Duration::from_millis(250));
        let count_after_hide = buf.lock().unwrap().iter().filter(|l| l.starts_with("LINE:")).count();
        s.shutdown();
        // After hide, no further LINE: writes (allow ~1 in-flight tick = +1).
        assert!(count_after_hide <= count_after_show + 1,
            "expected LINE count not to grow after hide; before={count_after_show} after={count_after_hide}");
    }
}
