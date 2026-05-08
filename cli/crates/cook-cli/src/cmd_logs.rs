//! `cook logs` built-in — dump per-node logs from .cook/logs/.

use std::fs;
use std::path::PathBuf;

use crate::error::CookError;

pub fn cmd_logs(selector: Option<&str>, build: Option<&str>, failed: bool) -> Result<(), CookError> {
    let root = std::env::current_dir()
        .map_err(|e| CookError::Other(e.to_string()))?
        .join(".cook").join("logs");

    if !root.exists() {
        println!("no builds recorded yet");
        return Ok(());
    }

    let builds = list_builds(&root)?;
    if selector.is_none() && build.is_none() && !failed {
        for (id, _) in &builds {
            println!("{id}");
        }
        return Ok(());
    }

    let target = match build {
        Some(b) => root.join(b),
        None => builds.first()
            .map(|(id, _)| root.join(id))
            .ok_or_else(|| CookError::Other("no builds found".into()))?,
    };

    if failed {
        dump_failed(&target)?;
        return Ok(());
    }

    if let Some(sel) = selector {
        let (recipe, node) = split_selector(sel);
        dump_selector(&target, recipe, node)?;
    }
    Ok(())
}

fn list_builds(root: &PathBuf) -> Result<Vec<(String, std::time::SystemTime)>, CookError> {
    let mut out = Vec::new();
    for entry in fs::read_dir(root).map_err(|e| CookError::Other(e.to_string()))? {
        let entry = entry.map_err(|e| CookError::Other(e.to_string()))?;
        if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            let t = entry.metadata().and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH);
            out.push((entry.file_name().to_string_lossy().to_string(), t));
        }
    }
    out.sort_by(|a, b| b.1.cmp(&a.1));
    Ok(out)
}

fn split_selector(s: &str) -> (&str, Option<&str>) {
    match s.split_once(':') {
        Some((r, n)) => (r, Some(n)),
        None => (s, None),
    }
}

fn dump_selector(build_dir: &PathBuf, recipe: &str, node: Option<&str>) -> Result<(), CookError> {
    let dir = build_dir.join("nodes").join(recipe);
    if !dir.exists() {
        return Err(CookError::Other(format!("no logs for recipe {recipe}")));
    }
    if let Some(n) = node {
        let path = dir.join(format!("{n}.log"));
        let data = fs::read_to_string(&path).map_err(|e| CookError::Other(e.to_string()))?;
        print!("{data}");
    } else {
        for entry in fs::read_dir(&dir).map_err(|e| CookError::Other(e.to_string()))? {
            let entry = entry.map_err(|e| CookError::Other(e.to_string()))?;
            println!("─── {} ───", entry.file_name().to_string_lossy());
            let data = fs::read_to_string(entry.path()).map_err(|e| CookError::Other(e.to_string()))?;
            print!("{data}");
        }
    }
    Ok(())
}

fn dump_failed(build_dir: &PathBuf) -> Result<(), CookError> {
    let events_path = build_dir.join("events.jsonl");
    let data = fs::read_to_string(&events_path).map_err(|e| CookError::Other(e.to_string()))?;
    for line in data.lines() {
        if line.contains("\"type\":\"node-failed\"") {
            println!("{line}");
        }
    }
    Ok(())
}
