use crossterm::style::{Color, ResetColor, SetForegroundColor};
use std::fmt::Write as FmtWrite;

const BAR_CHAR: &str = "━";

pub fn render_bar(width: u16, completed: usize, total: usize, colors: bool) -> String {
    let width = width as usize;
    let filled = if total == 0 { 0 } else { (completed * width) / total };
    let empty = width - filled;

    if colors {
        let mut s = String::new();
        let _ = write!(
            s,
            "{}{}{}{}{}",
            SetForegroundColor(Color::Green),
            BAR_CHAR.repeat(filled),
            SetForegroundColor(Color::DarkGrey),
            BAR_CHAR.repeat(empty),
            ResetColor
        );
        s
    } else {
        BAR_CHAR.repeat(width)
    }
}

pub fn render_full_bar(width: u16, color: Color, colors: bool) -> String {
    let bar = BAR_CHAR.repeat(width as usize);
    if colors {
        format!("{}{bar}{}", SetForegroundColor(color), ResetColor)
    } else {
        bar
    }
}

pub fn render_failed_bar(width: u16, completed: usize, total: usize, colors: bool) -> String {
    let width_usize = width as usize;
    let filled = if total == 0 { 0 } else { (completed * width_usize) / total };
    let empty = width_usize - filled;

    if colors {
        let mut s = String::new();
        let _ = write!(
            s,
            "{}{}{}{}{}",
            SetForegroundColor(Color::Red),
            BAR_CHAR.repeat(filled),
            SetForegroundColor(Color::DarkGrey),
            BAR_CHAR.repeat(empty),
            ResetColor
        );
        s
    } else {
        BAR_CHAR.repeat(width_usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_bar() {
        let bar = render_bar(20, 5, 5, false);
        assert_eq!(bar, "━━━━━━━━━━━━━━━━━━━━");
    }

    #[test]
    fn empty_bar() {
        let bar = render_bar(20, 0, 5, false);
        assert_eq!(bar, "━━━━━━━━━━━━━━━━━━━━");
    }

    #[test]
    fn half_bar() {
        let bar = render_bar(20, 3, 6, false);
        assert_eq!(bar.chars().count(), 20);
    }

    #[test]
    fn zero_total_is_empty_bar() {
        let bar = render_bar(20, 0, 0, false);
        assert_eq!(bar.chars().count(), 20);
    }

    #[test]
    fn bar_width_respected() {
        for width in [10, 20, 40, 80] {
            let bar = render_bar(width, 3, 10, false);
            assert_eq!(bar.chars().count(), width as usize);
        }
    }

    #[test]
    fn colored_bar_contains_ansi() {
        let bar = render_bar(20, 3, 6, true);
        assert!(bar.contains('\x1b'));
    }
}
