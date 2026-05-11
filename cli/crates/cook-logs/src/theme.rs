//! Color palette for the logs viewer.

use ratatui::style::{Color, Modifier, Style};

#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub fg: Color,
    pub fg_dim: Color,
    pub accent: Color,
    pub ok: Color,
    pub warn: Color,
    pub err: Color,
    pub skip: Color,
    pub selection_bg: Color,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            fg: Color::Reset,
            fg_dim: Color::DarkGray,
            accent: Color::Cyan,
            ok: Color::Green,
            warn: Color::Yellow,
            err: Color::Red,
            skip: Color::DarkGray,
            selection_bg: Color::Rgb(40, 40, 60),
        }
    }
}

impl Theme {
    pub fn ok_style(&self) -> Style { Style::default().fg(self.ok) }
    pub fn err_style(&self) -> Style { Style::default().fg(self.err) }
    pub fn skip_style(&self) -> Style { Style::default().fg(self.skip) }
    pub fn dim_style(&self) -> Style { Style::default().fg(self.fg_dim) }
    pub fn header_style(&self) -> Style { Style::default().fg(self.accent).add_modifier(Modifier::BOLD) }
    pub fn selection_style(&self) -> Style { Style::default().bg(self.selection_bg).add_modifier(Modifier::BOLD) }
}
