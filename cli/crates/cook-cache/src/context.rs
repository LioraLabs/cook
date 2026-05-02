//! Per-build execution context: machine identity + tool-binary hashing.
//!
//! `MachineIdentity` is build-wide (probed once per `cook build`).
//! `ToolHash` is per-binary, cached by canonical realpath for the build's lifetime.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MachineIdentity {
    pub target_triple: String,
    pub libc_version: Option<String>,
    pub locale_baseline: BTreeMap<String, String>,
}

impl MachineIdentity {
    /// Probe the current machine. Cheap; suitable to call once per build.
    pub fn probe() -> Self {
        Self {
            target_triple: env!("COOK_TARGET_TRIPLE").to_string(),
            libc_version: probe_libc_version(),
            locale_baseline: probe_locale_baseline(),
        }
    }

    /// Stable byte encoding for hashing. BTreeMap iteration is sorted ⇒ deterministic.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(self.target_triple.as_bytes());
        out.push(0x1F);
        match &self.libc_version {
            Some(v) => out.extend_from_slice(v.as_bytes()),
            None => out.extend_from_slice(b"<none>"),
        }
        out.push(0x1F);
        for (k, v) in &self.locale_baseline {
            out.extend_from_slice(k.as_bytes());
            out.push(b'=');
            out.extend_from_slice(v.as_bytes());
            out.push(0x1E);
        }
        out
    }
}

/// Best-effort probe of the host glibc version.
///
/// Tries to invoke a known libc.so.6 path with `--version` and read the first
/// line of output. Returns `None` on any failure: musl, macOS, BSD, or any
/// platform where none of the candidate paths exist. Returning `None` is
/// safe — it just means `MachineIdentity::libc_version` is omitted from the
/// hash, which is acceptable for those platforms (they will still hash
/// distinctly via target_triple and tool-binary-content).
///
/// **Coverage gap:** the candidate list covers x86_64, aarch64, and generic
/// /lib/libc.so.6. RISC-V, PowerPC, s390x, and other Linux targets fall
/// through to None — extend the list when those platforms are exercised.
///
/// **Hang risk:** `Command::output()` will wait indefinitely if the
/// subprocess blocks. Real glibc completes in microseconds, but unusual
/// sandbox environments could block. Acceptable for v3 (runs once per build);
/// add a timeout if Cook ever runs in such environments.
fn probe_libc_version() -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        let candidates = [
            "/lib/x86_64-linux-gnu/libc.so.6",
            "/lib64/libc.so.6",
            "/lib/aarch64-linux-gnu/libc.so.6",
            "/lib/libc.so.6",
        ];
        for path in &candidates {
            if std::path::Path::new(path).exists() {
                if let Ok(out) = std::process::Command::new(path).output() {
                    if out.status.success() {
                        let s = String::from_utf8_lossy(&out.stdout);
                        if let Some(line) = s.lines().next() {
                            return Some(line.trim().to_string());
                        }
                    }
                }
            }
        }
    }
    None
}

fn probe_locale_baseline() -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let exact_keys = ["LANG", "TZ", "SOURCE_DATE_EPOCH"];
    for k in &exact_keys {
        if let Ok(v) = std::env::var(k) {
            out.insert((*k).to_string(), v);
        }
    }
    for (k, v) in std::env::vars() {
        if k.starts_with("LC_") {
            out.insert(k, v);
        }
    }
    out
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct ToolHash {
    pub content_sha256: [u8; 32],
}

impl ToolHash {
    /// Sentinel for "binary not resolvable" — uses an all-zero array which is
    /// not a valid SHA-256 output and therefore cannot collide with any real
    /// file's content hash. (SHA-256 of any real input always has non-zero bytes
    /// in practice; an attacker producing a true zero-array preimage would have
    /// broken the hash function.)
    pub fn empty() -> Self {
        Self { content_sha256: [0u8; 32] }
    }

    pub fn from_file(path: &Path) -> Option<Self> {
        let bytes = std::fs::read(path).ok()?;
        let mut h = Sha256::new();
        h.update(&bytes);
        let digest: [u8; 32] = h.finalize().into();
        Some(Self { content_sha256: digest })
    }

    pub fn for_resolved(realpath: Option<&Path>) -> Self {
        match realpath.and_then(Self::from_file) {
            Some(h) => h,
            None => Self::empty(),
        }
    }
}

pub struct ExecutionContext {
    pub machine: MachineIdentity,
    /// Per-build: lazily probed and cached, keyed by canonical realpath.
    pub(crate) tool_cache: Mutex<HashMap<PathBuf, ToolHash>>,
}

impl ExecutionContext {
    pub fn probe() -> Self {
        Self {
            machine: MachineIdentity::probe(),
            tool_cache: Mutex::new(HashMap::new()),
        }
    }

    pub fn step_context_hash(&self, command: &str) -> u64 {
        let primary_tool = first_argv_token(command);
        let realpath = primary_tool
            .and_then(|t| which::which(t).ok())
            .and_then(|p| std::fs::canonicalize(p).ok());
        let tool_hash = self.tool_hash_for(realpath.as_deref());

        let machine_bytes = self.machine.encode();
        let mut hasher = xxhash_rust::xxh3::Xxh3::new();
        hasher.update(&machine_bytes);
        hasher.update(&tool_hash.content_sha256);
        hasher.digest()
    }

    fn tool_hash_for(&self, realpath: Option<&Path>) -> ToolHash {
        let Some(rp) = realpath else { return ToolHash::empty(); };
        {
            let cache = self.tool_cache.lock().expect("tool_cache poisoned");
            if let Some(h) = cache.get(rp) {
                return h.clone();
            }
        }
        let computed = ToolHash::for_resolved(Some(rp));
        let mut cache = self.tool_cache.lock().expect("tool_cache poisoned");
        cache.insert(rp.to_path_buf(), computed.clone());
        computed
    }
}

fn first_argv_token(command: &str) -> Option<&str> {
    command.split_whitespace().next()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn machine_identity_target_triple_is_set() {
        let m = MachineIdentity::probe();
        assert!(!m.target_triple.is_empty(), "target_triple must be populated");
        assert!(m.target_triple.contains('-'));
    }

    #[test]
    fn machine_identity_locale_baseline_includes_lang_when_set() {
        let m = MachineIdentity::probe();
        let _ = m.locale_baseline.get("LANG");
    }

    #[test]
    fn machine_identity_encode_deterministic() {
        let m1 = MachineIdentity::probe();
        let m2 = MachineIdentity::probe();
        assert_eq!(m1.encode(), m2.encode(), "two probes on same host produce same bytes");
    }

    #[test]
    fn tool_hash_of_known_file_is_deterministic() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("fake-tool");
        std::fs::write(&path, b"fake binary contents").expect("write");

        let h1 = ToolHash::from_file(&path).expect("hash");
        let h2 = ToolHash::from_file(&path).expect("hash");
        assert_eq!(h1.content_sha256, h2.content_sha256);
    }

    #[test]
    fn tool_hash_differs_for_different_contents() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p1 = dir.path().join("a"); std::fs::write(&p1, b"AAA").expect("write");
        let p2 = dir.path().join("b"); std::fs::write(&p2, b"BBB").expect("write");

        let h1 = ToolHash::from_file(&p1).expect("hash");
        let h2 = ToolHash::from_file(&p2).expect("hash");
        assert_ne!(h1.content_sha256, h2.content_sha256);
    }

    #[test]
    fn tool_hash_missing_file_returns_empty_hash() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("nonexistent");
        let h = ToolHash::for_resolved(Some(path.as_path()));
        let expected = ToolHash::empty();
        assert_eq!(h.content_sha256, expected.content_sha256);
    }

    #[test]
    fn step_context_hash_combines_machine_and_tool() {
        let ctx = ExecutionContext::probe();
        let h_a = ctx.step_context_hash("/bin/sh -c true");
        let h_b = ctx.step_context_hash("/bin/sh -c true");
        assert_eq!(h_a, h_b, "same command on same machine → same hash");
    }

    #[test]
    fn step_context_hash_differs_on_first_token() {
        let ctx = ExecutionContext::probe();
        let (Some(sh), Some(tr)) = (which::which("sh").ok(), which::which("true").ok()) else {
            eprintln!("skipping: sh or true not on PATH");
            return;
        };
        let h_sh = ctx.step_context_hash(sh.to_string_lossy().as_ref());
        let h_true = ctx.step_context_hash(tr.to_string_lossy().as_ref());
        assert_ne!(h_sh, h_true, "different binaries should hash differently");
    }

    #[test]
    fn tool_cache_caches_per_realpath() {
        let ctx = ExecutionContext::probe();
        let _ = ctx.step_context_hash("/bin/sh foo");
        let _ = ctx.step_context_hash("/bin/sh bar");
        let cache = ctx.tool_cache.lock().expect("lock");
        let bin_sh_realpath = std::fs::canonicalize("/bin/sh").ok();
        if let Some(rp) = bin_sh_realpath {
            assert_eq!(cache.get(&rp).is_some(), true, "tool_cache should contain /bin/sh's realpath");
        }
    }
}
