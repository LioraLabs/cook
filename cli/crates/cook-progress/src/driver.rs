//! Event-loop driver — wires BuildState + renderer + log store.

use std::io;
use std::sync::mpsc;

use crate::event::ProgressEvent;
use crate::log_store::LogStore;
use crate::model::build::BuildState;
use crate::render::Renderer;

pub struct Driver {
    pub state: BuildState,
    pub renderer: Box<dyn Renderer>,
    pub log_store: Option<LogStore>,
}

impl Driver {
    pub fn new(renderer: Box<dyn Renderer>, log_store: Option<LogStore>) -> Self {
        Self { state: BuildState::new(), renderer, log_store }
    }

    pub fn run(&mut self, rx: mpsc::Receiver<ProgressEvent>) -> io::Result<bool> {
        while let Ok(event) = rx.recv() {
            self.state.apply(&event);
            if let Some(store) = self.log_store.as_mut() {
                let _ = store.record(&self.state, &event);
            }
            self.renderer.handle(&self.state, &event)?;
            if matches!(event, ProgressEvent::Finished { .. }) {
                break;
            }
        }
        self.renderer.finish(&self.state)?;
        let success = self.state.finished.unwrap_or(false);
        if let Some(store) = self.log_store.as_mut() { let _ = store.close(success); }
        Ok(success)
    }
}

#[cfg(test)]
#[path = "tests/driver_tests.rs"]
mod tests;
