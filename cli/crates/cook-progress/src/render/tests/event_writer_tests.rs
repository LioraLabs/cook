use super::*;
use crate::event::{NodeId, RecipeTopo};
use std::time::Duration;

fn empty_state() -> BuildState {
    let mut s = BuildState::new();
    s.apply(&ProgressEvent::BuildStarted {
        recipes: vec![RecipeTopo {
            id: RecipeId::new(0), name: "lib".into(), deps: vec![], expected_nodes: 1,
        }],
        total_nodes: 1,
    });
    s
}

fn render_one(state: &BuildState, ev: &ProgressEvent, opts: EventWriterOptions) -> String {
    let mut buf = Vec::new();
    let mut w = EventWriter::new(opts);
    w.handle(&mut buf, state, ev).unwrap();
    String::from_utf8(buf).unwrap()
}

#[test]
fn node_completed_compile_kind_emits_compiled_verb() {
    let mut state = empty_state();
    state.apply(&ProgressEvent::RecipeStarted { recipe: RecipeId::new(0) });
    state.apply(&ProgressEvent::NodeStarted {
        recipe: RecipeId::new(0), node: NodeId::new(0),
        name: "lvm.c".into(),
        artifact: Some("build/obj/liblua/lvm.o".into()),
        fallback_label: "clang -c lvm.c".into(),
        kind: NodeKind::Compile,
            cause: None,
            cache_key: None,
        });
    let ev = ProgressEvent::NodeCompleted {
        recipe: RecipeId::new(0), node: NodeId::new(0),
        elapsed: Duration::from_millis(880),
        kind: NodeKind::Compile,
        cache_key: None,
    };
    let opts = EventWriterOptions { colored: false, ..Default::default() };
    let out = render_one(&state, &ev, opts);
    // Full declared output path, not the artifact basename.
    assert_eq!(out, "    Compiled lib/build/obj/liblua/lvm.o in 0.88s\n");
}

/// Seed `state` + `w` with `n` cache hits (artifact-bearing) on recipe 0.
/// Mirrors the engine: a cached node gets no `NodeStarted`, the hit event
/// itself registers the node (and its artifact) in state.
fn apply_cache_hits(state: &mut BuildState, w: &mut EventWriter, buf: &mut Vec<u8>, n: u32) {
    for i in 0..n {
        let ev = ProgressEvent::NodeCacheHit {
            recipe: RecipeId::new(0), node: NodeId::new(i),
            name: format!("a{i}.o"), artifact: Some(format!("a{i}.o").into()), kind: NodeKind::Cooked,
        };
        state.apply(&ev);
        w.handle(buf, state, &ev).unwrap();
    }
}

fn deps_state(expected_nodes: usize) -> BuildState {
    let mut state = BuildState::new();
    state.apply(&ProgressEvent::BuildStarted {
        recipes: vec![RecipeTopo {
            id: RecipeId::new(0), name: "deps".into(), deps: vec![], expected_nodes,
        }],
        total_nodes: expected_nodes,
    });
    state
}

#[test]
fn cached_lines_held_until_real_work_then_collapse_after_threshold() {
    let mut state = deps_state(12);
    let mut buf = Vec::new();
    let mut w = EventWriter::new(EventWriterOptions { colored: false, cached_inline_threshold: 3, ..Default::default() });

    apply_cache_hits(&mut state, &mut w, &mut buf, 6);
    // Nothing prints while the recipe might still be a no-op.
    assert!(buf.is_empty(), "cached lines must be held, got: {}", String::from_utf8_lossy(&buf));

    // Real work arrives — the held lines flush in front of it.
    let started = ProgressEvent::NodeStarted {
        recipe: RecipeId::new(0), node: NodeId::new(6),
        name: "b.c".into(), artifact: Some("b.o".into()),
        fallback_label: "cc b.c".into(), kind: NodeKind::Compile,
            cause: None,
            cache_key: None,
        };
    state.apply(&started);
    let ev = ProgressEvent::NodeCompleted {
        recipe: RecipeId::new(0), node: NodeId::new(6),
        elapsed: Duration::from_millis(120), kind: NodeKind::Compile,
        cache_key: None,
    };
    state.apply(&ev);
    w.handle(&mut buf, &state, &ev).unwrap();

    let out = String::from_utf8(buf.clone()).unwrap();
    let cached_lines = out.lines().filter(|l| l.contains("Cached")).count();
    assert_eq!(cached_lines, 3, "got: {out}");
    assert!(out.contains("Compiled deps/b.o"), "got: {out}");
    let compiled_pos = out.find("Compiled").unwrap();
    let cached_pos = out.find("Cached").unwrap();
    assert!(cached_pos < compiled_pos, "held lines must flush before the trigger: {out}");
    // The collapse count is deferred to the recipe's final flush — one
    // report per recipe, not one per flush burst.
    assert!(!out.contains("more cached"), "got: {out}");

    let done = ProgressEvent::RecipeCompleted {
        recipe: RecipeId::new(0),
        elapsed: Duration::from_millis(200),
        cached: 6, total: 7,
        kind: crate::event::RecipeKind::Recipe,
    };
    state.apply(&done);
    w.handle(&mut buf, &state, &done).unwrap();
    let out = String::from_utf8(buf).unwrap();
    assert!(out.contains("(3 more cached)"), "got: {out}");
    assert!(out.contains("Finished deps"), "got: {out}");
}

#[test]
fn all_cached_recipe_collapses_to_single_line() {
    let mut state = deps_state(6);
    let mut buf = Vec::new();
    let mut w = EventWriter::new(EventWriterOptions { colored: false, ..Default::default() });

    apply_cache_hits(&mut state, &mut w, &mut buf, 6);
    let ev = ProgressEvent::RecipeCompleted {
        recipe: RecipeId::new(0),
        elapsed: Duration::from_millis(400),
        cached: 6, total: 6,
        kind: crate::event::RecipeKind::Recipe,
    };
    state.apply(&ev);
    w.handle(&mut buf, &state, &ev).unwrap();

    let out = String::from_utf8(buf).unwrap();
    assert_eq!(out.lines().count(), 1, "warm no-op recipe must be one line, got: {out}");
    assert!(out.contains("Cached deps (6 nodes)"), "got: {out}");
    assert!(!out.contains("a0.o"), "per-node cached lines must be dropped: {out}");
    assert!(!out.contains("Finished"), "got: {out}");
}

#[test]
fn probes_group_into_single_resolved_line() {
    let mut state = deps_state(3);
    let mut buf = Vec::new();
    let mut w = EventWriter::new(EventWriterOptions { colored: false, ..Default::default() });

    for (i, key) in ["probe:cc:compiler:auto", "probe:cc:find:sdl2"].iter().enumerate() {
        let started = ProgressEvent::NodeStarted {
            recipe: RecipeId::new(0), node: NodeId::new(i as u32),
            name: (*key).into(), artifact: None,
            fallback_label: (*key).into(), kind: NodeKind::Resolve,
            cause: None,
            cache_key: None,
        };
        state.apply(&started);
        let ev = ProgressEvent::NodeCompleted {
            recipe: RecipeId::new(0), node: NodeId::new(i as u32),
            elapsed: Duration::from_millis(10), kind: NodeKind::Resolve,
            cache_key: None,
        };
        state.apply(&ev);
        w.handle(&mut buf, &state, &ev).unwrap();
    }
    assert!(buf.is_empty(), "probes must group, got: {}", String::from_utf8_lossy(&buf));

    // A real node flushes the group in front of itself.
    let started = ProgressEvent::NodeStarted {
        recipe: RecipeId::new(0), node: NodeId::new(2),
        name: "x.c".into(), artifact: Some("x.o".into()),
        fallback_label: "cc x.c".into(), kind: NodeKind::Compile,
            cause: None,
            cache_key: None,
        };
    state.apply(&started);
    let ev = ProgressEvent::NodeCompleted {
        recipe: RecipeId::new(0), node: NodeId::new(2),
        elapsed: Duration::from_millis(100), kind: NodeKind::Compile,
        cache_key: None,
    };
    state.apply(&ev);
    w.handle(&mut buf, &state, &ev).unwrap();

    let out = String::from_utf8(buf).unwrap();
    assert!(out.contains("Resolved cc toolchain for deps (2 probes) in 0.02s"), "got: {out}");
    assert!(!out.contains("probe:cc:compiler"), "raw probe keys must not leak: {out}");
}

#[test]
fn fully_cached_probe_set_stays_silent() {
    let mut state = deps_state(2);
    let mut buf = Vec::new();
    let mut w = EventWriter::new(EventWriterOptions { colored: false, ..Default::default() });

    let started = ProgressEvent::NodeStarted {
        recipe: RecipeId::new(0), node: NodeId::new(0),
        name: "probe:cc:compiler:auto".into(), artifact: None,
        fallback_label: "probe:cc:compiler:auto".into(), kind: NodeKind::Resolve,
            cause: None,
            cache_key: None,
        };
    state.apply(&started);
    let hit = ProgressEvent::NodeCacheHit {
        recipe: RecipeId::new(0), node: NodeId::new(0),
        name: "probe:cc:compiler:auto".into(), artifact: None, kind: NodeKind::Cooked,
    };
    state.apply(&hit);
    w.handle(&mut buf, &state, &hit).unwrap();

    let done = ProgressEvent::RecipeCompleted {
        recipe: RecipeId::new(0),
        elapsed: Duration::from_millis(5),
        cached: 2, total: 2,
        kind: crate::event::RecipeKind::Recipe,
    };
    state.apply(&done);
    w.handle(&mut buf, &state, &done).unwrap();

    let out = String::from_utf8(buf).unwrap();
    assert!(!out.contains("Resolved"), "cached probes must stay silent: {out}");
    assert!(out.contains("Cached deps (2 nodes)"), "got: {out}");
}

#[test]
fn probes_only_work_still_collapses_recipe_summary() {
    // Probes re-ran but every real node was cached: still a warm no-op —
    // one Resolved line plus the dim collapsed summary.
    let mut state = deps_state(3);
    let mut buf = Vec::new();
    let mut w = EventWriter::new(EventWriterOptions { colored: false, ..Default::default() });

    apply_cache_hits(&mut state, &mut w, &mut buf, 2);
    let started = ProgressEvent::NodeStarted {
        recipe: RecipeId::new(0), node: NodeId::new(2),
        name: "probe:cc:compiler:auto".into(), artifact: None,
        fallback_label: "probe:cc:compiler:auto".into(), kind: NodeKind::Resolve,
            cause: None,
            cache_key: None,
        };
    state.apply(&started);
    let probe = ProgressEvent::NodeCompleted {
        recipe: RecipeId::new(0), node: NodeId::new(2),
        elapsed: Duration::from_millis(20), kind: NodeKind::Resolve,
        cache_key: None,
    };
    state.apply(&probe);
    w.handle(&mut buf, &state, &probe).unwrap();

    let done = ProgressEvent::RecipeCompleted {
        recipe: RecipeId::new(0),
        elapsed: Duration::from_millis(30),
        cached: 2, total: 3,
        kind: crate::event::RecipeKind::Recipe,
    };
    state.apply(&done);
    w.handle(&mut buf, &state, &done).unwrap();

    let out = String::from_utf8(buf).unwrap();
    assert!(out.contains("Resolved cc toolchain for deps (1 probe) in 0.02s"), "got: {out}");
    assert!(out.contains("Cached deps (3 nodes)"), "got: {out}");
    assert!(!out.contains("a0.o"), "held cached lines must be dropped: {out}");
    assert!(!out.contains("Finished"), "got: {out}");
}

#[test]
fn internal_recipe_shows_module_tag_and_no_summary() {
    let mut state = BuildState::new();
    state.apply(&ProgressEvent::BuildStarted {
        recipes: vec![RecipeTopo {
            id: RecipeId::new(0),
            name: "__cc_config_header__build_dhewm3_config_h".into(),
            deps: vec![], expected_nodes: 1,
        }],
        total_nodes: 1,
    });
    let mut buf = Vec::new();
    let mut w = EventWriter::new(EventWriterOptions { colored: false, ..Default::default() });

    let started = ProgressEvent::NodeStarted {
        recipe: RecipeId::new(0), node: NodeId::new(0),
        name: "config.h".into(),
        artifact: Some("build/dhewm3/config.h".into()),
        fallback_label: "render config.h".into(), kind: NodeKind::Generate,
            cause: None,
            cache_key: None,
        };
    state.apply(&started);
    let ev = ProgressEvent::NodeCompleted {
        recipe: RecipeId::new(0), node: NodeId::new(0),
        elapsed: Duration::from_millis(10), kind: NodeKind::Generate,
        cache_key: None,
    };
    state.apply(&ev);
    w.handle(&mut buf, &state, &ev).unwrap();

    let done = ProgressEvent::RecipeCompleted {
        recipe: RecipeId::new(0),
        elapsed: Duration::from_millis(15),
        cached: 0, total: 1,
        kind: crate::event::RecipeKind::Recipe,
    };
    state.apply(&done);
    w.handle(&mut buf, &state, &done).unwrap();

    let out = String::from_utf8(buf).unwrap();
    assert!(out.contains("Generated cc/build/dhewm3/config.h"), "got: {out}");
    assert!(!out.contains("__cc_config_header"), "raw minted name must not leak: {out}");
    assert!(!out.contains("Finished"), "internal recipes have no summary row: {out}");
}

#[test]
fn node_started_with_cause_prints_rebuilding_line() {
    let mut state = empty_state();
    state.apply(&ProgressEvent::RecipeStarted { recipe: RecipeId::new(0) });
    let ev = ProgressEvent::NodeStarted {
        recipe: RecipeId::new(0), node: NodeId::new(0),
        name: "next build".into(),
        artifact: Some("apps/web/.next".into()),
        fallback_label: "next build".into(),
        kind: NodeKind::Cooked,
        cause: Some("input changed: apps/web/app/.well-known/workflow/v1/manifest.json (+2 more)".into()),
        cache_key: None,
    };
    state.apply(&ev);
    let opts = EventWriterOptions { colored: false, ..Default::default() };
    let out = render_one(&state, &ev, opts);
    assert_eq!(
        out,
        "  Rebuilding lib/apps/web/.next — input changed: apps/web/app/.well-known/workflow/v1/manifest.json (+2 more)\n",
        "got: {out:?}"
    );
}

#[test]
fn node_started_without_cause_stays_silent() {
    let mut state = empty_state();
    state.apply(&ProgressEvent::RecipeStarted { recipe: RecipeId::new(0) });
    let ev = ProgressEvent::NodeStarted {
        recipe: RecipeId::new(0), node: NodeId::new(0),
        name: "x.c".into(), artifact: Some("x.o".into()),
        fallback_label: "cc x.c".into(), kind: NodeKind::Compile,
        cause: None,
        cache_key: None,
    };
    state.apply(&ev);
    let opts = EventWriterOptions { colored: false, ..Default::default() };
    let out = render_one(&state, &ev, opts);
    assert_eq!(out, "", "cold start must not print an attribution line");
}

#[test]
fn cause_line_flushes_held_cached_lines_first() {
    let mut state = deps_state(3);
    let mut buf = Vec::new();
    let mut w = EventWriter::new(EventWriterOptions { colored: false, ..Default::default() });
    apply_cache_hits(&mut state, &mut w, &mut buf, 2);
    let ev = ProgressEvent::NodeStarted {
        recipe: RecipeId::new(0), node: NodeId::new(2),
        name: "c.c".into(), artifact: Some("c.o".into()),
        fallback_label: "cc c.c".into(), kind: NodeKind::Compile,
        cause: Some("input changed: c.c".into()),
        cache_key: None,
    };
    state.apply(&ev);
    w.handle(&mut buf, &state, &ev).unwrap();
    let out = String::from_utf8(buf).unwrap();
    let cached_pos = out.find("Cached").expect("held cached lines flushed");
    let rebuild_pos = out.find("Rebuilding").expect("cause line printed");
    assert!(cached_pos < rebuild_pos, "got: {out}");
}

#[test]
fn finished_all_cached_says_all_cached() {
    let mut state = deps_state(6);
    let mut w = EventWriter::new(EventWriterOptions { colored: false, ..Default::default() });
    let mut buf = Vec::new();
    apply_cache_hits(&mut state, &mut w, &mut buf, 6);
    let done = ProgressEvent::RecipeCompleted {
        recipe: RecipeId::new(0),
        elapsed: Duration::from_millis(5),
        cached: 6, total: 6,
        kind: crate::event::RecipeKind::Recipe,
    };
    state.apply(&done);
    w.handle(&mut buf, &state, &done).unwrap();
    let fin = ProgressEvent::Finished { success: true };
    state.apply(&fin);
    w.handle(&mut buf, &state, &fin).unwrap();
    let out = String::from_utf8(buf).unwrap();
    assert!(out.contains("all cached"), "got: {out}");
}

#[test]
fn node_failed_dumps_indented_stderr() {
    let mut state = empty_state();
    state.apply(&ProgressEvent::RecipeStarted { recipe: RecipeId::new(0) });
    state.apply(&ProgressEvent::NodeStarted {
        recipe: RecipeId::new(0), node: NodeId::new(0),
        name: "lvm.c".into(), artifact: None,
        fallback_label: "clang lvm.c".into(),
        kind: NodeKind::Compile,
            cause: None,
            cache_key: None,
        });
    let ev = ProgressEvent::NodeFailed {
        recipe: RecipeId::new(0), node: NodeId::new(0),
        elapsed: Duration::from_millis(1820),
        error: "lvm.c:42:9: error: 'bar' was not declared\n    int foo = bar(x);".into(),
    };
    let opts = EventWriterOptions { colored: false, ..Default::default() };
    let out = render_one(&state, &ev, opts);
    let expected = "      Failed lib/$clang in 1.82s\n               lvm.c:42:9: error: 'bar' was not declared\n                   int foo = bar(x);\n";
    assert_eq!(out, expected, "got: {out}");
}

#[test]
fn quiet_suppresses_per_node_lines_but_keeps_recipe_summary() {
    let mut state = empty_state();
    state.apply(&ProgressEvent::RecipeStarted { recipe: RecipeId::new(0) });
    let opts = EventWriterOptions { colored: false, quiet: true, ..Default::default() };

    let started = ProgressEvent::NodeStarted {
        recipe: RecipeId::new(0), node: NodeId::new(0),
        name: "x.c".into(), artifact: Some("x.o".into()),
        fallback_label: "x".into(), kind: NodeKind::Compile,
            cause: None,
            cache_key: None,
        };
    state.apply(&started);
    let completed = ProgressEvent::NodeCompleted {
        recipe: RecipeId::new(0), node: NodeId::new(0),
        elapsed: Duration::from_millis(100), kind: NodeKind::Compile,
        cache_key: None,
    };
    state.apply(&completed);

    let mut buf = Vec::new();
    let mut w = EventWriter::new(opts);
    w.handle(&mut buf, &state, &completed).unwrap();

    let recipe_done = ProgressEvent::RecipeCompleted {
        recipe: RecipeId::new(0),
        elapsed: Duration::from_millis(200),
        cached: 0, total: 1,
        kind: crate::event::RecipeKind::Recipe,
    };
    state.apply(&recipe_done);
    w.handle(&mut buf, &state, &recipe_done).unwrap();

    let out = String::from_utf8(buf).unwrap();
    assert!(!out.contains("Compiled"), "quiet should suppress per-node verbs: {out}");
    assert!(out.contains("Finished lib"), "got: {out}");
}

#[test]
fn verbose_emits_node_output_lines() {
    // The live-stdout tag must use the node's own full output path
    // (its `display()` label) — not its raw node name/command text.
    let mut state = empty_state();
    state.apply(&ProgressEvent::RecipeStarted { recipe: RecipeId::new(0) });
    state.apply(&ProgressEvent::NodeStarted {
        recipe: RecipeId::new(0), node: NodeId::new(0),
        name: "lvm.c".into(),
        artifact: Some("build/obj/lvm.o".into()),
        fallback_label: "x".into(),
        kind: NodeKind::Compile,
            cause: None,
            cache_key: None,
        });
    let ev = ProgressEvent::NodeOutput {
        recipe: RecipeId::new(0), node: NodeId::new(0),
        line: "warning: unused".into(), stream: Stream::Stderr,
    };
    let opts = EventWriterOptions { colored: false, verbose: true, ..Default::default() };
    let out = render_one(&state, &ev, opts);
    assert_eq!(out, "[lib/build/obj/lvm.o] (stderr) warning: unused\n");
}

#[test]
fn finished_success_emits_subjectless_summary() {
    let mut state = empty_state();
    state.totals.completed_nodes = 47;
    let ev = ProgressEvent::Finished { success: true };
    let opts = EventWriterOptions { colored: false, ..Default::default() };
    let out = render_one(&state, &ev, opts);
    // No "build" subject, no collision with a recipe of the same name.
    assert!(out.starts_with("    Finished in "), "got: {out}");
    assert!(out.contains("(47 nodes, 0 cached)"), "got: {out}");
}

#[test]
fn upstream_failed_skips_collapse_to_one_line() {
    let mut state = BuildState::new();
    state.apply(&ProgressEvent::BuildStarted {
        recipes: vec![
            RecipeTopo { id: RecipeId::new(1), name: "lua".into(), deps: vec![], expected_nodes: 2 },
            RecipeTopo { id: RecipeId::new(2), name: "luac".into(), deps: vec![], expected_nodes: 2 },
        ],
        total_nodes: 4,
    });

    let mut buf = Vec::new();
    let mut w = EventWriter::new(EventWriterOptions { colored: false, ..Default::default() });

    for (rid, n) in [(1u32, "lua.o"), (1, "lua"), (2, "luac.o"), (2, "luac")] {
        let ev = ProgressEvent::NodeSkipped {
            recipe: RecipeId::new(rid), node: NodeId::new(0),
            name: n.into(), reason: SkipReason::UpstreamFailed,
        };
        state.apply(&ev);
        w.handle(&mut buf, &state, &ev).unwrap();
    }
    let fin = ProgressEvent::Finished { success: false };
    state.apply(&fin);
    w.handle(&mut buf, &state, &fin).unwrap();

    let out = String::from_utf8(buf).unwrap();
    let skipped_lines = out.lines().filter(|l| l.contains("Skipped")).count();
    assert_eq!(skipped_lines, 1, "expected 1 collapsed line, got: {out}");
    assert!(out.contains("upstream failed"), "got: {out}");
}

#[test]
fn terminal_interactive_end_suppresses_subsequent_output() {
    let mut state = empty_state();
    state.apply(&ProgressEvent::RecipeStarted { recipe: RecipeId::new(0) });

    let opts = EventWriterOptions { colored: false, ..Default::default() };
    let mut w = EventWriter::new(opts);
    let mut buf = Vec::new();

    // Chore-style sequence: InteractiveStart → InteractiveEnd(terminal) → trailing events.
    let start = ProgressEvent::InteractiveStart {
        recipe: RecipeId::new(0), node: NodeId::new(0), name: "@45".into(),
        chore_step_count: 0,
    };
    state.apply(&start);
    w.handle(&mut buf, &state, &start).unwrap();

    let end = ProgressEvent::InteractiveEnd {
        recipe: RecipeId::new(0), node: NodeId::new(0), name: "@45".into(),
        elapsed: Duration::from_millis(10),
        success: true,
        is_terminal: true,
        failed_step: None,
    };
    state.apply(&end);
    w.handle(&mut buf, &state, &end).unwrap();

    // These would normally print but should be suppressed after a terminal chore end.
    for ev in [
        ProgressEvent::NodeCompleted {
            recipe: RecipeId::new(0), node: NodeId::new(0),
            elapsed: Duration::from_millis(10), kind: NodeKind::Cooked,
            cache_key: None,
        },
        ProgressEvent::RecipeCompleted {
            recipe: RecipeId::new(0),
            elapsed: Duration::from_millis(15),
            cached: 0, total: 1,
            kind: crate::event::RecipeKind::Recipe,
        },
        ProgressEvent::Finished { success: true },
    ] {
        state.apply(&ev);
        w.handle(&mut buf, &state, &ev).unwrap();
    }

    let out = String::from_utf8(buf).unwrap();
    // Only one Running line; nothing else.
    let line_count = out.lines().count();
    assert_eq!(line_count, 1, "expected only the Running line; got: {out}");
    assert!(out.contains("Running"), "got: {out}");
    assert!(!out.contains("Cooked"), "Cooked should be suppressed: {out}");
    assert!(!out.contains("Finished"), "Finished should be suppressed: {out}");
}

#[test]
fn node_completed_no_artifact_emits_no_line() {
    let mut state = empty_state();
    state.apply(&ProgressEvent::RecipeStarted { recipe: RecipeId::new(0) });
    state.apply(&ProgressEvent::NodeStarted {
        recipe: RecipeId::new(0), node: NodeId::new(0),
        name: "@45".into(),
        artifact: None,
        fallback_label: "@45".into(),
        kind: NodeKind::Cooked,
            cause: None,
            cache_key: None,
        });
    let ev = ProgressEvent::NodeCompleted {
        recipe: RecipeId::new(0), node: NodeId::new(0),
        elapsed: Duration::from_millis(100),
        kind: NodeKind::Cooked,
        cache_key: None,
    };
    let opts = EventWriterOptions { colored: false, ..Default::default() };
    let out = render_one(&state, &ev, opts);
    assert_eq!(out, "", "anonymous shell step (no artifact) must emit nothing, got: {out:?}");
}

#[test]
fn node_completed_no_artifact_verbose_still_prints() {
    let mut state = empty_state();
    state.apply(&ProgressEvent::RecipeStarted { recipe: RecipeId::new(0) });
    state.apply(&ProgressEvent::NodeStarted {
        recipe: RecipeId::new(0), node: NodeId::new(0),
        name: "@45".into(), artifact: None, fallback_label: "@45".into(),
        kind: NodeKind::Cooked,
            cause: None,
            cache_key: None,
        });
    let ev = ProgressEvent::NodeCompleted {
        recipe: RecipeId::new(0), node: NodeId::new(0),
        elapsed: Duration::from_millis(100),
        kind: NodeKind::Cooked,
        cache_key: None,
    };
    let opts = EventWriterOptions { colored: false, verbose: true, ..Default::default() };
    let out = render_one(&state, &ev, opts);
    assert!(out.contains("Cooked"), "verbose path should still print Cooked line, got: {out:?}");
}

#[test]
fn node_cache_hit_no_artifact_emits_no_line() {
    let mut state = empty_state();
    state.apply(&ProgressEvent::RecipeStarted { recipe: RecipeId::new(0) });
    state.apply(&ProgressEvent::NodeStarted {
        recipe: RecipeId::new(0), node: NodeId::new(0),
        name: "@45".into(), artifact: None, fallback_label: "@45".into(),
        kind: NodeKind::Cooked,
            cause: None,
            cache_key: None,
        });
    let ev = ProgressEvent::NodeCacheHit {
        recipe: RecipeId::new(0), node: NodeId::new(0),
        name: "@45".into(),
        artifact: None,
        kind: NodeKind::Cooked,
    };
    let opts = EventWriterOptions { colored: false, ..Default::default() };
    let out = render_one(&state, &ev, opts);
    assert_eq!(out, "");
}

#[test]
fn recipe_completed_zero_nodes_emits_no_line() {
    let mut state = empty_state();
    state.apply(&ProgressEvent::RecipeStarted { recipe: RecipeId::new(0) });
    let ev = ProgressEvent::RecipeCompleted {
        recipe: RecipeId::new(0),
        elapsed: Duration::from_millis(0),
        cached: 0, total: 0,
        kind: crate::event::RecipeKind::Recipe,
    };
    let opts = EventWriterOptions { colored: false, ..Default::default() };
    let out = render_one(&state, &ev, opts);
    assert_eq!(out, "", "aggregator (total=0) must emit nothing, got: {out:?}");
}

#[test]
fn recipe_completed_one_node_still_prints() {
    let mut state = empty_state();
    state.apply(&ProgressEvent::RecipeStarted { recipe: RecipeId::new(0) });
    let ev = ProgressEvent::RecipeCompleted {
        recipe: RecipeId::new(0),
        elapsed: Duration::from_millis(100),
        cached: 0, total: 1,
        kind: crate::event::RecipeKind::Recipe,
    };
    let opts = EventWriterOptions { colored: false, ..Default::default() };
    let out = render_one(&state, &ev, opts);
    assert!(out.contains("Finished lib"), "single-node recipe should still print, got: {out:?}");
}

#[test]
fn recipe_completed_chore_kind_uses_chore_detail() {
    let mut state = empty_state();
    state.apply(&ProgressEvent::RecipeStarted { recipe: RecipeId::new(0) });
    let ev = ProgressEvent::RecipeCompleted {
        recipe: RecipeId::new(0),
        elapsed: Duration::from_millis(4910),
        cached: 0, total: 4,
        kind: crate::event::RecipeKind::Chore,
    };
    let opts = EventWriterOptions { colored: false, ..Default::default() };
    let out = render_one(&state, &ev, opts);
    assert!(out.contains("(chore)"), "chore recipe summary should show (chore), got: {out:?}");
    assert!(!out.contains("nodes"), "chore detail must not mention node math, got: {out:?}");
}

#[test]
fn chore_window_failure_renders_step_index_and_chore_name() {
    let mut state = empty_state();
    state.apply(&ProgressEvent::RecipeStarted { recipe: RecipeId::new(0) });
    // Apply NodeStarted with the chore name as both name and (no) artifact.
    // In the real engine flow, the chore-window failure path emits NodeFailed
    // with `name = chore_recipe`; here we synthesize that view of the state.
    state.apply(&ProgressEvent::NodeStarted {
        recipe: RecipeId::new(0),
        node: NodeId::new(0),
        name: "play".into(),
        artifact: None,
        fallback_label: "play".into(),
        kind: NodeKind::Cooked,
            cause: None,
            cache_key: None,
        });
    let ev = ProgressEvent::NodeFailed {
        recipe: RecipeId::new(0),
        node: NodeId::new(0),
        elapsed: Duration::from_millis(400),
        error: "step 2/4: exit 130".into(),
    };
    let opts = EventWriterOptions { colored: false, ..Default::default() };
    let out = render_one(&state, &ev, opts);
    // node_display for an artifact-less node with fallback_label "play" (no leading '$'):
    // stripped = "play", first = "play", not starting with '@', so returns "$play".
    assert!(out.contains("Failed lib/$play") || out.contains("Failed lib/play"),
        "expected 'Failed lib/$play' (with optional $-prefix from node_display fallback), got: {out:?}");
    assert!(out.contains("step 2/4: exit 130"), "got: {out:?}");
}

#[test]
fn interactive_start_with_at_tag_drops_the_tag() {
    let mut state = empty_state();
    state.apply(&ProgressEvent::RecipeStarted { recipe: RecipeId::new(0) });

    let ev = ProgressEvent::InteractiveStart {
        recipe: RecipeId::new(0), node: NodeId::new(0), name: "@45".into(),
        chore_step_count: 0,
    };
    let opts = EventWriterOptions { colored: false, ..Default::default() };
    let out = render_one(&state, &ev, opts);
    // Should be "Running lib" (the recipe name in empty_state), not "Running lib/@45".
    assert_eq!(out, "     Running lib\n", "got: {out:?}");
}
