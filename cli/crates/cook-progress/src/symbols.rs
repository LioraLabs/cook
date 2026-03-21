#[derive(Debug, Clone)]
pub struct Symbols {
    pub running: &'static str,
    pub completed: &'static str,
    pub failed: &'static str,
    pub cached: &'static str,
    pub waiting: &'static str,
    pub item_running: &'static str,
    pub item_completed: &'static str,
    pub item_failed: &'static str,
    pub item_cached: &'static str,
    pub item_skipped: &'static str,
}

impl Default for Symbols {
    fn default() -> Self {
        Self {
            running: "◆",
            completed: "✓",
            failed: "✗",
            cached: "≋",
            waiting: "◇",
            item_running: "◇",
            item_completed: "✓",
            item_failed: "✗",
            item_cached: "≋",
            item_skipped: "○",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_symbols() {
        let s = Symbols::default();
        assert_eq!(s.running, "◆");
        assert_eq!(s.completed, "✓");
        assert_eq!(s.failed, "✗");
        assert_eq!(s.cached, "≋");
        assert_eq!(s.waiting, "◇");
    }

    #[test]
    fn default_item_symbols() {
        let s = Symbols::default();
        assert_eq!(s.item_running, "◇");
        assert_eq!(s.item_completed, "✓");
        assert_eq!(s.item_failed, "✗");
        assert_eq!(s.item_cached, "≋");
        assert_eq!(s.item_skipped, "○");
    }
}
