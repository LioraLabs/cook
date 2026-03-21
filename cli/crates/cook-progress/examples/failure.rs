use std::io::{self, Write};
use std::thread;
use std::time::{Duration, Instant};

use cook_progress::{Frame, Renderer, RenderConfig, Section, Status, ItemStatus};

fn main() -> io::Result<()> {
    let mut stdout = io::stdout();
    let (cols, _) = crossterm::terminal::size().unwrap_or((80, 24));
    let mut renderer = Renderer::new(RenderConfig {
        width: cols,
        max_output_lines: 8,
        colors: true,
        ..Default::default()
    });

    write!(stdout, "\x1b[?25l")?;
    let start = Instant::now();

    let nodes = [
        "compile a.c",
        "compile b.c",
        "compile c.c (error)",
        "compile d.c",
        "link lib",
    ];
    let total = nodes.len();

    // Run for 15 ticks — fail partway through node 2
    for tick in 0..15usize {
        renderer.clear_last_frame(&mut stdout)?;

        let node_idx = (tick * total) / 15;
        let node_idx = node_idx.min(total - 1);
        let node = nodes[node_idx];

        // Emit compiler-like output as we progress
        match tick {
            2 => renderer.push_output("lib", "  compile a.c..."),
            5 => renderer.push_output("lib", "  compile b.c..."),
            8 => {
                renderer.push_output("lib", "  compile c.c...");
                renderer.push_output("lib", "c.c:42:10: error: use of undeclared identifier 'foo'");
                renderer.push_output("lib", "      int x = foo(bar, baz);");
                renderer.push_output("lib", "              ^~~");
                renderer.push_output("lib", "c.c:57:3: error: unknown type name 'Widget'");
                renderer.push_output("lib", "   Widget *w = Widget_new();");
                renderer.push_output("lib", "   ^~~~~~");
                renderer.push_output("lib", "2 errors generated.");
            }
            _ => {}
        }

        let frame = Frame::new().section(
            Section::new("lib", "lib")
                .status(Status::Running)
                .progress(node_idx, total)
                .elapsed(start.elapsed())
                .active_item(node, ItemStatus::Running),
        );
        renderer.render_frame(&frame, &mut stdout)?;
        stdout.flush()?;
        thread::sleep(Duration::from_millis(120));
    }

    // Trigger error expansion and show failed state
    renderer.set_error("lib");
    renderer.clear_last_frame(&mut stdout)?;

    let frame = Frame::new().section(
        Section::new("lib", "lib")
            .status(Status::Failed)
            .progress(2, total)
            .elapsed(start.elapsed()),
    );
    renderer.render_frame(&frame, &mut stdout)?;
    write!(stdout, "\x1b[?25h")?;
    stdout.flush()?;
    Ok(())
}
