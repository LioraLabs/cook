//! Terminal color detection and configuration.

use std::str::FromStr;

use crossterm::style::Stylize;

// ---------------------------------------------------------------------------
// ColorMode
// ---------------------------------------------------------------------------

/// Controls when ANSI color output is used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ColorMode {
    /// Enable color only when stdout is a TTY and `NO_COLOR` is not set.
    #[default]
    Auto,
    /// Always emit ANSI color sequences.
    Always,
    /// Never emit ANSI color sequences.
    Never,
}

impl FromStr for ColorMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "auto" => Ok(ColorMode::Auto),
            "always" => Ok(ColorMode::Always),
            "never" => Ok(ColorMode::Never),
            other => Err(format!("unknown color mode: {other}")),
        }
    }
}

// ---------------------------------------------------------------------------
// ColorConfig
// ---------------------------------------------------------------------------

/// Resolved color configuration for a single run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub struct ColorConfig {
    pub enabled: bool,
}

impl ColorConfig {
    /// Resolve the effective color config from the mode, TTY status, and
    /// whether the `NO_COLOR` environment variable is set.
    ///
    /// - `Always`  -> enabled regardless of TTY or `NO_COLOR`
    /// - `Never`   -> disabled regardless of TTY or `NO_COLOR`
    /// - `Auto`    -> enabled only when `is_tty` is true and `no_color_set` is false
    pub fn resolve(mode: ColorMode, is_tty: bool, no_color_set: bool) -> Self {
        let enabled = match mode {
            ColorMode::Always => true,
            ColorMode::Never => false,
            ColorMode::Auto => is_tty && !no_color_set,
        };
        ColorConfig { enabled }
    }

    // -----------------------------------------------------------------------
    // Color helper methods
    // -----------------------------------------------------------------------

    /// Wrap `s` in green ANSI styling if color is enabled.
    pub fn green(&self, s: &str) -> String {
        if self.enabled {
            s.green().to_string()
        } else {
            s.to_string()
        }
    }

    /// Wrap `s` in red ANSI styling if color is enabled.
    pub fn red(&self, s: &str) -> String {
        if self.enabled {
            s.red().to_string()
        } else {
            s.to_string()
        }
    }

    /// Wrap `s` in blue ANSI styling if color is enabled.
    #[allow(dead_code)]
    pub fn blue(&self, s: &str) -> String {
        if self.enabled {
            s.blue().to_string()
        } else {
            s.to_string()
        }
    }

    /// Wrap `s` in magenta ANSI styling if color is enabled.
    #[allow(dead_code)]
    pub fn magenta(&self, s: &str) -> String {
        if self.enabled {
            s.magenta().to_string()
        } else {
            s.to_string()
        }
    }

    /// Wrap `s` in dim ANSI styling if color is enabled.
    pub fn dim(&self, s: &str) -> String {
        if self.enabled {
            s.dim().to_string()
        } else {
            s.to_string()
        }
    }

    /// Wrap `s` in bold ANSI styling if color is enabled.
    pub fn bold(&self, s: &str) -> String {
        if self.enabled {
            s.bold().to_string()
        } else {
            s.to_string()
        }
    }
}

// ---------------------------------------------------------------------------
// Symbols
// ---------------------------------------------------------------------------

/// Unicode symbols used throughout the progress rendering system.
#[allow(dead_code)]
pub struct Symbols {
    pub finished: &'static str,
    pub running: &'static str,
    pub waiting: &'static str,
    pub cache_hit: &'static str,
    pub success: &'static str,
    pub failure: &'static str,
}

impl Symbols {
    pub fn new() -> Self {
        Symbols {
            finished: "\u{25c6}",
            running: "\u{25c7}",
            waiting: "\u{25cb}",
            cache_hit: "\u{224b}",
            success: "\u{2713}",
            failure: "\u{2717}",
        }
    }
}

impl Default for Symbols {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_color_mode_auto_respects_no_color_env() {
        let config = ColorConfig::resolve(ColorMode::Auto, false, true);
        assert!(!config.enabled);
    }

    #[test]
    fn test_color_mode_always_overrides() {
        let config = ColorConfig::resolve(ColorMode::Always, false, true);
        assert!(config.enabled);
    }

    #[test]
    fn test_color_mode_never() {
        let config = ColorConfig::resolve(ColorMode::Never, true, false);
        assert!(!config.enabled);
    }

    #[test]
    fn test_color_mode_auto_tty() {
        let config = ColorConfig::resolve(ColorMode::Auto, true, false);
        assert!(config.enabled);
    }

    #[test]
    fn test_color_mode_auto_no_tty() {
        let config = ColorConfig::resolve(ColorMode::Auto, false, false);
        assert!(!config.enabled);
    }

    #[test]
    fn test_symbols() {
        let s = Symbols::new();
        assert_eq!(s.finished, "\u{25c6}");
        assert_eq!(s.running, "\u{25c7}");
        assert_eq!(s.waiting, "\u{25cb}");
        assert_eq!(s.cache_hit, "\u{224b}");
        assert_eq!(s.success, "\u{2713}");
        assert_eq!(s.failure, "\u{2717}");
    }
}
