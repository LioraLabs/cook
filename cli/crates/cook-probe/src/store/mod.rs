use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static WRITE_COUNTER: AtomicU64 = AtomicU64::new(0);

pub fn materialize_value(dir: &Path, key: &str, bytes: &[u8]) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let name = cook_contracts::probe::value::probe_file_name(key);
    let destination = dir.join(&name);
    let sequence = WRITE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let temporary = dir.join(format!(".{name}.tmp-{}-{sequence}", std::process::id()));

    if let Err(error) = std::fs::write(&temporary, bytes) {
        let _ = std::fs::remove_file(&temporary);
        return Err(error);
    }
    if let Err(error) = std::fs::rename(&temporary, &destination) {
        let _ = std::fs::remove_file(&temporary);
        return Err(error);
    }

    Ok(destination)
}
