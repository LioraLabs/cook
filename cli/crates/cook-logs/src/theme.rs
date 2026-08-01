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
    /// Palette for terminals where colour is unwanted or unavailable.
    ///
    /// COOK-409: `cook logs --theme` documented "auto (default) or mono" and
    /// there was no mono theme; `cmd_logs` was always handed
    /// `Theme::default()`, so the flag parsed and was discarded. Rather than
    /// delete a documented flag, this is the theme it promised.
    ///
    /// Status is carried by the glyph and by bold/dim weight rather than hue,
    /// so a failed node stays distinguishable with colour off.
    pub fn mono() -> Self {
        Self {
            fg: Color::Reset,
            fg_dim: Color::DarkGray,
            accent: Color::Reset,
            ok: Color::Reset,
            warn: Color::Reset,
            err: Color::Reset,
            skip: Color::DarkGray,
            selection_bg: Color::Reset,
        }
    }

    /// Resolve a `--theme` value. `auto` is the colour palette; `mono` drops
    /// hue. An unknown name is the caller's diagnostic, never a silent default.
    pub fn from_name(name: &str) -> Result<Self, String> {
        match name {
            "auto" => Ok(Self::default()),
            "mono" => Ok(Self::mono()),
            other => Err(format!(
                "unknown theme '{other}'; expected 'auto' or 'mono'"
            )),
        }
    }

    /// True when this palette carries no hue, so callers can lean on weight.
    pub fn is_mono(&self) -> bool {
        self.accent == Color::Reset && self.ok == Color::Reset && self.err == Color::Reset
    }

    pub fn ok_style(&self) -> Style { Style::default().fg(self.ok) }
    pub fn err_style(&self) -> Style { Style::default().fg(self.err) }
    pub fn skip_style(&self) -> Style { Style::default().fg(self.skip) }
    pub fn dim_style(&self) -> Style { Style::default().fg(self.fg_dim) }
    pub fn header_style(&self) -> Style { Style::default().fg(self.accent).add_modifier(Modifier::BOLD) }
    pub fn selection_style(&self) -> Style { Style::default().bg(self.selection_bg).add_modifier(Modifier::BOLD) }
}
