//! Verb vocabulary for the inline event renderer.

use crate::event::NodeKind;

/// What kind of line we're rendering. Drives both verb choice and color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    /// NodeCompleted (artifact-producing) — verb derived from NodeKind.
    NodeCompleted,
    /// NodeCacheHit.
    NodeCached,
    /// NodeFailed.
    NodeFailed,
    /// NodeSkipped (single, non-collapsed).
    NodeSkipped,
    /// RecipeCompleted (success) or build-end success.
    RecipeFinished,
    /// RecipeFailed or build-end failure.
    RecipeFailed,
    /// InteractiveStart — chore handoff.
    InteractiveRunning,
    /// Sticky status-line verb (always "Cooking" regardless of node kinds).
    StatusBar,
}

/// Color slot. Renderer maps each to ANSI codes (or to the empty string
/// when `NO_COLOR` is set or output is not a TTY).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerbColor {
    Default,
    Dim,
    Yellow,
    Green,
    Red,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Verb {
    pub text: &'static str,
    pub color: VerbColor,
    pub bold: bool,
}

/// Pure mapping `(LineKind, NodeKind) -> Verb`. NodeKind is only consulted
/// for `LineKind::NodeCompleted`; other LineKinds ignore it.
pub const fn verb_for(line: LineKind, kind: NodeKind) -> Verb {
    match line {
        LineKind::NodeCompleted => match kind {
            NodeKind::Compile  => Verb { text: "Compiled",  color: VerbColor::Default, bold: true },
            NodeKind::Link     => Verb { text: "Linked",    color: VerbColor::Default, bold: true },
            NodeKind::Resolve  => Verb { text: "Resolved",  color: VerbColor::Default, bold: true },
            NodeKind::Generate => Verb { text: "Generated", color: VerbColor::Default, bold: true },
            NodeKind::Write    => Verb { text: "Wrote",     color: VerbColor::Default, bold: true },
            NodeKind::Test     => Verb { text: "Tested",    color: VerbColor::Green,   bold: true },
            NodeKind::Cooked   => Verb { text: "Cooked",    color: VerbColor::Default, bold: true },
        },
        LineKind::NodeCached         => Verb { text: "Cached",    color: VerbColor::Dim,    bold: false },
        LineKind::NodeSkipped        => Verb { text: "Skipped",   color: VerbColor::Yellow, bold: false },
        LineKind::NodeFailed         => Verb { text: "Failed",    color: VerbColor::Red,    bold: true },
        LineKind::RecipeFinished     => Verb { text: "Finished",  color: VerbColor::Green,  bold: true },
        LineKind::RecipeFailed       => Verb { text: "Failed",    color: VerbColor::Red,    bold: true },
        LineKind::InteractiveRunning => Verb { text: "Running",   color: VerbColor::Green,  bold: true },
        LineKind::StatusBar          => Verb { text: "Cooking",   color: VerbColor::Default, bold: true },
    }
}

/// Width of the right-aligned verb column. Matches cargo's 12-col padding.
pub const VERB_COL_WIDTH: usize = 12;

/// Format a verb prefix with right-aligned text and optional ANSI styling.
/// `colored` controls whether ANSI escapes are emitted; pass `false` for
/// snapshot tests, plain renderer reuse, and NO_COLOR.
pub fn format_verb(verb: Verb, colored: bool) -> String {
    let padded = format!("{:>width$}", verb.text, width = VERB_COL_WIDTH);
    if !colored {
        return padded;
    }
    let mut out = String::new();
    if verb.bold { out.push_str("\x1b[1m"); }
    match verb.color {
        VerbColor::Default => {}
        VerbColor::Dim     => out.push_str("\x1b[2m"),
        VerbColor::Yellow  => out.push_str("\x1b[33m"),
        VerbColor::Green   => out.push_str("\x1b[32m"),
        VerbColor::Red     => out.push_str("\x1b[31m"),
    }
    out.push_str(&padded);
    out.push_str("\x1b[0m");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compile_kind_maps_to_compiled_verb() {
        let v = verb_for(LineKind::NodeCompleted, NodeKind::Compile);
        assert_eq!(v.text, "Compiled");
        assert!(v.bold);
        assert_eq!(v.color, VerbColor::Default);
    }

    #[test]
    fn cooked_is_default_for_completed_node_with_no_kind_info() {
        let v = verb_for(LineKind::NodeCompleted, NodeKind::Cooked);
        assert_eq!(v.text, "Cooked");
    }

    #[test]
    fn cached_uses_dim_color() {
        let v = verb_for(LineKind::NodeCached, NodeKind::Cooked);
        assert_eq!(v.text, "Cached");
        assert_eq!(v.color, VerbColor::Dim);
    }

    #[test]
    fn failed_is_bold_red() {
        let v = verb_for(LineKind::NodeFailed, NodeKind::Cooked);
        assert!(v.bold);
        assert_eq!(v.color, VerbColor::Red);
    }

    #[test]
    fn finished_is_bold_green() {
        let v = verb_for(LineKind::RecipeFinished, NodeKind::Cooked);
        assert!(v.bold);
        assert_eq!(v.color, VerbColor::Green);
    }

    #[test]
    fn status_bar_verb_is_cooking() {
        let v = verb_for(LineKind::StatusBar, NodeKind::Cooked);
        assert_eq!(v.text, "Cooking");
        assert!(v.bold);
    }

    #[test]
    fn format_verb_pads_to_12_cols() {
        let v = verb_for(LineKind::NodeCompleted, NodeKind::Cooked);
        let s = format_verb(v, false);
        assert_eq!(s.chars().count(), 12);
        assert!(s.ends_with("Cooked"));
    }

    #[test]
    fn format_verb_with_color_wraps_in_ansi() {
        let v = verb_for(LineKind::NodeFailed, NodeKind::Cooked);
        let s = format_verb(v, true);
        assert!(s.starts_with("\x1b[1m\x1b[31m"));
        assert!(s.ends_with("\x1b[0m"));
    }
}
