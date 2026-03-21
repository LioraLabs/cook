use std::io::{self, Write};
use std::thread;
use std::time::{Duration, Instant};

use cook_progress::{Frame, Renderer, RenderConfig, Section, Status, ItemStatus};

fn main() -> io::Result<()> {
    let mut stdout = io::stdout();
    let (cols, _) = crossterm::terminal::size().unwrap_or((80, 24));
    let mut renderer = Renderer::new(RenderConfig {
        width: cols,
        max_output_lines: 4,
        colors: true,
        ..Default::default()
    });

    write!(stdout, "\x1b[?25l")?;
    let start = Instant::now();

    let tick_ms: u64 = 100;
    let total_ticks: usize = 50;

    // deps: runs ticks 0-8, then Cached
    let deps_nodes = ["pkg-a", "pkg-b", "pkg-c", "pkg-d"];

    // lib: waits until tick 8, runs 8-28, then Completed with cache(2, 6)
    let lib_nodes = [
        "compile a.c",
        "compile b.c",
        "compile c.c",
        "compile d.c",
        "compile e.c",
        "link lib",
    ];

    // test: waits until tick 28, runs 28-36 (ItemStatus::Failed at tick 34+), fails at 36
    let test_nodes = ["test_alpha", "test_beta", "test_gamma", "test_delta"];

    // bench: always waiting
    let mut deps_cached = false;
    let mut lib_done = false;
    let mut test_failed = false;
    let mut error_set = false;

    for tick in 0..total_ticks {
        renderer.clear_last_frame(&mut stdout)?;

        // ── deps section ──────────────────────────────────────────────────
        let deps_section = if tick >= 8 {
            deps_cached = true;
            Section::new("deps", "deps")
                .status(Status::Cached)
                .progress(deps_nodes.len(), deps_nodes.len())
        } else {
            let i = (tick * deps_nodes.len()) / 8;
            let i = i.min(deps_nodes.len() - 1);
            let node = deps_nodes[i];
            if tick % 2 == 0 {
                renderer.push_output("deps", &format!("  resolving {node}..."));
            }
            Section::new("deps", "deps")
                .status(Status::Running)
                .progress(i, deps_nodes.len())
                .elapsed(start.elapsed())
                .active_item(node, ItemStatus::Running)
        };
        let _ = deps_cached; // used above

        // ── lib section ───────────────────────────────────────────────────
        let lib_section = if tick < 8 {
            Section::new("lib", "lib").status(Status::Waiting)
        } else if tick >= 28 {
            lib_done = true;
            Section::new("lib", "lib")
                .status(Status::Completed)
                .progress(lib_nodes.len(), lib_nodes.len())
                .elapsed(Duration::from_millis(tick_ms * 20))
                .cache(2, lib_nodes.len())
        } else {
            let local_tick = tick - 8; // 0..20
            let i = (local_tick * lib_nodes.len()) / 20;
            let i = i.min(lib_nodes.len() - 1);
            let node = lib_nodes[i];

            // Demonstrate all ItemStatus variants across the run
            let item_status = match local_tick {
                0..=4 => {
                    if local_tick % 2 == 0 {
                        renderer.push_output("lib", &format!("  building {node}..."));
                    }
                    ItemStatus::Running
                }
                5..=8 => ItemStatus::Completed,
                9..=12 => ItemStatus::Cached,
                13..=16 => ItemStatus::Skipped,
                _ => ItemStatus::Running,
            };

            Section::new("lib", "lib")
                .status(Status::Running)
                .progress(i, lib_nodes.len())
                .elapsed(start.elapsed())
                .active_item(node, item_status)
        };
        let _ = lib_done;

        // ── test section ──────────────────────────────────────────────────
        let test_section = if tick < 28 {
            Section::new("test", "test").status(Status::Waiting)
        } else if tick >= 36 {
            if !error_set {
                renderer.push_output("test", "test_gamma: assertion failed: left == right");
                renderer.push_output("test", "  left:  42");
                renderer.push_output("test", "  right: 99");
                renderer.push_output("test", "thread 'test_gamma' panicked at src/lib.rs:77");
                renderer.set_error("test");
                error_set = true;
            }
            test_failed = true;
            Section::new("test", "test")
                .status(Status::Failed)
                .progress(2, test_nodes.len())
                .elapsed(Duration::from_millis(tick_ms * 8))
        } else {
            let local_tick = tick - 28; // 0..8
            let i = (local_tick * test_nodes.len()) / 8;
            let i = i.min(test_nodes.len() - 1);
            let node = test_nodes[i];

            // Show ItemStatus::Failed at tick 34+ (local_tick 6+)
            let item_status = if local_tick >= 6 {
                ItemStatus::Failed
            } else {
                ItemStatus::Running
            };

            Section::new("test", "test")
                .status(Status::Running)
                .progress(i, test_nodes.len())
                .elapsed(start.elapsed())
                .active_item(node, item_status)
        };
        let _ = test_failed;

        // ── bench section: always waiting ─────────────────────────────────
        let bench_section = Section::new("bench", "bench").status(Status::Waiting);

        // ── footer with dynamic counts ────────────────────────────────────
        let all = [&deps_section, &lib_section, &test_section, &bench_section];
        let running = all.iter().filter(|s| s.status == Status::Running).count();
        let done = all.iter().filter(|s| matches!(s.status, Status::Completed)).count();
        let cached = all.iter().filter(|s| s.status == Status::Cached).count();
        let failed = all.iter().filter(|s| s.status == Status::Failed).count();
        let waiting = all.iter().filter(|s| s.status == Status::Waiting).count();

        let mut parts = Vec::new();
        if running > 0 { parts.push(format!("{running} running")); }
        if done > 0 { parts.push(format!("{done} done")); }
        if cached > 0 { parts.push(format!("{cached} cached")); }
        if failed > 0 { parts.push(format!("{failed} failed")); }
        if waiting > 0 { parts.push(format!("{waiting} waiting")); }
        let footer = parts.join(" · ");

        let frame = Frame::new()
            .section(deps_section)
            .section(lib_section)
            .section(test_section)
            .section(bench_section)
            .footer(footer);

        renderer.render_frame(&frame, &mut stdout)?;
        stdout.flush()?;
        thread::sleep(Duration::from_millis(tick_ms));
    }

    write!(stdout, "\x1b[?25h")?;
    stdout.flush()?;
    Ok(())
}
