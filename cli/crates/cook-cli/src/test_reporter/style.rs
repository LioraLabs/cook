//! Color resolution and ANSI helpers for the test reporter, per §3.5
//! of the test-runner output design.
//!
//! Color decisions follow this precedence:
//!   1. `--color=always|never|auto` flag
//!   2. `NO_COLOR` env var (any non-empty value forces no color)
//!   3. TTY detection on stdout
//!
//! ANSI codes used are standard 16-color SGR escapes — works on every
//! terminal that supports color at all.

/// Resolve whether to emit color, based on the cli flag, the env, and
/// whether stdout is a terminal.
pub fn resolve_color_choice(
    cli_color: &str,
    no_color_env: Option<&str>,
    is_tty: bool,
) -> bool {
    match cli_color {
        "always" => true,
        "never" => false,
        _ => {
            // auto
            if let Some(v) = no_color_env {
                if !v.is_empty() {
                    return false;
                }
            }
            is_tty
        }
    }
}

#[derive(Clone, Copy)]
pub struct Style {
    pub colored: bool,
}

impl Style {
    pub fn new(colored: bool) -> Self {
        Self { colored }
    }

    fn wrap(&self, code: &str, s: &str) -> String {
        if self.colored {
            format!("\x1b[{code}m{s}\x1b[0m")
        } else {
            s.to_string()
        }
    }

    pub fn green(&self, s: &str) -> String { self.wrap("32", s) }
    pub fn red(&self, s: &str) -> String { self.wrap("31", s) }
    pub fn yellow(&self, s: &str) -> String { self.wrap("33", s) }
    pub fn cyan(&self, s: &str) -> String { self.wrap("36", s) }
    pub fn dim(&self, s: &str) -> String { self.wrap("2", s) }
    pub fn bold(&self, s: &str) -> String { self.wrap("1", s) }
    pub fn bold_red(&self, s: &str) -> String { self.wrap("1;31", s) }
    pub fn bold_yellow(&self, s: &str) -> String { self.wrap("1;33", s) }
    pub fn dim_cyan(&self, s: &str) -> String { self.wrap("2;36", s) }
}

#[cfg(test)]
#[path = "tests/style_tests.rs"]
mod tests;
