use super::*;
use std::time::Instant;

fn entry(name: &str, secs_ago: u64) -> RunningEntry {
    let started = Instant::now() - Duration::from_secs(secs_ago);
    RunningEntry { started_at: started, display: name.into() }
}

fn snapshot(total: usize, done: usize, running: &[(&str, u64)], elapsed: u64) -> StatusSnapshot {
    StatusSnapshot {
        total_nodes: total,
        done_nodes: done,
        running: running.iter().map(|(n, s)| entry(n, *s)).collect(),
        started_at: Instant::now() - Duration::from_secs(elapsed),
    }
}

#[test]
fn empty_when_no_running_work() {
    let s = snapshot(47, 47, &[], 4);
    let line = render_status_line(&s, StatusLineOptions { colored: false, ..Default::default() }, 100);
    assert_eq!(line, "");
}

#[test]
fn empty_when_total_below_threshold() {
    let s = snapshot(2, 0, &[("a.c", 0), ("b.c", 0)], 1);
    let line = render_status_line(&s, StatusLineOptions { colored: false, min_nodes: 5 }, 100);
    assert_eq!(line, "");
}

#[test]
fn renders_verb_bar_counter_names_elapsed() {
    let s = snapshot(47, 14, &[("lvm.o", 1), ("ldebug.o", 1), ("lcode.o", 0)], 2);
    let line = render_status_line(&s, StatusLineOptions { colored: false, ..Default::default() }, 120);
    assert!(line.contains("Cooking"));
    assert!(line.contains("14/47"));
    assert!(line.contains("lvm.o"));
    assert!(line.contains("ldebug.o"));
    assert!(line.contains("lcode.o"));
    // 2 secs of elapsed.
    assert!(line.contains("2.0s"), "got: {line}");
}

#[test]
fn names_overflow_emits_plus_n() {
    let names: Vec<(&str, u64)> = (0..8).map(|i|
        (Box::leak(format!("file_with_long_name_{i}.o").into_boxed_str()) as &str, 0)
    ).collect();
    let s = snapshot(47, 14, &names, 1);
    let line = render_status_line(&s, StatusLineOptions { colored: false, ..Default::default() }, 80);
    assert!(line.contains("+"), "expected overflow indicator: {line}");
}
