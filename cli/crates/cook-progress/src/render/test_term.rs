//! In-memory `TermLike` for snapshot testing indicatif output.

use std::sync::{Arc, Mutex};

use indicatif::TermLike;

#[derive(Clone, Debug)]
pub struct TestTerm {
    inner: Arc<Mutex<Inner>>,
}

#[derive(Debug)]
struct Inner {
    width: u16,
    height: u16,
    buffer: String,
}

impl TestTerm {
    pub fn new(width: u16) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                width,
                height: 40,
                buffer: String::new(),
            })),
        }
    }

    pub fn contents(&self) -> String {
        self.inner.lock().unwrap().buffer.clone()
    }
}

impl TermLike for TestTerm {
    fn width(&self) -> u16 {
        self.inner.lock().unwrap().width
    }
    fn height(&self) -> u16 {
        self.inner.lock().unwrap().height
    }
    fn move_cursor_up(&self, _n: usize) -> std::io::Result<()> {
        Ok(())
    }
    fn move_cursor_down(&self, _n: usize) -> std::io::Result<()> {
        Ok(())
    }
    fn move_cursor_right(&self, _n: usize) -> std::io::Result<()> {
        Ok(())
    }
    fn move_cursor_left(&self, _n: usize) -> std::io::Result<()> {
        Ok(())
    }
    fn write_line(&self, line: &str) -> std::io::Result<()> {
        let mut g = self.inner.lock().unwrap();
        g.buffer.push_str(line);
        g.buffer.push('\n');
        Ok(())
    }
    fn write_str(&self, s: &str) -> std::io::Result<()> {
        let mut g = self.inner.lock().unwrap();
        g.buffer.push_str(s);
        Ok(())
    }
    fn clear_line(&self) -> std::io::Result<()> {
        Ok(())
    }
    fn flush(&self) -> std::io::Result<()> {
        Ok(())
    }
}
