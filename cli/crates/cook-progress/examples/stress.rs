use std::collections::BTreeMap;
use std::io::{self, Write};
use std::thread;
use std::time::{Duration, Instant};

use cook_progress::{Frame, ItemStatus, RenderConfig, Renderer, Section, Status};

/// Simulated recipe with multiple nodes
struct SimRecipe {
    name: String,
    nodes: Vec<String>,
    total_ticks: usize,    // how many ticks to complete
    start_tick: usize,     // when this recipe starts
    cache_ratio: f32,      // 0.0-1.0, fraction of nodes that are cache hits
    fail_at: Option<usize>, // if Some, fail at this node index
}

struct SimState {
    completed: usize,
    active: Vec<String>,
    started: bool,
    finished: bool,
    failed: bool,
    cached_count: usize,
}

fn main() -> io::Result<()> {
    let mut stdout = io::stderr();
    let (cols, _) = crossterm::terminal::size().unwrap_or((120, 40));
    let mut renderer = Renderer::new(RenderConfig {
        width: cols,
        max_output_lines: 2,
        colors: true,
        ..Default::default()
    });

    write!(stdout, "\x1b[?25l")?; // hide cursor

    // Build a massive simulated project
    let recipes = build_project();
    let total_recipes = recipes.len();
    let max_tick = recipes.iter().map(|r| r.start_tick + r.total_ticks).max().unwrap_or(0) + 5;

    let mut states: BTreeMap<String, SimState> = BTreeMap::new();
    for r in &recipes {
        states.insert(r.name.clone(), SimState {
            completed: 0,
            active: Vec::new(),
            started: false,
            finished: false,
            failed: false,
            cached_count: 0,
        });
    }

    let start = Instant::now();
    let tick_ms: u64 = 40; // fast ticks for drama

    for tick in 0..max_tick {
        // Update simulation state
        for recipe in &recipes {
            let state = states.get_mut(&recipe.name).unwrap();
            if state.finished { continue; }

            if tick < recipe.start_tick {
                continue; // not started yet
            }

            if !state.started {
                state.started = true;
            }

            let total = recipe.nodes.len();
            let progress_tick = tick - recipe.start_tick;
            let ticks_per_node = if total > 0 { recipe.total_ticks / total } else { 1 };

            // Complete nodes over time
            let should_complete = if ticks_per_node > 0 {
                (progress_tick / ticks_per_node).min(total)
            } else {
                total
            };

            while state.completed < should_complete {
                let node_idx = state.completed;

                // Check for failure
                if recipe.fail_at == Some(node_idx) {
                    state.failed = true;
                    state.finished = true;
                    let node_name = &recipe.nodes[node_idx];
                    renderer.push_output(&recipe.name, &format!("error: {node_name} failed to compile"));
                    renderer.push_output(&recipe.name, "  --> src/generated.rs:42:5");
                    renderer.push_output(&recipe.name, "  = note: expected type `Result<(), Error>`");
                    renderer.set_error(&recipe.name);
                    break;
                }

                // Check for cache hit
                let is_cached = (node_idx as f32 / total as f32) < recipe.cache_ratio;
                if is_cached {
                    state.cached_count += 1;
                }

                // Simulate output
                if !is_cached && node_idx % 3 == 0 {
                    let node_name = &recipe.nodes[node_idx];
                    renderer.push_output(&recipe.name, &format!("compiling {node_name}"));
                }

                state.completed += 1;
            }

            if state.failed { continue; }

            // Update active nodes
            state.active.clear();
            let active_start = state.completed;
            let active_end = (active_start + 3).min(total);
            for i in active_start..active_end {
                state.active.push(recipe.nodes[i].clone());
            }

            // Check if done
            if state.completed >= total && progress_tick >= recipe.total_ticks {
                state.finished = true;
                state.active.clear();
            }
        }

        // Build frame
        let mut frame = Frame::new();
        for recipe in &recipes {
            let state = &states[&recipe.name];
            let total = recipe.nodes.len();
            let mut section = Section::new(&recipe.name, &recipe.name);

            if !state.started {
                section = section.status(Status::Waiting);
            } else if state.finished && state.failed {
                section = section
                    .status(Status::Failed)
                    .progress(state.completed, total)
                    .elapsed(Duration::from_millis(tick_ms * (tick - recipe.start_tick) as u64));
            } else if state.finished {
                let all_cached = state.cached_count == total;
                if all_cached {
                    section = section
                        .status(Status::Cached)
                        .progress(total, total);
                } else {
                    section = section
                        .status(Status::Completed)
                        .progress(total, total)
                        .elapsed(Duration::from_millis(tick_ms * recipe.total_ticks as u64));
                    if state.cached_count > 0 {
                        section = section.cache(state.cached_count, total);
                    }
                }
            } else {
                section = section
                    .status(Status::Running)
                    .progress(state.completed, total)
                    .elapsed(Duration::from_millis(tick_ms * (tick - recipe.start_tick) as u64));
                for node in &state.active {
                    section = section.active_item(node, ItemStatus::Running);
                }
            }

            frame = frame.section(section);
        }

        // Footer
        let done = states.values().filter(|s| s.finished && !s.failed).count();
        let failed = states.values().filter(|s| s.failed).count();
        let running = states.values().filter(|s| s.started && !s.finished).count();
        let waiting = total_recipes - done - failed - running;
        let cached = states.values().map(|s| s.cached_count).sum::<usize>();
        let total_nodes: usize = recipes.iter().map(|r| r.nodes.len()).sum();

        let mut parts = Vec::new();
        if done > 0 { parts.push(format!("{done} done")); }
        if running > 0 { parts.push(format!("{running} running")); }
        if failed > 0 { parts.push(format!("{failed} failed")); }
        if waiting > 0 { parts.push(format!("{waiting} waiting")); }
        if cached > 0 { parts.push(format!("{cached}/{total_nodes} cached")); }
        frame = frame.footer(parts.join(" · "));

        renderer.clear_last_frame(&mut stdout)?;
        renderer.render_frame(&frame, &mut stdout)?;
        stdout.flush()?;
        thread::sleep(Duration::from_millis(tick_ms));
    }

    write!(stdout, "\x1b[?25h")?; // show cursor
    stdout.flush()?;

    eprintln!("\n  Completed in {:.1}s", start.elapsed().as_secs_f64());
    Ok(())
}

fn build_project() -> Vec<SimRecipe> {
    let mut recipes = Vec::new();
    let mut tick_offset;

    // ── Wave 1: Foundation libs (start immediately, fast) ─────────────
    for i in 0..8 {
        let name = format!("foundation-{}", (b'a' + i as u8) as char);
        let node_count = 3 + (i % 4);
        recipes.push(SimRecipe {
            name: name.clone(),
            nodes: (0..node_count).map(|n| format!("{name}/src/mod_{n}.rs")).collect(),
            total_ticks: 15 + (i * 2),
            start_tick: 0,
            cache_ratio: if i < 3 { 1.0 } else { 0.0 }, // first 3 are fully cached
            fail_at: None,
        });
    }
    tick_offset = 10;

    // ── Wave 2: Core libraries (start after foundation, medium) ───────
    for i in 0..12 {
        let name = format!("core-{}", match i {
            0 => "parser",   1 => "lexer",     2 => "codegen",
            3 => "optimizer", 4 => "linker",   5 => "resolver",
            6 => "typechk",  7 => "inference", 8 => "lowering",
            9 => "ir-gen",  10 => "backend",  11 => "frontend",
            _ => "unknown",
        });
        let node_count = 5 + (i % 6);
        recipes.push(SimRecipe {
            name,
            nodes: (0..node_count).map(|n| format!("compile unit_{n}.o")).collect(),
            total_ticks: 25 + (i * 3),
            start_tick: tick_offset + (i * 3),
            cache_ratio: if i % 5 == 0 { 0.4 } else { 0.0 },
            fail_at: None,
        });
    }
    tick_offset = 30;

    // ── Wave 3: Feature modules (staggered, some with cache hits) ─────
    let features = [
        "auth", "api", "db", "cache", "queue", "search", "notify",
        "logging", "metrics", "tracing", "config", "secrets", "storage",
        "gateway", "proxy", "router", "middleware", "serializer",
        "validator", "migration", "schema", "admin", "dashboard",
        "billing", "payments", "webhooks", "events", "pubsub",
    ];
    for (i, feat) in features.iter().enumerate() {
        let node_count = 4 + (i % 8);
        recipes.push(SimRecipe {
            name: feat.to_string(),
            nodes: (0..node_count).map(|n| format!("{feat}/src/part_{n}.rs")).collect(),
            total_ticks: 20 + (i * 2),
            start_tick: tick_offset + (i * 2),
            cache_ratio: if i % 7 == 0 { 0.6 } else if i % 4 == 0 { 0.3 } else { 0.0 },
            fail_at: None,
        });
    }
    tick_offset = 60;

    // ── Wave 4: Platform targets (parallel, heavy) ────────────────────
    let platforms = [
        "linux-x86_64", "linux-aarch64", "macos-x86_64", "macos-aarch64",
        "windows-x86_64", "wasm32", "android-arm", "ios-aarch64",
    ];
    for (i, plat) in platforms.iter().enumerate() {
        let node_count = 8 + (i % 5);
        recipes.push(SimRecipe {
            name: format!("platform-{plat}"),
            nodes: (0..node_count).map(|n| format!("{plat}/obj_{n}.o")).collect(),
            total_ticks: 35 + (i * 4),
            start_tick: tick_offset + (i * 4),
            cache_ratio: 0.0,
            fail_at: if i == 5 { Some(6) } else { None }, // wasm32 fails!
        });
    }
    tick_offset = 80;

    // ── Wave 5: Test suites ───────────────────────────────────────────
    let test_suites = [
        "unit-tests", "integration-tests", "e2e-tests", "perf-tests",
        "fuzz-tests", "snapshot-tests", "contract-tests", "smoke-tests",
        "regression-tests", "security-tests",
    ];
    for (i, suite) in test_suites.iter().enumerate() {
        let node_count = 6 + (i % 7);
        recipes.push(SimRecipe {
            name: suite.to_string(),
            nodes: (0..node_count).map(|n| format!("test_{suite}_{n}")).collect(),
            total_ticks: 30 + (i * 3),
            start_tick: tick_offset + (i * 3),
            cache_ratio: 0.0,
            fail_at: if i == 2 { Some(4) } else { None }, // e2e-tests fail!
        });
    }
    tick_offset = 110;

    // ── Wave 6: Documentation & packaging ─────────────────────────────
    let final_steps = [
        "docs-api", "docs-guide", "docs-changelog",
        "package-deb", "package-rpm", "package-brew",
        "container-build", "container-push",
        "release-sign", "release-publish",
    ];
    for (i, step) in final_steps.iter().enumerate() {
        let node_count = 2 + (i % 4);
        recipes.push(SimRecipe {
            name: step.to_string(),
            nodes: (0..node_count).map(|n| format!("{step}-stage-{n}")).collect(),
            total_ticks: 15 + (i * 2),
            start_tick: tick_offset + (i * 2),
            cache_ratio: if i < 3 { 0.5 } else { 0.0 },
            fail_at: None,
        });
    }

    eprintln!("  Simulating {} recipes, {} total nodes\n",
        recipes.len(),
        recipes.iter().map(|r| r.nodes.len()).sum::<usize>(),
    );
    thread::sleep(Duration::from_millis(500));

    recipes
}
