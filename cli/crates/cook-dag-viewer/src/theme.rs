//! Color theme — see design spec §Themes.

use ratatui::style::Color;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeKind {
    Auto,
    Mono,
}

#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub edge: Color,
    pub file: Color,
    pub badge_cached: Color,
    pub badge_stale: Color,
    pub badge_modified: Color,
    pub badge_discovered: Color,
    pub pin_slots: [Color; 9],
    pub selected_ring: Color,
    pub search_highlight: Color,
    pub kind: ThemeKind,
}

impl Theme {
    pub fn auto() -> Self {
        Self {
            edge: Color::DarkGray,
            file: Color::Gray,
            badge_cached: Color::Green,
            badge_stale: Color::Red,
            badge_modified: Color::Yellow,
            badge_discovered: Color::Cyan,
            pin_slots: [
                Color::Magenta,
                Color::Blue,
                Color::LightMagenta,
                Color::LightBlue,
                Color::LightCyan,
                Color::White,
                Color::Gray,
                Color::DarkGray,
                Color::Cyan,
            ],
            selected_ring: Color::White,
            search_highlight: Color::LightYellow,
            kind: ThemeKind::Auto,
        }
    }

    pub fn mono() -> Self {
        Self {
            edge: Color::Reset,
            file: Color::Reset,
            badge_cached: Color::Reset,
            badge_stale: Color::Reset,
            badge_modified: Color::Reset,
            badge_discovered: Color::Reset,
            pin_slots: [Color::Reset; 9],
            selected_ring: Color::Reset,
            search_highlight: Color::Reset,
            kind: ThemeKind::Mono,
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "mono" => Self::mono(),
            _ => Self::auto(),
        }
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::auto()
    }
}
