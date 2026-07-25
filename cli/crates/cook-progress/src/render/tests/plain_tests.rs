use super::*;
use crate::event::{NodeId, NodeKind, RecipeTopo};

fn topo(recipes: &[(u32, &str, usize)]) -> Vec<RecipeTopo> {
    recipes.iter().map(|(id, name, n)| RecipeTopo {
        id: RecipeId::new(*id),
        name: (*name).to_string(),
        deps: vec![],
        expected_nodes: *n,
    }).collect()
}

#[test]
fn build_started_writes_queued_lines() {
    let mut state = BuildState::new();
    let ev = ProgressEvent::BuildStarted {
        recipes: topo(&[(0, "deps", 2), (1, "lib", 3)]),
        total_nodes: 5,
    };
    state.apply(&ev);
    let mut buf = Vec::new();
    {
        let mut r = PlainRenderer::new(&mut buf);
        r.handle(&state, &ev).unwrap();
    }
    let s = String::from_utf8(buf).unwrap();
    assert!(s.contains("deps"));
    assert!(s.contains("queued  (2 nodes)"));
    assert!(s.contains("lib"));
}

#[test]
fn recipe_completed_writes_done_line() {
    let mut state = BuildState::new();
    state.apply(&ProgressEvent::BuildStarted {
        recipes: topo(&[(0, "deps", 2)]), total_nodes: 2,
    });
    let ev = ProgressEvent::RecipeCompleted {
        recipe: RecipeId::new(0),
        elapsed: Duration::from_millis(400),
        cached: 0, total: 2,
        kind: crate::event::RecipeKind::Recipe,
    };
    state.apply(&ev);
    let mut buf = Vec::new();
    {
        let mut r = PlainRenderer::new(&mut buf);
        r.handle(&state, &ev).unwrap();
    }
    let s = String::from_utf8(buf).unwrap();
    assert!(s.contains("deps"), "got: {s}");
    assert!(s.contains("done"), "got: {s}");
    assert!(s.contains("0.40s"), "got: {s}");
}

#[test]
fn recipe_skipped_writes_skipped_not_done_line() {
    let mut state = BuildState::new();
    state.apply(&ProgressEvent::BuildStarted {
        recipes: topo(&[(0, "report", 1)]), total_nodes: 1,
    });
    state.apply(&ProgressEvent::RecipeStarted {
        recipe: RecipeId::new(0),
    });
    let ev = ProgressEvent::RecipeSkipped {
        recipe: RecipeId::new(0),
        elapsed: Duration::from_millis(400),
        skipped: 1,
        completed: 0,
        total: 1,
    };
    state.apply(&ev);
    let mut buf = Vec::new();
    {
        let mut r = PlainRenderer::new(&mut buf);
        r.handle(&state, &ev).unwrap();
    }
    let s = String::from_utf8(buf).unwrap();
    assert!(s.contains("report"), "got: {s}");
    assert!(s.contains("skipped"), "got: {s}");
    assert!(s.contains("0/1 ran"), "got: {s}");
    assert!(!s.contains("done"), "got: {s}");
}

#[test]
fn node_output_prefix_includes_recipe_and_node() {
    // The live-stdout tag must use the node's own full output path
    // (its `display()` label) — not its raw node name/command text.
    let mut state = BuildState::new();
    state.apply(&ProgressEvent::BuildStarted {
        recipes: topo(&[(0, "lib", 1)]), total_nodes: 1,
    });
    state.apply(&ProgressEvent::NodeStarted {
        recipe: RecipeId::new(0), node: NodeId::new(0),
        name: "lvm.c".into(),
        artifact: Some(std::path::PathBuf::from("build/obj/lvm.o")),
        fallback_label: "x".into(),
        kind: crate::event::NodeKind::Cooked,
            cause: None,
        });
    let ev = ProgressEvent::NodeOutput {
        recipe: RecipeId::new(0), node: NodeId::new(0),
        line: "warning: unused".into(), stream: Stream::Stderr,
    };
    let mut buf = Vec::new();
    {
        let mut r = PlainRenderer::new(&mut buf);
        r.handle(&state, &ev).unwrap();
    }
    let s = String::from_utf8(buf).unwrap();
    assert!(s.contains("[lib/build/obj/lvm.o]"), "got: {s}");
    assert!(s.contains("(stderr)"), "got: {s}");
    assert!(s.contains("warning: unused"), "got: {s}");
}

#[test]
fn cache_hit_line_uses_full_output_path_not_raw_command() {
    // A held cached row, once flushed by real work in the same recipe,
    // must show the unit's own full declared output path, not the raw
    // shell command that produced it and not just the output's basename —
    // the distinguishing directory segment must survive.
    let mut state = BuildState::new();
    state.apply(&ProgressEvent::BuildStarted {
        recipes: topo(&[(0, "build", 2)]), total_nodes: 2,
    });
    state.apply(&ProgressEvent::NodeStarted {
        recipe: RecipeId::new(0), node: NodeId::new(0),
        name: "wc -w < a.txt > build/counts/alpha.count".into(),
        artifact: Some(std::path::PathBuf::from("build/counts/alpha.count")),
        fallback_label: "wc -w < a.txt > build/counts/alpha.count".into(),
        kind: crate::event::NodeKind::Cooked,
            cause: None,
        });
    let hit = ProgressEvent::NodeCacheHit {
        recipe: RecipeId::new(0), node: NodeId::new(0),
        name: "wc -w < a.txt > build/counts/alpha.count".into(),
        artifact: Some(std::path::PathBuf::from("build/counts/alpha.count")),
        kind: NodeKind::Cooked,
    };
    state.apply(&hit);
    state.apply(&ProgressEvent::NodeStarted {
        recipe: RecipeId::new(0), node: NodeId::new(1),
        name: "beta".into(),
        artifact: Some(std::path::PathBuf::from("build/counts/beta.count")),
        fallback_label: "wc -w < b.txt > build/counts/beta.count".into(),
        kind: crate::event::NodeKind::Cooked,
            cause: None,
        });
    let completed = ProgressEvent::NodeCompleted {
        recipe: RecipeId::new(0), node: NodeId::new(1),
        elapsed: Duration::from_millis(50),
        kind: crate::event::NodeKind::Cooked,
        cache_key: None,
    };
    let mut buf = Vec::new();
    {
        let mut r = PlainRenderer::new(&mut buf);
        r.handle(&state, &hit).unwrap();
        state.apply(&completed);
        r.handle(&state, &completed).unwrap();
    }
    let s = String::from_utf8(buf).unwrap();
    assert!(s.contains("build/build/counts/alpha.count"), "got: {s}");
    assert!(s.contains("cached"), "got: {s}");
    assert!(!s.contains("wc -w"), "raw command leaked into label: {s}");
}

#[test]
fn all_cached_recipe_drops_held_rows_keeps_summary() {
    let mut state = BuildState::new();
    state.apply(&ProgressEvent::BuildStarted {
        recipes: topo(&[(0, "deps", 2)]), total_nodes: 2,
    });
    let mut buf = Vec::new();
    {
        let mut r = PlainRenderer::new(&mut buf);
        for i in 0..2u32 {
            state.apply(&ProgressEvent::NodeStarted {
                recipe: RecipeId::new(0), node: NodeId::new(i),
                name: format!("a{i}.c"),
                artifact: Some(format!("a{i}.o").into()),
                fallback_label: format!("cc a{i}.c"),
                kind: crate::event::NodeKind::Compile,
            cause: None,
        });
            let hit = ProgressEvent::NodeCacheHit {
                recipe: RecipeId::new(0), node: NodeId::new(i),
                name: format!("a{i}.o"), artifact: Some(format!("a{i}.o").into()), kind: NodeKind::Cooked,
            };
            state.apply(&hit);
            r.handle(&state, &hit).unwrap();
        }
        let done = ProgressEvent::RecipeCompleted {
            recipe: RecipeId::new(0),
            elapsed: Duration::from_millis(5),
            cached: 2, total: 2,
            kind: crate::event::RecipeKind::Recipe,
        };
        state.apply(&done);
        r.handle(&state, &done).unwrap();
    }
    let s = String::from_utf8(buf).unwrap();
    assert_eq!(s.lines().count(), 1, "warm no-op recipe must be one row, got: {s}");
    assert!(s.contains("deps"), "got: {s}");
    assert!(s.contains("(2/2 cached)"), "got: {s}");
    assert!(!s.contains("a0.o"), "per-node cached rows must be dropped: {s}");
}

#[test]
fn queued_list_skips_zero_node_and_internal_recipes() {
    let mut state = BuildState::new();
    let ev = ProgressEvent::BuildStarted {
        recipes: topo(&[
            (0, "idLib", 3),
            (1, "game", 0),
            (2, "__cc_config_header__x", 1),
        ]),
        total_nodes: 4,
    };
    state.apply(&ev);
    let mut buf = Vec::new();
    {
        let mut r = PlainRenderer::new(&mut buf);
        r.handle(&state, &ev).unwrap();
    }
    let s = String::from_utf8(buf).unwrap();
    assert!(s.contains("idLib"), "got: {s}");
    assert!(!s.contains("game"), "zero-node aggregator must not queue: {s}");
    assert!(!s.contains("__cc"), "internal recipe must not queue: {s}");
}

#[test]
fn internal_recipe_node_row_uses_module_tag_and_no_done_row() {
    let mut state = BuildState::new();
    state.apply(&ProgressEvent::BuildStarted {
        recipes: topo(&[(0, "__cc_config_header__x", 1)]), total_nodes: 1,
    });
    state.apply(&ProgressEvent::NodeStarted {
        recipe: RecipeId::new(0), node: NodeId::new(0),
        name: "config.h".into(),
        artifact: Some(std::path::PathBuf::from("build/config.h")),
        fallback_label: "render config.h".into(),
        kind: crate::event::NodeKind::Generate,
            cause: None,
        });
    let completed = ProgressEvent::NodeCompleted {
        recipe: RecipeId::new(0), node: NodeId::new(0),
        elapsed: Duration::from_millis(10),
        kind: crate::event::NodeKind::Generate,
        cache_key: None,
    };
    let done = ProgressEvent::RecipeCompleted {
        recipe: RecipeId::new(0),
        elapsed: Duration::from_millis(15),
        cached: 0, total: 1,
        kind: crate::event::RecipeKind::Recipe,
    };
    let mut buf = Vec::new();
    {
        let mut r = PlainRenderer::new(&mut buf);
        state.apply(&completed);
        r.handle(&state, &completed).unwrap();
        state.apply(&done);
        r.handle(&state, &done).unwrap();
    }
    let s = String::from_utf8(buf).unwrap();
    assert!(s.contains("cc/build/config.h"), "got: {s}");
    assert!(!s.contains("__cc"), "raw minted name must not leak: {s}");
    assert!(!s.contains("done"), "internal recipes have no summary row: {s}");
}

#[test]
fn probes_collapse_to_one_row() {
    let mut state = BuildState::new();
    state.apply(&ProgressEvent::BuildStarted {
        recipes: topo(&[(0, "idLib", 3)]), total_nodes: 3,
    });
    let mut buf = Vec::new();
    {
        let mut r = PlainRenderer::new(&mut buf);
        for (i, key) in ["probe:cc:compiler:auto", "probe:cc:find:sdl2"].iter().enumerate() {
            state.apply(&ProgressEvent::NodeStarted {
                recipe: RecipeId::new(0), node: NodeId::new(i as u32),
                name: (*key).into(), artifact: None,
                fallback_label: (*key).into(),
                kind: crate::event::NodeKind::Resolve,
            cause: None,
        });
            let completed = ProgressEvent::NodeCompleted {
                recipe: RecipeId::new(0), node: NodeId::new(i as u32),
                elapsed: Duration::from_millis(10),
                kind: crate::event::NodeKind::Resolve,
                cache_key: None,
            };
            state.apply(&completed);
            r.handle(&state, &completed).unwrap();
        }
        state.apply(&ProgressEvent::NodeStarted {
            recipe: RecipeId::new(0), node: NodeId::new(2),
            name: "x.c".into(), artifact: Some("x.o".into()),
            fallback_label: "cc x.c".into(),
            kind: crate::event::NodeKind::Compile,
            cause: None,
        });
        let completed = ProgressEvent::NodeCompleted {
            recipe: RecipeId::new(0), node: NodeId::new(2),
            elapsed: Duration::from_millis(100),
            kind: crate::event::NodeKind::Compile,
            cache_key: None,
        };
        state.apply(&completed);
        r.handle(&state, &completed).unwrap();
    }
    let s = String::from_utf8(buf).unwrap();
    assert!(s.contains("idLib/probe:cc (2 probes)"), "got: {s}");
    assert!(!s.contains("probe:cc:compiler"), "raw probe keys must not leak: {s}");
    let probe_count = s.lines().filter(|l| l.contains("probe:")).count();
    assert_eq!(probe_count, 1, "got: {s}");
}

#[test]
fn interactive_label_drops_internal_line_tag() {
    // `@N` is an internal source-line tag; never expose it in frames.
    assert_eq!(interactive_label("greet", "@23"), "greet");
    assert_eq!(interactive_label("greet", "shell"), "greet/shell");
}
