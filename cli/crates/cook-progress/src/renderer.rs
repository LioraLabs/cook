use std::collections::BTreeMap;
use std::fmt::Write as FmtWrite;
use std::io::{self, Write};
use std::time::Duration;

use crossterm::style::{Attribute, Color, ResetColor, SetAttribute, SetForegroundColor};

use crate::bar;
use crate::frame::{Footer, Frame, ItemStatus, Section, Status};
use crate::output::OutputBuffer;
use crate::symbols::Symbols;

pub struct RenderConfig {
    pub width: u16,
    pub max_output_lines: usize,
    pub symbols: Symbols,
    pub colors: bool,
}

impl Default for RenderConfig {
    fn default() -> Self {
        Self {
            width: 80,
            max_output_lines: 3,
            symbols: Symbols::default(),
            colors: true,
        }
    }
}

pub struct Renderer {
    last_frame_height: usize,
    output_buffers: BTreeMap<String, OutputBuffer>,
    config: RenderConfig,
}

impl Renderer {
    pub fn new(config: RenderConfig) -> Self {
        Self {
            last_frame_height: 0,
            output_buffers: BTreeMap::new(),
            config,
        }
    }

    pub fn last_frame_height(&self) -> usize {
        self.last_frame_height
    }

    pub fn push_output(&mut self, section_id: &str, line: &str) {
        self.output_buffers
            .entry(section_id.to_string())
            .or_default()
            .push(line);
    }

    pub fn set_error(&mut self, section_id: &str) {
        self.output_buffers
            .entry(section_id.to_string())
            .or_default()
            .set_error();
    }

    pub fn set_width(&mut self, width: u16) {
        self.config.width = width;
    }

    pub fn visible_lines(&self, section_id: &str) -> Vec<&str> {
        match self.output_buffers.get(section_id) {
            Some(buf) => buf.visible_lines(self.config.max_output_lines),
            None => vec![],
        }
    }

    pub fn reset(&mut self) {
        self.output_buffers.clear();
        self.last_frame_height = 0;
    }

    /// Erase the previously rendered frame by moving the cursor up and clearing each line.
    pub fn clear_last_frame<W: Write>(&self, out: &mut W) -> io::Result<()> {
        for _ in 0..self.last_frame_height {
            write!(out, "\x1b[1A\x1b[2K")?;
        }
        Ok(())
    }

    pub fn render_frame<W: Write>(&mut self, frame: &Frame, out: &mut W) -> io::Result<()> {
        let mut line_count = 0usize;

        for section in &frame.sections {
            let lines = self.render_section(section);
            for line in &lines {
                writeln!(out, "{line}")?;
                line_count += 1;
            }
        }

        if let Some(footer) = &frame.footer {
            let lines = self.render_footer(footer);
            for line in &lines {
                writeln!(out, "{line}")?;
                line_count += 1;
            }
        }

        self.last_frame_height = line_count;
        Ok(())
    }

    // ── Section dispatch ────────────────────────────────────────────────────

    fn render_section(&self, section: &Section) -> Vec<String> {
        match section.status {
            Status::Waiting => self.render_waiting(section),
            Status::Running => self.render_running(section),
            Status::Completed => self.render_completed(section),
            Status::Cached => self.render_cached(section),
            Status::Failed => self.render_failed(section),
        }
    }

    // ── Status renderers ────────────────────────────────────────────────────

    fn render_waiting(&self, section: &Section) -> Vec<String> {
        let sym = self.config.symbols.waiting;
        let label = &section.label;
        let line = if self.config.colors {
            format!(
                "{}{sym} {label}{}",
                SetForegroundColor(Color::DarkGrey),
                ResetColor
            )
        } else {
            format!("{sym} {label}")
        };
        vec![line]
    }

    fn render_running(&self, section: &Section) -> Vec<String> {
        let sym = self.config.symbols.running;
        let label = &section.label;
        let (completed, total) = section.progress.unwrap_or((0, 0));
        let elapsed_str = section
            .elapsed
            .map(Self::format_duration)
            .unwrap_or_default();
        let counter = format!("{completed}/{total}");
        let elapsed_part = if elapsed_str.is_empty() {
            String::new()
        } else {
            format!(" · {elapsed_str}")
        };

        let bw = self.bar_width(label, completed, total, &elapsed_str);
        let bar_str = bar::render_bar(bw, completed, total, self.config.colors);

        let header = if self.config.colors {
            let mut s = String::new();
            let _ = write!(
                s,
                "{}{sym}{} {}{label}{} {bar_str} {counter}{elapsed_part}",
                SetForegroundColor(Color::Blue),
                ResetColor,
                SetAttribute(Attribute::Bold),
                SetAttribute(Attribute::Reset),
            );
            s
        } else {
            format!("{sym} {label} {bar_str} {counter}{elapsed_part}")
        };

        let mut lines = vec![header];

        // Active items (all on one line)
        if !section.active_items.is_empty() {
            let item_line = if self.config.colors {
                let mut s = String::from("  ");
                for (i, item) in section.active_items.iter().enumerate() {
                    if i > 0 {
                        s.push_str("  ");
                    }
                    let (item_sym, item_color) = self.item_symbol_and_color(&item.status);
                    let _ = write!(s, "{}{item_sym}{} {}",
                        SetForegroundColor(item_color), ResetColor, item.label);
                }
                s
            } else {
                let items: Vec<String> = section.active_items.iter().map(|item| {
                    let sym = match item.status {
                        ItemStatus::Running => self.config.symbols.item_running,
                        ItemStatus::Completed => self.config.symbols.item_completed,
                        ItemStatus::Failed => self.config.symbols.item_failed,
                        ItemStatus::Cached => self.config.symbols.item_cached,
                        ItemStatus::Skipped => self.config.symbols.item_skipped,
                    };
                    format!("{sym} {}", item.label)
                }).collect();
                format!("  {}", items.join("  "))
            };
            lines.push(item_line);
        }

        // Output lines (dimmed, truncated to fit terminal)
        let output_lines = self
            .output_buffers
            .get(&section.id)
            .map(|b| b.visible_lines(self.config.max_output_lines))
            .unwrap_or_default();

        let max_content = self.content_width().saturating_sub(4); // "    " indent
        for line in output_lines {
            let truncated = Self::truncate(line, max_content);
            let out_line = if self.config.colors {
                format!(
                    "    {}{}{}",
                    SetForegroundColor(Color::DarkGrey),
                    truncated,
                    ResetColor
                )
            } else {
                format!("    {truncated}")
            };
            lines.push(out_line);
        }

        lines
    }

    fn render_completed(&self, section: &Section) -> Vec<String> {
        let sym = self.config.symbols.completed;
        let label = &section.label;
        let (completed, total) = section.progress.unwrap_or((0, 0));
        let elapsed_str = section
            .elapsed
            .map(Self::format_duration)
            .unwrap_or_default();
        let counter = format!("{completed}/{total}");
        let elapsed_part = if elapsed_str.is_empty() {
            String::new()
        } else {
            format!(" · {elapsed_str}")
        };

        let cache_part = section
            .cache_info
            .map(|ci| format!(" ({}/{} cached)", ci.hits, ci.total))
            .unwrap_or_default();

        // Subtract cache_part from available bar width so the line doesn't wrap
        let base_bw = self.bar_width(label, completed, total, &elapsed_str);
        let bw = (base_bw as usize).saturating_sub(cache_part.len()).max(1) as u16;
        let bar_str = bar::render_full_bar(bw, Color::Green, self.config.colors);

        let line = if self.config.colors {
            let mut s = String::new();
            let _ = write!(
                s,
                "{}{sym}{} {}{label}{} {bar_str} {counter}{elapsed_part}{cache_part}",
                SetForegroundColor(Color::Green),
                ResetColor,
                SetAttribute(Attribute::Bold),
                SetAttribute(Attribute::Reset),
            );
            s
        } else {
            format!("{sym} {label} {bar_str} {counter}{elapsed_part}{cache_part}")
        };

        vec![line]
    }

    fn render_cached(&self, section: &Section) -> Vec<String> {
        let sym = self.config.symbols.cached;
        let label = &section.label;
        let (completed, total) = section.progress.unwrap_or((0, 0));

        let bw = self.bar_width_cached(label, completed, total);
        let bar_str = bar::render_full_bar(bw, Color::Green, self.config.colors);
        let counter = format!("{completed}/{total}");

        let line = if self.config.colors {
            let mut s = String::new();
            let _ = write!(
                s,
                "{}{sym}{} {}{label}{} {bar_str} {counter} cached",
                SetForegroundColor(Color::Green),
                ResetColor,
                SetAttribute(Attribute::Bold),
                SetAttribute(Attribute::Reset),
            );
            s
        } else {
            format!("{sym} {label} {bar_str} {counter} cached")
        };

        vec![line]
    }

    fn render_failed(&self, section: &Section) -> Vec<String> {
        let sym = self.config.symbols.failed;
        let label = &section.label;
        let (completed, total) = section.progress.unwrap_or((0, 0));
        let elapsed_str = section
            .elapsed
            .map(Self::format_duration)
            .unwrap_or_default();
        let counter = format!("{completed}/{total}");
        let elapsed_part = if elapsed_str.is_empty() {
            String::new()
        } else {
            format!(" · {elapsed_str}")
        };

        let bw = self.bar_width(label, completed, total, &elapsed_str);
        let bar_str = bar::render_failed_bar(bw, completed, total, self.config.colors);

        let header = if self.config.colors {
            let mut s = String::new();
            let _ = write!(
                s,
                "{}{sym}{} {}{label}{} {bar_str} {counter}{elapsed_part}",
                SetForegroundColor(Color::Red),
                ResetColor,
                SetAttribute(Attribute::Bold),
                SetAttribute(Attribute::Reset),
            );
            s
        } else {
            format!("{sym} {label} {bar_str} {counter}{elapsed_part}")
        };

        let mut lines = vec![header];

        // Error output lines with red border (truncated to fit terminal)
        let output_lines = self
            .output_buffers
            .get(&section.id)
            .map(|b| b.visible_lines(self.config.max_output_lines))
            .unwrap_or_default();

        let max_err_content = self.content_width().saturating_sub(4); // "  │ " prefix
        for line in output_lines {
            let truncated = Self::truncate(line, max_err_content);
            let out_line = if self.config.colors {
                format!(
                    "  {}│{} {truncated}",
                    SetForegroundColor(Color::Red),
                    ResetColor
                )
            } else {
                format!("  │ {truncated}")
            };
            lines.push(out_line);
        }

        lines
    }

    fn render_footer(&self, footer: &Footer) -> Vec<String> {
        let separator = if self.config.colors {
            format!(
                "{}{}{}",
                SetForegroundColor(Color::DarkGrey),
                "─".repeat(self.content_width()),
                ResetColor
            )
        } else {
            "─".repeat(self.content_width())
        };

        let text_line = if self.config.colors {
            format!(
                "{}{}{}",
                SetForegroundColor(Color::DarkGrey),
                footer.text,
                ResetColor
            )
        } else {
            footer.text.clone()
        };

        vec![separator, text_line]
    }

    // ── Helpers ─────────────────────────────────────────────────────────────

    fn item_symbol_and_color(&self, status: &ItemStatus) -> (&str, Color) {
        match status {
            ItemStatus::Running => (self.config.symbols.item_running, Color::DarkGrey),
            ItemStatus::Skipped => (self.config.symbols.item_skipped, Color::DarkGrey),
            ItemStatus::Completed => (self.config.symbols.item_completed, Color::Green),
            ItemStatus::Cached => (self.config.symbols.item_cached, Color::Green),
            ItemStatus::Failed => (self.config.symbols.item_failed, Color::Red),
        }
    }

    /// Usable content width — 1 less than terminal width to prevent
    /// full-width lines from triggering a phantom wrap on the terminal.
    fn content_width(&self) -> usize {
        (self.config.width as usize).saturating_sub(1)
    }

    /// Calculate the bar width available after the fixed-width text elements.
    /// Layout: `{sym} {label} {bar} {counter} {elapsed}`
    /// Fixed parts: sym(1) + space(1) + label + space(1) + space(1) + counter + space(1) + elapsed
    fn bar_width(&self, label: &str, completed: usize, total: usize, elapsed: &str) -> u16 {
        let counter = format!("{completed}/{total}");
        let elapsed_part = if elapsed.is_empty() {
            0
        } else {
            elapsed.len() + 3 // " · " separator
        };
        // sym + " " + label + " " + bar + " " + counter + elapsed_part
        let fixed: usize = 1 + 1 + label.chars().count() + 1 + 1 + counter.len() + elapsed_part;
        let available = self.content_width().saturating_sub(fixed);
        available.max(1) as u16
    }

    fn bar_width_cached(&self, label: &str, completed: usize, total: usize) -> u16 {
        let counter = format!("{completed}/{total}");
        // sym + " " + label + " " + bar + " " + counter + " cached"
        let fixed: usize = 1 + 1 + label.chars().count() + 1 + 1 + counter.len() + 7; // " cached" = 7
        let available = self.content_width().saturating_sub(fixed);
        available.max(1) as u16
    }

    /// Truncate a plain text string to fit within `max_cols` visible columns.
    /// This operates on the text content before ANSI codes are applied.
    fn truncate(text: &str, max_cols: usize) -> &str {
        if text.len() <= max_cols {
            return text;
        }
        // Find the byte boundary at or before max_cols chars
        let mut end = 0;
        for (col, (i, c)) in text.char_indices().enumerate() {
            if col >= max_cols {
                break;
            }
            end = i + c.len_utf8();
        }
        &text[..end]
    }

    pub fn format_duration(d: Duration) -> String {
        let total_secs = d.as_secs();
        if total_secs < 60 {
            let tenths = d.subsec_millis() / 100;
            format!("{total_secs}.{tenths}s")
        } else {
            let mins = total_secs / 60;
            let secs = total_secs % 60;
            format!("{mins}m{secs}s")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> RenderConfig {
        RenderConfig {
            width: 80,
            max_output_lines: 3,
            symbols: Symbols::default(),
            colors: false,
        }
    }

    fn render_to_string(renderer: &mut Renderer, frame: &Frame) -> String {
        let mut buf = Vec::new();
        renderer.render_frame(frame, &mut buf).unwrap();
        String::from_utf8(buf).unwrap()
    }

    #[test]
    fn new_renderer_has_no_state() {
        let r = Renderer::new(test_config());
        assert_eq!(r.last_frame_height(), 0);
    }

    #[test]
    fn push_output_creates_buffer() {
        let mut r = Renderer::new(test_config());
        r.push_output("lib", "compiling foo.c");
        assert_eq!(r.visible_lines("lib"), vec!["compiling foo.c"]);
    }

    #[test]
    fn push_output_appends_to_existing_buffer() {
        let mut r = Renderer::new(test_config());
        r.push_output("lib", "line 1");
        r.push_output("lib", "line 2");
        assert_eq!(r.visible_lines("lib"), vec!["line 1", "line 2"]);
    }

    #[test]
    fn set_error_shows_all_lines() {
        let mut r = Renderer::new(test_config());
        for i in 0..10 {
            r.push_output("lib", &format!("line {i}"));
        }
        r.set_error("lib");
        assert_eq!(r.visible_lines("lib").len(), 10);
    }

    #[test]
    fn set_width_updates_config() {
        let mut r = Renderer::new(test_config());
        r.set_width(120);
    }

    #[test]
    fn reset_clears_all_state() {
        let mut r = Renderer::new(test_config());
        r.push_output("lib", "line 1");
        r.set_error("lib");
        r.reset();
        assert!(r.visible_lines("lib").is_empty());
        assert_eq!(r.last_frame_height(), 0);
    }

    #[test]
    fn missing_section_returns_empty_lines() {
        let r = Renderer::new(test_config());
        assert!(r.visible_lines("nonexistent").is_empty());
    }

    // ── Task 7: frame rendering integration tests ────────────────────────

    #[test]
    fn render_waiting_section() {
        let mut r = Renderer::new(test_config());
        let frame = Frame::new().section(Section::new("lib", "lib").status(Status::Waiting));
        let out = render_to_string(&mut r, &frame);
        assert!(out.contains("◇ lib"), "got: {out:?}");
        assert_eq!(r.last_frame_height(), 1);
    }

    #[test]
    fn render_running_section_with_bar() {
        let mut r = Renderer::new(test_config());
        r.push_output("lib", "compiling foo.c");
        let frame = Frame::new().section(
            Section::new("lib", "lib")
                .status(Status::Running)
                .progress(3, 5)
                .elapsed(Duration::from_millis(800))
                .active_item("foo.c", ItemStatus::Running),
        );
        let out = render_to_string(&mut r, &frame);
        assert!(out.contains("◆"), "got: {out:?}");
        assert!(out.contains("lib"), "got: {out:?}");
        assert!(out.contains("━"), "got: {out:?}");
        assert!(out.contains("3/5"), "got: {out:?}");
        assert!(out.contains("0.8s"), "got: {out:?}");
        assert!(out.contains("foo.c"), "got: {out:?}");
        assert!(out.contains("compiling foo.c"), "got: {out:?}");
        assert_eq!(r.last_frame_height(), 3, "got: {out:?}");
    }

    #[test]
    fn render_completed_section() {
        let mut r = Renderer::new(test_config());
        let frame = Frame::new().section(
            Section::new("lib", "lib")
                .status(Status::Completed)
                .progress(5, 5)
                .elapsed(Duration::from_secs(2)),
        );
        let out = render_to_string(&mut r, &frame);
        assert!(out.contains("✓ lib"), "got: {out:?}");
        assert!(out.contains("5/5"), "got: {out:?}");
        assert_eq!(r.last_frame_height(), 1);
    }

    #[test]
    fn render_completed_with_cache_info() {
        let mut r = Renderer::new(test_config());
        let frame = Frame::new().section(
            Section::new("lib", "lib")
                .status(Status::Completed)
                .progress(5, 5)
                .cache(3, 5),
        );
        let out = render_to_string(&mut r, &frame);
        assert!(out.contains("3/5 cached"), "got: {out:?}");
    }

    #[test]
    fn render_cached_section() {
        let mut r = Renderer::new(test_config());
        let frame = Frame::new().section(
            Section::new("lib", "lib")
                .status(Status::Cached)
                .progress(5, 5),
        );
        let out = render_to_string(&mut r, &frame);
        assert!(out.contains("≋ lib"), "got: {out:?}");
        assert!(out.contains("cached"), "got: {out:?}");
        assert_eq!(r.last_frame_height(), 1);
    }

    #[test]
    fn render_failed_section_with_error_output() {
        let mut r = Renderer::new(test_config());
        r.push_output("lib", "error: undefined ref");
        r.push_output("lib", "error: link failed");
        r.set_error("lib");
        let frame = Frame::new().section(
            Section::new("lib", "lib")
                .status(Status::Failed)
                .progress(3, 5)
                .elapsed(Duration::from_secs(1)),
        );
        let out = render_to_string(&mut r, &frame);
        assert!(out.contains("✗ lib"), "got: {out:?}");
        assert!(out.contains("│"), "got: {out:?}");
        assert!(out.contains("error: undefined ref"), "got: {out:?}");
        assert_eq!(r.last_frame_height(), 3, "got: {out:?}");
    }

    #[test]
    fn render_footer() {
        let mut r = Renderer::new(test_config());
        let frame = Frame::new()
            .section(Section::new("lib", "lib").status(Status::Running).progress(1, 5))
            .footer("1 running · 0 failed");
        let out = render_to_string(&mut r, &frame);
        assert!(out.contains("─"), "got: {out:?}");
        assert!(out.contains("1 running · 0 failed"), "got: {out:?}");
        // 1 section line + 2 footer lines (separator + text)
        assert_eq!(r.last_frame_height(), 3, "got: {out:?}");
    }

    #[test]
    fn clear_last_frame_emits_correct_escapes() {
        let mut r = Renderer::new(test_config());
        let frame = Frame::new()
            .section(Section::new("a", "a").status(Status::Waiting))
            .section(Section::new("b", "b").status(Status::Waiting));
        // Render to get height = 2
        let mut sink = Vec::new();
        r.render_frame(&frame, &mut sink).unwrap();
        assert_eq!(r.last_frame_height(), 2);

        let mut clear_buf = Vec::new();
        r.clear_last_frame(&mut clear_buf).unwrap();
        let clear_str = String::from_utf8(clear_buf).unwrap();
        // Each line produces \x1b[1A\x1b[2K
        assert_eq!(
            clear_str.matches("\x1b[1A\x1b[2K").count(),
            2,
            "got: {clear_str:?}"
        );
    }

    #[test]
    fn render_composite_frame() {
        let mut r = Renderer::new(test_config());
        // deps(completed) + lib(running, 1 active item) + test(waiting) + footer = ?
        // deps: 1 line, lib: 1 header + 1 active item = 2, test: 1 line, footer: 2 lines = 6
        let frame = Frame::new()
            .section(
                Section::new("deps", "deps")
                    .status(Status::Completed)
                    .progress(10, 10)
                    .elapsed(Duration::from_secs(1)),
            )
            .section(
                Section::new("lib", "lib")
                    .status(Status::Running)
                    .progress(3, 10)
                    .elapsed(Duration::from_millis(500))
                    .active_item("foo.c", ItemStatus::Running),
            )
            .section(Section::new("test", "test").status(Status::Waiting))
            .footer("1 completed · 1 running · 1 waiting");

        let out = render_to_string(&mut r, &frame);
        assert_eq!(r.last_frame_height(), 6, "got:\n{out}");
    }
}
