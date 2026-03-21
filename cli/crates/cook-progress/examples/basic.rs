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
    let nodes = ["compile a.c", "compile b.c", "compile c.c", "compile d.c", "link lib"];
    let total = nodes.len();

    for (i, node) in nodes.iter().enumerate() {
        for tick in 0..5 {
            renderer.clear_last_frame(&mut stdout)?;
            if tick == 2 {
                renderer.push_output("lib", &format!("  {node}..."));
            }
            let frame = Frame::new().section(
                Section::new("lib", "lib")
                    .status(Status::Running)
                    .progress(i, total)
                    .elapsed(start.elapsed())
                    .active_item(*node, ItemStatus::Running),
            );
            renderer.render_frame(&frame, &mut stdout)?;
            stdout.flush()?;
            thread::sleep(Duration::from_millis(80));
        }
    }

    renderer.clear_last_frame(&mut stdout)?;
    let frame = Frame::new().section(
        Section::new("lib", "lib")
            .status(Status::Completed)
            .progress(total, total)
            .elapsed(start.elapsed()),
    );
    renderer.render_frame(&frame, &mut stdout)?;
    write!(stdout, "\x1b[?25h")?;
    stdout.flush()?;
    Ok(())
}
