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
