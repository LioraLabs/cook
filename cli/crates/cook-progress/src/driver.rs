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
mod tests {
    use super::*;
    use crate::event::{RecipeId, RecipeTopo};
    use crate::render::plain::PlainRenderer;
    use std::time::Duration;

    struct SharedWriter(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);
    impl std::io::Write for SharedWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
    }

    #[test]
    fn driver_consumes_events_until_finished() {
        let (tx, rx) = mpsc::channel();
        let shared = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let renderer = Box::new(PlainRenderer::new(SharedWriter(shared.clone())));
        let mut driver = Driver::new(renderer, None);

        tx.send(ProgressEvent::BuildStarted {
            recipes: vec![RecipeTopo { id: RecipeId::new(0), name: "deps".into(), deps: vec![], expected_nodes: 1 }],
            total_nodes: 1,
        }).unwrap();
        tx.send(ProgressEvent::RecipeStarted { recipe: RecipeId::new(0) }).unwrap();
        tx.send(ProgressEvent::RecipeCompleted {
            recipe: RecipeId::new(0),
            elapsed: Duration::from_millis(10),
            cached: 0, total: 1,
            kind: crate::event::RecipeKind::Recipe,
        }).unwrap();
        tx.send(ProgressEvent::Finished { success: true }).unwrap();
        drop(tx);

        let success = driver.run(rx).unwrap();
        assert!(success);
        let out = String::from_utf8(shared.lock().unwrap().clone()).unwrap();
        assert!(out.contains("deps"), "got: {out}");
        assert!(out.contains("done"), "got: {out}");
    }
}
