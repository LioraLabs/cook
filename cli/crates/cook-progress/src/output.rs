#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BufferState {
    Normal,
    Error,
}

#[derive(Debug, Clone)]
pub struct OutputBuffer {
    lines: Vec<String>,
    state: BufferState,
}

impl Default for OutputBuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl OutputBuffer {
    pub fn new() -> Self {
        Self {
            lines: Vec::new(),
            state: BufferState::Normal,
        }
    }

    pub fn push(&mut self, line: impl Into<String>) {
        self.lines.push(line.into());
    }

    pub fn visible_lines(&self, max_lines: usize) -> Vec<&str> {
        if self.state == BufferState::Error {
            self.lines.iter().map(|s| s.as_str()).collect()
        } else if self.lines.len() <= max_lines {
            self.lines.iter().map(|s| s.as_str()).collect()
        } else {
            self.lines[self.lines.len() - max_lines..]
                .iter()
                .map(|s| s.as_str())
                .collect()
        }
    }

    pub fn set_error(&mut self) {
        self.state = BufferState::Error;
    }

    pub fn is_error(&self) -> bool {
        self.state == BufferState::Error
    }

    pub fn clear(&mut self) {
        self.lines.clear();
        self.state = BufferState::Normal;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_buffer_returns_no_lines() {
        let buf = OutputBuffer::new();
        assert!(buf.visible_lines(3).is_empty());
        assert!(!buf.is_error());
    }

    #[test]
    fn buffer_grows_incrementally() {
        let mut buf = OutputBuffer::new();
        buf.push("line 1");
        assert_eq!(buf.visible_lines(3), vec!["line 1"]);
        buf.push("line 2");
        assert_eq!(buf.visible_lines(3), vec!["line 1", "line 2"]);
        buf.push("line 3");
        assert_eq!(buf.visible_lines(3), vec!["line 1", "line 2", "line 3"]);
    }

    #[test]
    fn buffer_rolls_at_max() {
        let mut buf = OutputBuffer::new();
        buf.push("line 1");
        buf.push("line 2");
        buf.push("line 3");
        buf.push("line 4");
        assert_eq!(buf.visible_lines(3), vec!["line 2", "line 3", "line 4"]);
    }

    #[test]
    fn error_shows_all_lines() {
        let mut buf = OutputBuffer::new();
        buf.push("line 1");
        buf.push("line 2");
        buf.push("line 3");
        buf.push("line 4");
        buf.set_error();
        assert_eq!(buf.visible_lines(2), vec!["line 1", "line 2", "line 3", "line 4"]);
        assert!(buf.is_error());
    }

    #[test]
    fn clear_resets_buffer() {
        let mut buf = OutputBuffer::new();
        buf.push("line 1");
        buf.set_error();
        buf.clear();
        assert!(buf.visible_lines(3).is_empty());
        assert!(!buf.is_error());
    }
}
