//! Terminal test reporter — live event accumulation + final summary block.
//!
//! Per docs/superpowers/specs/2026-05-07-test-runner-design.md §6.5.

use std::collections::BTreeMap;
use std::time::Duration;
use cook_engine::{EngineEvent, TestId, TestOutcome, TestResult};
use crate::cli::Cli;

pub struct Reporter {
    by_recipe: BTreeMap<String, RecipeStats>,
    started: std::time::Instant,
    verbose: bool,
}

#[derive(Default, Clone)]
struct RecipeStats {
    passed: usize,
    failed: usize,
    blocked: usize,
    timed_out: usize,
    cached: usize,
    duration: Duration,
}

impl Reporter {
    pub fn new(cli: &Cli) -> Self {
        Self {
            by_recipe: BTreeMap::new(),
            started: std::time::Instant::now(),
            verbose: cli.verbose,
        }
    }

    pub fn on_event(&mut self, evt: EngineEvent) {
        match evt {
            EngineEvent::TestStarted { id, .. } => {
                if self.verbose {
                    println!("    test {} ...", id);
                }
            }
            EngineEvent::TestPassed { id, duration, cached, .. } => {
                let recipe = recipe_of(&id);
                let s = self.by_recipe.entry(recipe).or_default();
                s.passed += 1;
                if cached { s.cached += 1; }
                s.duration += duration;
            }
            EngineEvent::TestFailed { id, duration, .. } => {
                let recipe = recipe_of(&id);
                let s = self.by_recipe.entry(recipe).or_default();
                s.failed += 1;
                s.duration += duration;
            }
            EngineEvent::TestBlocked { id, .. } => {
                let recipe = recipe_of(&id);
                self.by_recipe.entry(recipe).or_default().blocked += 1;
            }
            EngineEvent::TestTimedOut { id, timeout, .. } => {
                let recipe = recipe_of(&id);
                let s = self.by_recipe.entry(recipe).or_default();
                s.timed_out += 1;
                s.duration += timeout;
            }
            _ => {}
        }
    }

    pub fn finish(&mut self, results: &[TestResult]) {
        // Per-recipe lines
        for (recipe, s) in &self.by_recipe {
            let icon = if s.failed > 0 || s.blocked > 0 || s.timed_out > 0 {
                "FAIL"
            } else if s.cached == s.passed && s.passed > 0 {
                "CACHED"
            } else {
                "PASS"
            };
            print!("[{}] {:<25}", icon, recipe);
            let mut parts = Vec::new();
            if s.passed > 0 { parts.push(format!("{} passed", s.passed)); }
            if s.failed > 0 { parts.push(format!("{} failed", s.failed)); }
            if s.blocked > 0 { parts.push(format!("{} blocked", s.blocked)); }
            if s.timed_out > 0 { parts.push(format!("{} timed out", s.timed_out)); }
            if s.cached > 0 { parts.push(format!("{} cached", s.cached)); }
            print!(" {}", parts.join(", "));
            println!("  ({:.1}s)", s.duration.as_secs_f64());
        }

        // Failures section
        let failures: Vec<&TestResult> = results.iter()
            .filter(|r| matches!(r.outcome, TestOutcome::Failed | TestOutcome::TimedOut))
            .collect();
        if !failures.is_empty() {
            println!();
            println!("Failures:");
            for r in &failures {
                let display_name = if r.name.is_empty() { "(unnamed)" } else { r.name.as_str() };
                println!("  {} > {}", recipe_of(&r.id), display_name);
                if !r.stdout.is_empty() {
                    for line in r.stdout.lines().take(20) {
                        println!("    {}", line);
                    }
                }
                if !r.stderr.is_empty() {
                    println!("    [ stderr ]");
                    for line in r.stderr.lines().take(20) {
                        println!("    {}", line);
                    }
                }
                println!();
            }
        }

        // Blocked section
        let blocked: Vec<&TestResult> = results.iter()
            .filter(|r| matches!(r.outcome, TestOutcome::Blocked))
            .collect();
        if !blocked.is_empty() {
            println!("Blocked:");
            for r in &blocked {
                let display_name = if r.name.is_empty() { "(unnamed)" } else { r.name.as_str() };
                let cause = r.blocked_by.as_deref().unwrap_or("upstream cook step");
                println!("  {} > {}  (build failed: {})", recipe_of(&r.id), display_name, cause);
            }
            println!();
        }

        // Summary
        let total_passed: usize = self.by_recipe.values().map(|s| s.passed).sum();
        let total_failed: usize = self.by_recipe.values().map(|s| s.failed).sum();
        let total_blocked: usize = self.by_recipe.values().map(|s| s.blocked).sum();
        let total_to: usize = self.by_recipe.values().map(|s| s.timed_out).sum();
        let total_cached: usize = self.by_recipe.values().map(|s| s.cached).sum();
        let wall = self.started.elapsed();
        let cache_savings: Duration = results.iter()
            .filter(|r| r.from_cache)
            .map(|r| r.duration)
            .sum();

        let mut parts = Vec::new();
        if total_passed > 0 { parts.push(format!("{} passed", total_passed)); }
        if total_failed > 0 { parts.push(format!("{} failed", total_failed)); }
        if total_blocked > 0 { parts.push(format!("{} blocked", total_blocked)); }
        if total_to > 0 { parts.push(format!("{} timed out", total_to)); }
        if total_cached > 0 { parts.push(format!("{} cached", total_cached)); }
        if parts.is_empty() {
            println!("Summary: no tests ran  --  {:.1}s wall", wall.as_secs_f64());
        } else {
            println!(
                "Summary: {}  --  {:.1}s wall ({:.1}s saved by cache)",
                parts.join(", "),
                wall.as_secs_f64(),
                cache_savings.as_secs_f64()
            );
        }

        // Footer hint when there are failures
        if total_failed > 0 || total_blocked > 0 || total_to > 0 {
            println!();
            println!("Failed tests:");
            println!("  cook --test --rerun-failed         # re-run only these");
            println!("  cat .cook/test-report.json | jq    # full structured report");
        }
    }
}

fn recipe_of(id: &TestId) -> String {
    let s = &id.0;
    s.split(':').next().unwrap_or("").to_string()
}

// Stubs filled in Phase 8.
pub fn write_json_sidecar(
    _root: &std::path::Path,
    _cli: &Cli,
    _results: &[TestResult],
) -> std::io::Result<()> {
    Ok(())
}

pub fn write_junit_sidecar(
    _path: &std::path::Path,
    _results: &[TestResult],
) -> std::io::Result<()> {
    Ok(())
}
