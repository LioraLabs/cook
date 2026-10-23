//! The getenv recorder `cook audit` preloads into a traced re-run, built from
//! `src/envtrace.c` and shipped inside the binary.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The recorder's shared object; `None` off Linux, where nothing can preload it.
#[cfg(envtrace_built)]
pub const PRELOAD: Option<&'static [u8]> = Some(include_bytes!(concat!(env!("OUT_DIR"), "/libcook_envtrace.so")));
#[cfg(not(envtrace_built))]
pub const PRELOAD: Option<&'static [u8]> = None;

const PRELOAD_VAR: &str = "LD_PRELOAD";

/// The variable naming the file the recorder appends names to; defined once, in `build.rs`.
pub const OUT_VAR: &str = env!("COOK_ENVTRACE_OUT_VAR");

/// Writes the recorder into `dir`; `None` where there is none to write.
pub fn install(dir: &Path) -> std::io::Result<Option<PathBuf>> {
    let Some(so) = PRELOAD else { return Ok(None) };
    let path = dir.join("libcook_envtrace.so");
    // LD_PRELOAD splits on these; a path holding one would silently load nothing.
    if path.to_string_lossy().contains([' ', ':']) {
        let why = format!("the env recorder's path {} contains ' ' or ':', which LD_PRELOAD cannot carry", path.display());
        return Err(std::io::Error::new(std::io::ErrorKind::Unsupported, why));
    }
    std::fs::write(&path, so)?;
    Ok(Some(path))
}

/// The environment that makes a process tree record its queried names into
/// `names_file`. This process's own `LD_PRELOAD` is kept after ours.
pub fn env_for(so: &Path, names_file: &Path) -> BTreeMap<String, String> {
    let so = so.to_string_lossy();
    let preload = match std::env::var(PRELOAD_VAR) {
        Ok(old) if !old.is_empty() => format!("{so}:{old}"),
        _ => so.into_owned(),
    };
    BTreeMap::from([
        (PRELOAD_VAR.to_string(), preload),
        (OUT_VAR.to_string(), names_file.to_string_lossy().into_owned()),
    ])
}

#[cfg(all(test, envtrace_built))]
#[path = "tests/lib_tests.rs"]
mod tests;
