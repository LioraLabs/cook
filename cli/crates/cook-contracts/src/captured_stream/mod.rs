const STREAM_CAP: usize = 64 * 1024;
const STREAM_HEAD: usize = 16 * 1024;

/// Captured command output rendered lossily within a bounded source-byte budget.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedStream {
    rendered: String,
}

impl CapturedStream {
    pub fn from_bytes(bytes: &[u8]) -> Self {
        if bytes.len() <= STREAM_CAP {
            return Self {
                rendered: String::from_utf8_lossy(bytes).into_owned(),
            };
        }

        let flat_head_end = STREAM_HEAD;
        let flat_tail_start = bytes.len() - (STREAM_CAP - STREAM_HEAD);
        let snapped_head = bytes[..flat_head_end]
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map_or(0, |newline| newline + 1);
        let head_end = if snapped_head > 0 {
            snapped_head
        } else {
            flat_head_end
        };
        let snapped_tail = bytes[flat_tail_start..]
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(bytes.len(), |offset| flat_tail_start + offset + 1);
        let tail_start = if snapped_tail > head_end && snapped_tail < bytes.len() {
            snapped_tail
        } else {
            flat_tail_start.max(head_end)
        };

        let mut rendered = String::from_utf8_lossy(&bytes[..head_end]).into_owned();
        if !rendered.ends_with('\n') {
            rendered.push('\n');
        }
        rendered.push_str(&format!(
            "... ({} bytes elided; showing the first {} and last {} bytes) ...\n",
            tail_start - head_end,
            head_end,
            bytes.len() - tail_start
        ));
        rendered.push_str(&String::from_utf8_lossy(&bytes[tail_start..]));
        Self { rendered }
    }

    pub fn as_str(&self) -> &str {
        &self.rendered
    }

    pub fn is_empty(&self) -> bool {
        self.rendered.is_empty()
    }
}

#[cfg(test)]
#[path = "tests/captured_stream_tests.rs"]
mod tests;
