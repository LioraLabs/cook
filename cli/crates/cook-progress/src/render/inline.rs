//! Inline renderer — append-only event lines + sticky status line.
//!
//! Replaces the indicatif `MultiProgress` model from
//! 2026-04-20-cook-indicatif-rewrite-design.md. See
//! 2026-05-03-cook-progress-cargo-style-design.md for the design.

use std::io::{self, Write};

use crate::event::ProgressEvent;
use crate::model::build::BuildState;
use crate::render::event_writer::{EventWriter, EventWriterOptions};
use crate::render::snapshot::{StatusLineOptions, StatusSnapshot};
use crate::render::status_line::StatusLine;
use crate::render::Renderer;

#[derive(Debug, Clone, Copy, Default)]
pub struct InlineOptions {
    pub event: EventWriterOptions,
    pub status: StatusLineOptions,
    /// Render with the status line at all (false on `--quiet`, non-TTY,
    /// `NO_PROGRESS=1`, etc.). The renderer enforces the rest of the
    /// hide rules (min nodes, interactive windows, build done).
    pub status_enabled: bool,
}

pub struct InlineRenderer {
    event_writer: EventWriter,
    status: Option<StatusLine>,
}

impl InlineRenderer {
    pub fn new(opts: InlineOptions) -> Self {
        let event_writer = EventWriter::new(opts.event);
        let status = if opts.status_enabled {
            Some(StatusLine::spawn(opts.status, StatusSnapshot::empty()))
        } else {
            None
        };
        Self { event_writer, status }
    }
}

impl Renderer for InlineRenderer {
    fn handle(&mut self, state: &BuildState, event: &ProgressEvent) -> io::Result<()> {
        // Coordinate with the tick thread: clear the status line before
        // every event line we emit. Lock stderr so the tick thread can't
        // interleave.
        let mut stderr = io::stderr().lock();
        write!(stderr, "{}", crate::render::CLEAR_LINE)?;

        // Hand the event to EventWriter. It returns whether anything was written.
        let _wrote = self.event_writer.handle(&mut stderr, state, event)?;
        stderr.flush()?;
        drop(stderr);

        // Update + show/hide the status line.
        if let Some(s) = &self.status {
            match event {
                ProgressEvent::InteractiveStart { .. } => s.hide(),
                ProgressEvent::InteractiveEnd { is_terminal: false, .. } => {
                    s.update(StatusSnapshot::from_state(state));
                    s.show();
                }
                ProgressEvent::InteractiveEnd { is_terminal: true, .. } => {
                    s.hide();
                }
                ProgressEvent::Finished { .. } => s.hide(),
                _ => {
                    s.update(StatusSnapshot::from_state(state));
                    s.show();
                }
            }
        }
        Ok(())
    }

    fn finish(&mut self, _state: &BuildState) -> io::Result<()> {
        // Drop the StatusLine first — its Drop impl shuts down the tick thread,
        // ensuring no further repaints can race with our final clear.
        self.status.take();
        let mut stderr = io::stderr().lock();
        write!(stderr, "{}", crate::render::CLEAR_LINE)?;
        stderr.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{NodeId, NodeKind, ProgressEvent, RecipeId, RecipeTopo};
    use std::time::Duration;

    #[test]
    fn finish_clears_status_line_and_shuts_down_thread() {
        let opts = InlineOptions {
            event: EventWriterOptions { colored: false, ..Default::default() },
            status: StatusLineOptions { colored: false, ..Default::default() },
            status_enabled: false, // status line off in test to avoid stderr writes
        };
        let mut r = InlineRenderer::new(opts);
        let mut state = BuildState::new();
        let ev = ProgressEvent::BuildStarted {
            recipes: vec![RecipeTopo {
                id: RecipeId::new(0), name: "lib".into(), deps: vec![], expected_nodes: 1,
            }],
            total_nodes: 1,
        };
        state.apply(&ev);
        r.handle(&state, &ev).unwrap();
        r.finish(&state).unwrap();
    }

    #[test]
    fn handle_routes_events_to_event_writer() {
        // Smoke: build a complete event sequence without panicking.
        let opts = InlineOptions {
            event: EventWriterOptions { colored: false, ..Default::default() },
            status_enabled: false,
            ..Default::default()
        };
        let mut r = InlineRenderer::new(opts);
        let mut state = BuildState::new();
        for ev in [
            ProgressEvent::BuildStarted {
                recipes: vec![RecipeTopo { id: RecipeId::new(0), name: "lib".into(), deps: vec![], expected_nodes: 1 }],
                total_nodes: 1,
            },
            ProgressEvent::RecipeStarted { recipe: RecipeId::new(0) },
            ProgressEvent::NodeStarted {
                recipe: RecipeId::new(0), node: NodeId::new(0),
                name: "x.c".into(), artifact: None, fallback_label: "x".into(),
                kind: NodeKind::Compile,
            },
            ProgressEvent::NodeCompleted {
                recipe: RecipeId::new(0), node: NodeId::new(0),
                elapsed: Duration::from_millis(100), kind: NodeKind::Compile,
            },
            ProgressEvent::RecipeCompleted {
                recipe: RecipeId::new(0),
                elapsed: Duration::from_millis(150), cached: 0, total: 1,
            },
            ProgressEvent::Finished { success: true },
        ] {
            state.apply(&ev);
            r.handle(&state, &ev).unwrap();
        }
        r.finish(&state).unwrap();
    }
}
