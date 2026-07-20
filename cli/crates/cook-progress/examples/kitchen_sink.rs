//! Minimal kitchen-sink demo of the new cook-progress API.

use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use cook_progress::{
    Driver, EventWriterOptions, InlineOptions, InlineRenderer, NodeId, NodeKind,
    ProgressEvent, RecipeId, RecipeTopo, StatusLineOptions,
};

fn main() {
    let (tx, rx) = mpsc::channel::<ProgressEvent>();
    let handle = thread::spawn(move || {
        let opts = InlineOptions {
            event: EventWriterOptions::default(),
            status: StatusLineOptions::default(),
            status_enabled: true,
        };
        let mut driver = Driver::new(
            Box::new(InlineRenderer::new(opts)),
            None,
        );
        driver.run(rx).unwrap();
    });

    tx.send(ProgressEvent::BuildStarted {
        recipes: vec![
            RecipeTopo { id: RecipeId::new(0), name: "deps".into(), deps: vec![], expected_nodes: 2 },
            RecipeTopo { id: RecipeId::new(1), name: "lib".into(), deps: vec![RecipeId::new(0)], expected_nodes: 3 },
        ],
        total_nodes: 5,
    }).unwrap();

    thread::sleep(Duration::from_millis(200));
    tx.send(ProgressEvent::RecipeStarted { recipe: RecipeId::new(0) }).unwrap();
    thread::sleep(Duration::from_millis(300));
    tx.send(ProgressEvent::NodeStarted {
        recipe: RecipeId::new(0), node: NodeId::new(0),
        name: "fetch-a".into(), artifact: Some("build/deps/a.tar".into()),
        fallback_label: "fetch a".into(),
        kind: NodeKind::Cooked,
        cause: None,
    }).unwrap();
    thread::sleep(Duration::from_millis(400));
    tx.send(ProgressEvent::NodeCompleted {
        recipe: RecipeId::new(0), node: NodeId::new(0),
        elapsed: Duration::from_millis(400),
        kind: NodeKind::Cooked,
    }).unwrap();
    tx.send(ProgressEvent::RecipeCompleted {
        recipe: RecipeId::new(0),
        elapsed: Duration::from_millis(700),
        cached: 0, total: 2,
        kind: cook_progress::event::RecipeKind::Recipe,
    }).unwrap();
    thread::sleep(Duration::from_millis(200));
    tx.send(ProgressEvent::Finished { success: true }).unwrap();
    handle.join().unwrap();
}
