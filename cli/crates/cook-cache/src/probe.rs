//! Resolve a ProbeUnit's declared `tools` and `files` sets to content
//! digests, by consulting PATH and the filesystem right now.

use std::path::Path;

use cook_contracts::ProbeUnit;
use sha2::{Digest, Sha256};

/// A probe's declared `tools` and `files` sets, each resolved to
/// `(name-or-path, 32-byte content digest)` pairs against the current host.
///
/// CS-0244: these are VALUE inputs, not key inputs. Nothing keys a probe any
/// more; the pairs exist because four live callers in `cook_probe::eval` need
/// them — the synthesised `@tools-identity` and `@files-manifest` values, and
/// the CS-0214 / COOK-510 guards that refuse an all-zero digest standing in
/// for an identity or a present-but-unreadable file.
///
/// A name or path that cannot be read contributes the all-zero digest rather
/// than being dropped, which is exactly what those two guards test for.
#[derive(Debug, Clone, Default)]
pub struct ProbeInputDigests {
    pub tools: Vec<(String, [u8; 32])>,
    pub files: Vec<(String, [u8; 32])>,
}

/// Resolve a `ProbeUnit`'s declared `tools` and `files` against PATH and the
/// filesystem. Infallible: `requires` is a scheduling edge (CS-0244) and
/// resolves nothing here.
pub fn resolve_probe_input_digests(probe: &ProbeUnit, working_dir: &Path) -> ProbeInputDigests {
    ProbeInputDigests {
        tools: probe
            .inputs
            .tools
            .iter()
            .map(|name| (name.clone(), resolve_tool_hash(name)))
            .collect(),
        files: probe
            .inputs
            .files
            .iter()
            .map(|path| (path.clone(), hash_file_sha256(&working_dir.join(path))))
            .collect(),
    }
}

/// Resolve a tool name to its current PATH location, freshly, every call.
/// CS-0157: the resolved path is LOCATION metadata, not identity — it is
/// deliberately excluded from canonical probe values, and deliberately NOT
/// memoized here, so a consumer (the Lua read view, `cook why` display) always
/// sees where the tool resolves NOW rather than a cached location that can go
/// stale.
pub fn resolve_tool_path(name: &str) -> Option<String> {
    which::which(name).ok().map(|p| p.to_string_lossy().into_owned())
}

/// CS-0158: canonical tool identity for Lua consumers (`cook.tools.id`).
/// Resolves `name` on PATH and returns `(lowercase-hex sha256 of the binary,
/// resolved path)` — the hash is the machine-independent identity a module
/// folds into a sealed probe VALUE; the path is location metadata for
/// invocation. `None` when the name does not resolve. Hashing goes through
/// the same per-run memo as [`resolve_probe_input_digests`]
/// ([`crate::statmemo`]), so a module calling this never re-hashes a binary a
/// `tools` declaration
/// already read, and, since COOK-414, never sees a binary cook rebuilt mid-run
/// at its pre-build bytes either.
pub fn tool_identity(name: &str) -> Option<(String, String)> {
    let path = which::which(name).ok()?;
    let hash = crate::statmemo::tool_hash_memo(&path);
    Some((
        cook_contracts::render::lower_hex(&hash),
        path.to_string_lossy().into_owned(),
    ))
}

fn resolve_tool_hash(name: &str) -> [u8; 32] {
    let Ok(path) = which::which(name) else {
        return [0u8; 32];
    };
    crate::statmemo::tool_hash_memo(&path)
}

/// SHA-256 of a file's bytes, or all-zero when it cannot be read.
///
/// Public because a probe's declared `files` and `tools` sets are hashed with
/// it wherever they are read, and a second hasher over the same question is
/// how two readings of one file come to disagree.
///
/// COOK-414: cook-cache has two file hashes and they answer different
/// questions. This one is IDENTITY THAT LEAVES THE MACHINE: the digests a
/// probe's declared `files`/`tools` sets fold into its VALUE, and the cloud
/// key underneath a sealed consumer of that value.
/// [`crate::check::hash_file`] is the other: xxh3 local content identity for
/// `FileRecord` and the local cache key. The algorithm is part of each name
/// because this function was once ALSO called `hash_file`, privately, in this
/// module, which meant the crate's most-used verb named two different hashes
/// depending on which file you were reading.
pub fn hash_file_sha256(path: &Path) -> [u8; 32] {
    let Ok(bytes) = std::fs::read(path) else {
        return [0u8; 32];
    };
    Sha256::digest(&bytes).into()
}

#[cfg(test)]
#[path = "tests/probe_tests.rs"]
mod tests;
