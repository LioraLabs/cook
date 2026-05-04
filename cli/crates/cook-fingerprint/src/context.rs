//! Per-build execution context: machine identity + tool-binary hashing.
//!
//! `MachineIdentity` is build-wide (probed once per `cook build`).
//! `ToolHash` is per-binary, cached by canonical realpath for the build's lifetime.

use std::collections::{BTreeMap, BTreeSet, HashMap};
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

    /// Compute the per-step context hash from machine identity plus the
    /// content-hash of every tool binary the command's first executable on
    /// each newline-separated statement resolves to.
    ///
    /// **Tokenization model.** The command text is split on newlines into
    /// statements. For each statement, leading `VAR=value` env-prefix tokens
    /// are skipped, and the next token is treated as an executable name and
    /// resolved via `which`. Each unique resolved realpath is fingerprinted
    /// once and folded into the hash in deterministic (`BTreeSet`-sorted)
    /// order.
    ///
    /// **Coverage limit.** Only the first executable on each newline-separated
    /// statement is fingerprinted. Tools invoked downstream of a `;`, `&&`,
    /// `||`, pipe (`|`), command substitution (`$(…)`, backticks), subshell
    /// (`(…)`), or `xargs`/`find -exec` are NOT fingerprinted. To pin the
    /// fingerprint for those tools, place each invocation on its own line,
    /// or declare them explicitly via `cache.tools = { ... }` (planned cache
    /// configuration mechanism — see CS-0035).
    pub fn step_context_hash(&self, command: &str) -> u64 {
        let realpaths = resolved_tool_realpaths(command);
        let machine_bytes = self.machine.encode();
        let mut hasher = xxhash_rust::xxh3::Xxh3::new();
        hasher.update(&machine_bytes);
        if realpaths.is_empty() {
            // Preserve prior behavior for commands that resolve no executable
            // (e.g., empty string, builtins-only): fold in the empty ToolHash
            // so the hash still incorporates a tool component.
            let tool_hash = ToolHash::empty();
            hasher.update(&tool_hash.content_sha256);
        } else {
            for rp in &realpaths {
                let tool_hash = self.tool_hash_for(Some(rp.as_path()));
                hasher.update(&tool_hash.content_sha256);
            }
        }
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

/// Tokenize `command` and resolve every executable token reachable via
/// `which`, returning a deterministic, deduplicated set of canonical
/// realpaths (sorted via `BTreeSet`).
///
/// Strategy: split on newlines; for each line, skip leading `VAR=value`
/// env-prefix tokens (a token containing `=` whose name part is a valid
/// shell variable identifier); take the next remaining token as the
/// executable; resolve via `which` and `canonicalize`. Returns paths in
/// deterministic order.
///
/// See `step_context_hash` for the documented coverage limit.
fn resolved_tool_realpaths(command: &str) -> BTreeSet<PathBuf> {
    let mut out = BTreeSet::new();
    for line in command.split('\n') {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let exe = trimmed
            .split_whitespace()
            .find(|tok| !is_env_prefix_token(tok));
        let Some(exe) = exe else { continue };
        let Ok(p) = which::which(exe) else { continue };
        let Ok(rp) = std::fs::canonicalize(p) else { continue };
        out.insert(rp);
    }
    out
}

/// Returns true if `tok` looks like a leading `VAR=value` env assignment
/// (POSIX-style command-prefix env var, e.g. `LC_ALL=C make`). The name
/// must be a valid shell-style identifier (letters/digits/underscore, not
/// starting with a digit) to avoid mistaking arguments like `--flag=val`
/// for env prefixes.
fn is_env_prefix_token(tok: &str) -> bool {
    let Some(eq) = tok.find('=') else { return false };
    let name = &tok[..eq];
    if name.is_empty() {
        return false;
    }
    let mut chars = name.chars();
    let first = chars.next().unwrap();
    if !(first.is_ascii_alphabetic() || first == '_') {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
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
    fn step_context_hash_differs_on_executable() {
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

    #[test]
    fn step_context_hash_fingerprints_tools_on_later_lines() {
        // Multi-line shell scripts: the dominant Cookfile pattern. Pre-fix,
        // only the first line's first token was hashed, so a script whose
        // first line was `mkdir -p build` collapsed every subsequent
        // toolchain (gcc/clang/ld) to the `mkdir`-only fingerprint.
        let ctx = ExecutionContext::probe();
        let (Some(_), Some(_), Some(_)) = (
            which::which("mkdir").ok(),
            which::which("sh").ok(),
            which::which("true").ok(),
        ) else {
            eprintln!("skipping: required tools not on PATH");
            return;
        };
        let h_mkdir_only = ctx.step_context_hash("mkdir -p build");
        let h_mkdir_then_sh = ctx.step_context_hash("mkdir -p build\nsh -c true");
        let h_mkdir_then_true = ctx.step_context_hash("mkdir -p build\ntrue");
        assert_ne!(h_mkdir_only, h_mkdir_then_sh,
            "second line's tool MUST contribute to the fingerprint");
        assert_ne!(h_mkdir_then_sh, h_mkdir_then_true,
            "different second-line tool MUST yield different fingerprint");
    }

    #[test]
    fn step_context_hash_skips_env_prefix() {
        // `LC_ALL=C make foo` should fingerprint `make`, not collapse to the
        // `LC_ALL=C`-as-tool sentinel that the pre-fix code produced.
        let ctx = ExecutionContext::probe();
        let (Some(_), Some(_)) = (which::which("sh").ok(), which::which("true").ok()) else {
            eprintln!("skipping: sh or true not on PATH");
            return;
        };
        let h_sh = ctx.step_context_hash("sh -c true");
        let h_env_sh = ctx.step_context_hash("LC_ALL=C sh -c true");
        let h_env_true = ctx.step_context_hash("LC_ALL=C true");
        assert_eq!(h_sh, h_env_sh,
            "leading env-prefix MUST be skipped; tool component identical");
        assert_ne!(h_env_sh, h_env_true,
            "env-prefix bug fix: different tools after env prefix MUST differ");
    }

    #[test]
    fn step_context_hash_deterministic_across_line_order() {
        // The implementation deduplicates and folds tool hashes in BTreeSet
        // order (canonical realpath), so two scripts that resolve the same
        // set of tools — regardless of textual order — produce the same
        // fingerprint. This is intentional and documented.
        let ctx = ExecutionContext::probe();
        let (Some(_), Some(_)) = (which::which("sh").ok(), which::which("true").ok()) else {
            eprintln!("skipping: sh or true not on PATH");
            return;
        };
        let h_a = ctx.step_context_hash("sh -c true\ntrue");
        let h_b = ctx.step_context_hash("true\nsh -c true");
        assert_eq!(h_a, h_b,
            "fold order is BTreeSet-deterministic on canonical realpath");
    }

    #[test]
    fn step_context_hash_ignores_blank_and_comment_lines() {
        let ctx = ExecutionContext::probe();
        let Some(_) = which::which("sh").ok() else {
            eprintln!("skipping: sh not on PATH");
            return;
        };
        let h_a = ctx.step_context_hash("sh -c true");
        let h_b = ctx.step_context_hash("\n# leading comment\nsh -c true\n\n");
        assert_eq!(h_a, h_b);
    }

    #[test]
    fn is_env_prefix_token_recognises_assignments() {
        assert!(is_env_prefix_token("LC_ALL=C"));
        assert!(is_env_prefix_token("CFLAGS=-O2"));
        assert!(is_env_prefix_token("_PRIV=x"));
        assert!(is_env_prefix_token("FOO="));
        // Not env prefixes:
        assert!(!is_env_prefix_token("--flag=val"), "long-flag is not an env prefix");
        assert!(!is_env_prefix_token("=oops"), "missing name");
        assert!(!is_env_prefix_token("1FOO=x"), "name starts with digit");
        assert!(!is_env_prefix_token("foo.bar=x"), "name has invalid char");
        assert!(!is_env_prefix_token("plainword"), "no equals");
    }
}
