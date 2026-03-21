use std::io::{self, Write};
use std::thread;
use std::time::{Duration, Instant};

use cook_progress::{Frame, Renderer, RenderConfig, Section, Status, ItemStatus};

fn main() -> io::Result<()> {
    let mut stdout = io::stdout();
    let (cols, _) = crossterm::terminal::size().unwrap_or((80, 24));
    let mut renderer = Renderer::new(RenderConfig {
        width: cols,
        max_output_lines: 3,
        colors: true,
        ..Default::default()
    });

    write!(stdout, "\x1b[?25l")?;
    let start = Instant::now();

    let tick_ms: u64 = 100;
    let total_ticks: usize = 40;

    // deps: 10 nodes, runs ticks 0-10
    let deps_nodes = ["fetch a", "fetch b", "fetch c", "fetch d", "fetch e",
                      "fetch f", "fetch g", "fetch h", "fetch i", "fetch j"];
    // lib: 8 nodes, runs ticks 10-30
    let lib_nodes = ["compile a.c", "compile b.c", "compile c.c", "compile d.c",
                     "compile e.c", "compile f.c", "compile g.c", "link lib"];
    // test: 5 nodes, runs ticks 10-38
    let test_nodes = ["test_a", "test_b", "test_c", "test_d", "test_e"];

    let mut deps_done = false;
    let mut lib_done = false;
    let mut test_done = false;

    for tick in 0..total_ticks {
        renderer.clear_last_frame(&mut stdout)?;

        // — deps section (ticks 0–10) —
        let deps_section = if tick >= 10 && !deps_done {
            deps_done = true;
            Section::new("deps", "deps")
                .status(Status::Completed)
                .progress(deps_nodes.len(), deps_nodes.len())
                .elapsed(Duration::from_millis(tick_ms * 10))
        } else if deps_done || tick >= 10 {
            Section::new("deps", "deps")
                .status(Status::Completed)
                .progress(deps_nodes.len(), deps_nodes.len())
                .elapsed(Duration::from_millis(tick_ms * 10))
        } else {
            let i = (tick * deps_nodes.len()) / 10;
            let node = deps_nodes[i.min(deps_nodes.len() - 1)];
            if tick % 3 == 0 {
                renderer.push_output("deps", &format!("  downloading {node}..."));
            }
            Section::new("deps", "deps")
                .status(Status::Running)
                .progress(i, deps_nodes.len())
                .elapsed(start.elapsed())
                .active_item(node, ItemStatus::Running)
        };

        // — lib section (waits until tick 10, runs 10-30) —
        let lib_section = if tick < 10 {
            Section::new("lib", "lib").status(Status::Waiting)
        } else if tick >= 30 && !lib_done {
            lib_done = true;
            Section::new("lib", "lib")
                .status(Status::Completed)
                .progress(lib_nodes.len(), lib_nodes.len())
                .elapsed(Duration::from_millis(tick_ms * 20))
                .cache(2, lib_nodes.len())
        } else if lib_done || tick >= 30 {
            Section::new("lib", "lib")
                .status(Status::Completed)
                .progress(lib_nodes.len(), lib_nodes.len())
                .elapsed(Duration::from_millis(tick_ms * 20))
                .cache(2, lib_nodes.len())
        } else {
            let i = ((tick - 10) * lib_nodes.len()) / 20;
            let node = lib_nodes[i.min(lib_nodes.len() - 1)];
            if (tick - 10) % 4 == 0 {
                renderer.push_output("lib", &format!("  building {node}..."));
            }
            Section::new("lib", "lib")
                .status(Status::Running)
                .progress(i, lib_nodes.len())
                .elapsed(start.elapsed())
                .active_item(node, ItemStatus::Running)
        };

        // — test section (waits until tick 10, runs 10-38) —
        let test_section = if tick < 10 {
            Section::new("test", "test").status(Status::Waiting)
        } else if tick >= 38 && !test_done {
            test_done = true;
            Section::new("test", "test")
                .status(Status::Completed)
                .progress(test_nodes.len(), test_nodes.len())
                .elapsed(Duration::from_millis(tick_ms * 28))
        } else if test_done || tick >= 38 {
            Section::new("test", "test")
                .status(Status::Completed)
                .progress(test_nodes.len(), test_nodes.len())
                .elapsed(Duration::from_millis(tick_ms * 28))
        } else {
            let i = ((tick - 10) * test_nodes.len()) / 28;
            let node = test_nodes[i.min(test_nodes.len() - 1)];
            if (tick - 10) % 5 == 0 {
                renderer.push_output("test", &format!("  running {node}..."));
            }
            Section::new("test", "test")
                .status(Status::Running)
                .progress(i, test_nodes.len())
                .elapsed(start.elapsed())
                .active_item(node, ItemStatus::Running)
        };

        // — footer with running/done/waiting counts —
        let sections = [&deps_section, &lib_section, &test_section];
        let running = sections.iter().filter(|s| s.status == Status::Running).count();
        let done = sections.iter().filter(|s| matches!(s.status, Status::Completed | Status::Cached)).count();
        let waiting = sections.iter().filter(|s| s.status == Status::Waiting).count();
        let footer = format!("{running} running · {done} done · {waiting} waiting");

        let frame = Frame::new()
            .section(deps_section)
            .section(lib_section)
            .section(test_section)
            .footer(footer);

        renderer.render_frame(&frame, &mut stdout)?;
        stdout.flush()?;
        thread::sleep(Duration::from_millis(tick_ms));
    }

    write!(stdout, "\x1b[?25h")?;
    stdout.flush()?;
    Ok(())
}
