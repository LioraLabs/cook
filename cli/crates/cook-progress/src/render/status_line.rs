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
    /// Whether the tick thread may currently be painting the line — i.e.
    /// whether an event writer needs to clear before printing its own line.
    pub fn is_visible(&self) -> bool { self.visible.load(Ordering::Relaxed) }

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
        write!(out, "{}{line}", crate::render::CLEAR_LINE)?;
        out.flush()
    }
    fn clear_line(&mut self) -> io::Result<()> {
        let mut out = io::stderr().lock();
        write!(out, "{}", crate::render::CLEAR_LINE)?;
        out.flush()
    }
}

#[cfg(test)]
#[path = "tests/status_line_tests.rs"]
mod tests;
