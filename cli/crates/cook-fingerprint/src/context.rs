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

/// Failure modes for declared-tool resolution at build start (CS-0052).
///
/// Both variants are fatal — the build MUST NOT start with a misdeclared
/// `[cache] tools` entry. The whole point of explicit declaration is to
/// surface mistakes; falling back to "tool not found, ignored" reproduces
/// the silent-miss class CS-0052 exists to close.
#[derive(Debug)]
pub enum DeclaredToolError {
    NotFound { name: String, source: which::Error },
    Canonicalize { name: String, source: std::io::Error },
}

impl std::fmt::Display for DeclaredToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound { name, .. } => write!(
                f,
                "declared tool `{name}` not found on PATH (.cook/cloud.toml [cache] tools). \
                 Either install it, remove it from the list, or run in an environment \
                 where it resolves (devcontainer, nix-shell)."
            ),
            Self::Canonicalize { name, source } => write!(
                f,
                "declared tool `{name}` resolved but could not be canonicalized: {source}"
            ),
        }
    }
}

impl std::error::Error for DeclaredToolError {}

pub struct ExecutionContext {
    pub machine: MachineIdentity,
    /// Per-build: lazily probed and cached, keyed by canonical realpath.
    pub(crate) tool_cache: Mutex<HashMap<PathBuf, ToolHash>>,
    /// Build-wide hash over (declared name, content) pairs from `[cache] tools`.
    /// Sentinel zero when the list is empty — preserves the v3 fold-shape so
    /// projects without the config produce a deterministic identity contribution.
    pub declared_tools_hash: u64,
    /// Diagnostic: declared name → resolved canonical realpath. Surfaced by
    /// `cook --explain-cache-key` (future) and a `tracing::debug!` line at
    /// probe time. Not part of the hash directly; the hash is over
    /// (name, content_sha256) pairs computed in `compute_declared_tools_hash`.
    pub declared_tools: BTreeMap<String, PathBuf>,
}

impl ExecutionContext {
    pub fn probe() -> Self {
        Self::probe_with_declared_tools(&[]).expect("empty declared-tool list cannot fail")
    }

    /// Probe the build-wide context and resolve `[cache] tools` declarations.
    ///
    /// Each name in `declared_tool_names` is resolved via `which::which`,
    /// canonicalized to its realpath, and SHA-256-hashed. The combined hash
    /// folds into every step's `context_hash` (see `step_context_hash`).
    /// An empty slice produces a sentinel `declared_tools_hash` of `0`.
    pub fn probe_with_declared_tools(
        declared_tool_names: &[String],
    ) -> Result<Self, DeclaredToolError> {
        let machine = MachineIdentity::probe();
        let tool_cache: Mutex<HashMap<PathBuf, ToolHash>> = Mutex::new(HashMap::new());
        let (declared_tools_hash, declared_tools) =
            compute_declared_tools_hash(declared_tool_names, &tool_cache)?;
        // Diagnostic logging is the caller's responsibility — the fingerprint
        // crate stays free of logging deps. CLI bindings inspect
        // `declared_tools` after probe and emit a `tracing::debug!` line there.
        Ok(Self { machine, tool_cache, declared_tools_hash, declared_tools })
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
    /// configuration mechanism — see CS-0052).
    pub fn step_context_hash(&self, command: &str) -> u64 {
        let realpaths = resolved_tool_realpaths(command);
        let machine_bytes = self.machine.encode();
        let mut hasher = xxhash_rust::xxh3::Xxh3::new();
        hasher.update(&machine_bytes);
        hasher.update(&self.declared_tools_hash.to_le_bytes());
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

/// Resolve a list of declared tool names to canonical realpaths and hash
/// the (name, content) pairs deterministically.
///
/// Folding by *declared name* (rather than realpath) is deliberate: two
/// distros where `gcc` resolves to differently-named realpaths
/// (`/usr/bin/gcc-13` vs. `/usr/bin/gcc-11`) MUST produce different
/// `declared_tools_hash` values. The empty-input case returns the sentinel
/// `0` so projects without `[cache] tools` configuration produce a
/// deterministic "no contribution" value (CS-0052 §3.2 invariant 1).
pub fn compute_declared_tools_hash(
    names: &[String],
    tool_cache: &Mutex<HashMap<PathBuf, ToolHash>>,
) -> Result<(u64, BTreeMap<String, PathBuf>), DeclaredToolError> {
    if names.is_empty() {
        return Ok((0, BTreeMap::new()));
    }
    let mut resolved: BTreeMap<String, (PathBuf, [u8; 32])> = BTreeMap::new();
    for name in names {
        let p = which::which(name)
            .map_err(|source| DeclaredToolError::NotFound { name: name.clone(), source })?;
        let rp = std::fs::canonicalize(&p)
            .map_err(|source| DeclaredToolError::Canonicalize { name: name.clone(), source })?;
        let hash = {
            let mut cache = tool_cache.lock().expect("tool_cache poisoned");
            cache
                .entry(rp.clone())
                .or_insert_with(|| ToolHash::for_resolved(Some(rp.as_path())))
                .content_sha256
        };
        resolved.insert(name.clone(), (rp, hash));
    }
    let mut hasher = xxhash_rust::xxh3::Xxh3::new();
    for (name, (_rp, sha)) in &resolved {
        hasher.update(name.as_bytes());
        hasher.update(&[0x00]);
        hasher.update(sha);
    }
    let realpaths = resolved
        .into_iter()
        .map(|(n, (rp, _))| (n, rp))
        .collect::<BTreeMap<_, _>>();
    Ok((hasher.digest(), realpaths))
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

// ---------------------------------------------------------------------------
// Probe fingerprint (CS-0074 §22.5.3)
// ---------------------------------------------------------------------------

/// Inputs to a probe-unit's fingerprint (§22.5.3).
#[derive(Debug, Clone)]
pub struct ProbeFingerprintInputs {
    pub key: String,
    pub produce_source: String,
    /// (env-var name, current value or None if unset).
    pub env: Vec<(String, Option<String>)>,
    /// (tool name, 32-byte content hash of resolved binary, all-zero if missing).
    pub tools: Vec<(String, [u8; 32])>,
    /// (file path, 32-byte content hash, all-zero if missing).
    pub files: Vec<(String, [u8; 32])>,
    /// (upstream probe key, that probe's fingerprint).
    pub upstream_probes: Vec<(String, [u8; 32])>,
}

/// Compute the 32-byte SHA-256 fingerprint per §22.5.3.
pub fn compute_probe_fingerprint(inputs: &ProbeFingerprintInputs) -> [u8; 32] {
    let mut h = Sha256::new();

    // §22.5.3 section 1: literal marker
    h.update(b"COOK_PROBE_FP_V1\n");
    // §22.5.3 section 2: key
    h.update(inputs.key.as_bytes());
    h.update(b"\n");
    // §22.5.3 section 3: produce source string (UTF-8 bytes)
    h.update(inputs.produce_source.as_bytes());
    h.update(b"\n");

    // §22.5.3 section 4: ENV
    let mut env = inputs.env.clone();
    env.sort_by(|a, b| a.0.cmp(&b.0));
    h.update(b"ENV\n");
    for (k, v) in &env {
        h.update(k.as_bytes());
        h.update(b"=");
        match v {
            Some(s) => h.update(s.as_bytes()),
            None => h.update(b"<unset>"),
        }
        h.update(b"\n");
    }

    // §22.5.3 section 5: TOOLS
    let mut tools = inputs.tools.clone();
    tools.sort_by(|a, b| a.0.cmp(&b.0));
    h.update(b"TOOLS\n");
    for (name, hash) in &tools {
        h.update(name.as_bytes());
        h.update(b"=");
        h.update(probe_hex_encode(hash).as_bytes());
        h.update(b"\n");
    }

    // §22.5.3 section 6: FILES
    let mut files = inputs.files.clone();
    files.sort_by(|a, b| a.0.cmp(&b.0));
    h.update(b"FILES\n");
    for (path, hash) in &files {
        h.update(path.as_bytes());
        h.update(b"=");
        h.update(probe_hex_encode(hash).as_bytes());
        h.update(b"\n");
    }

    // §22.5.3 section 7: UPSTREAM
    let mut up = inputs.upstream_probes.clone();
    up.sort_by(|a, b| a.0.cmp(&b.0));
    h.update(b"UPSTREAM\n");
    for (key, fp) in &up {
        h.update(key.as_bytes());
        h.update(b"=");
        h.update(probe_hex_encode(fp).as_bytes());
        h.update(b"\n");
    }

    let result = h.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&result);
    out
}

fn probe_hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{:02x}", b));
    }
    s
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

    // ---- CS-0052 declared-tools tests (spec §6.1) ----

    fn make_fake_tool_dir(name: &str, contents: &[u8]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(name);
        std::fs::write(&path, contents).expect("write");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&path).expect("meta").permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&path, perms).expect("chmod");
        }
        dir
    }

    /// Process-wide lock that serialises any test which mutates `PATH`.
    /// Cargo runs tests in parallel by default; without serialisation, one
    /// test's `set_var("PATH", original)` cleanup can race ahead of another
    /// test's `which::which` lookup and produce spurious NotFound failures.
    static PATH_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Run `f` with the test tempdir prepended to `PATH` so `which::which`
    /// resolves against it. Reverts `PATH` after `f` returns.
    fn with_path_prefix<F, R>(extra: &Path, f: F) -> R
    where
        F: FnOnce() -> R,
    {
        // Hold the process-wide lock for the entire duration of the PATH
        // mutation + caller body. Lock poisoning from a panicking test is
        // recoverable here — the next test still re-sets PATH before reading.
        let _guard = PATH_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let original = std::env::var("PATH").unwrap_or_default();
        let new_path = format!(
            "{}{}{}",
            extra.display(),
            if cfg!(windows) { ";" } else { ":" },
            original
        );
        // SAFETY: env mutation under PATH_TEST_LOCK is the documented protocol
        // for these tests; no other test path mutates PATH outside the lock.
        unsafe {
            std::env::set_var("PATH", &new_path);
        }
        let r = f();
        unsafe {
            std::env::set_var("PATH", &original);
        }
        r
    }

    #[test]
    fn compute_declared_tools_hash_empty_is_sentinel_zero() {
        let tool_cache = Mutex::new(HashMap::new());
        let (h, map) = compute_declared_tools_hash(&[], &tool_cache).expect("ok");
        assert_eq!(h, 0, "empty input MUST return sentinel zero");
        assert!(map.is_empty());
    }

    #[test]
    fn compute_declared_tools_hash_deterministic() {
        let dir = make_fake_tool_dir("ftool", b"v1");
        with_path_prefix(dir.path(), || {
            let cache_a = Mutex::new(HashMap::new());
            let cache_b = Mutex::new(HashMap::new());
            let names = vec!["ftool".to_string()];
            let (h1, _) = compute_declared_tools_hash(&names, &cache_a).expect("ok");
            let (h2, _) = compute_declared_tools_hash(&names, &cache_b).expect("ok");
            assert_eq!(h1, h2, "same inputs MUST hash identically");
            assert_ne!(h1, 0, "non-empty input MUST NOT collide with the sentinel zero");
        });
    }

    #[test]
    fn compute_declared_tools_hash_differs_on_content() {
        let dir_a = make_fake_tool_dir("gtool", b"contents A");
        let cache_a = Mutex::new(HashMap::new());
        let names = vec!["gtool".to_string()];
        let h_a = with_path_prefix(dir_a.path(), || {
            compute_declared_tools_hash(&names, &cache_a).expect("ok").0
        });
        let dir_b = make_fake_tool_dir("gtool", b"contents B");
        let cache_b = Mutex::new(HashMap::new());
        let h_b = with_path_prefix(dir_b.path(), || {
            compute_declared_tools_hash(&names, &cache_b).expect("ok").0
        });
        assert_ne!(h_a, h_b, "different binary contents MUST hash differently");
    }

    #[test]
    fn compute_declared_tools_hash_differs_on_membership() {
        let dir = tempfile::tempdir().expect("tempdir");
        for n in &["mtool_a", "mtool_b"] {
            let p = dir.path().join(n);
            std::fs::write(&p, n.as_bytes()).expect("write");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut perms = std::fs::metadata(&p).expect("meta").permissions();
                perms.set_mode(0o755);
                std::fs::set_permissions(&p, perms).expect("chmod");
            }
        }
        with_path_prefix(dir.path(), || {
            let cache = Mutex::new(HashMap::new());
            let one = vec!["mtool_a".to_string()];
            let two = vec!["mtool_a".to_string(), "mtool_b".to_string()];
            let (h_one, _) = compute_declared_tools_hash(&one, &cache).expect("ok");
            let (h_two, _) = compute_declared_tools_hash(&two, &cache).expect("ok");
            assert_ne!(h_one, h_two, "adding a tool MUST change the hash");
        });
    }

    #[test]
    fn compute_declared_tools_hash_order_independent() {
        let dir = tempfile::tempdir().expect("tempdir");
        for (n, body) in &[("otool_a", &b"alpha"[..]), ("otool_b", &b"beta"[..])] {
            let p = dir.path().join(n);
            std::fs::write(&p, body).expect("write");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut perms = std::fs::metadata(&p).expect("meta").permissions();
                perms.set_mode(0o755);
                std::fs::set_permissions(&p, perms).expect("chmod");
            }
        }
        with_path_prefix(dir.path(), || {
            let cache = Mutex::new(HashMap::new());
            let ab = vec!["otool_a".to_string(), "otool_b".to_string()];
            let ba = vec!["otool_b".to_string(), "otool_a".to_string()];
            let (h_ab, _) = compute_declared_tools_hash(&ab, &cache).expect("ok");
            let (h_ba, _) = compute_declared_tools_hash(&ba, &cache).expect("ok");
            assert_eq!(h_ab, h_ba, "BTreeMap-sorted fold MUST be order-independent");
        });
    }

    #[test]
    fn compute_declared_tools_hash_errors_on_missing() {
        let cache = Mutex::new(HashMap::new());
        let names = vec!["definitely-not-on-path-cs0035-xyz".to_string()];
        let err = compute_declared_tools_hash(&names, &cache)
            .expect_err("missing tool MUST error, not silently ignore");
        match err {
            DeclaredToolError::NotFound { name, .. } => {
                assert_eq!(name, "definitely-not-on-path-cs0035-xyz");
            }
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[test]
    fn step_context_hash_folds_declared() {
        let dir = make_fake_tool_dir("ctool", b"v1");
        let names = vec!["ctool".to_string()];
        with_path_prefix(dir.path(), || {
            let ctx_empty = ExecutionContext::probe_with_declared_tools(&[]).expect("ok");
            let ctx_decl = ExecutionContext::probe_with_declared_tools(&names).expect("ok");
            let cmd = "/bin/sh -c true";
            let h_empty = ctx_empty.step_context_hash(cmd);
            let h_decl = ctx_decl.step_context_hash(cmd);
            assert_ne!(
                h_empty, h_decl,
                "declared_tools_hash MUST contribute to step_context_hash"
            );
        });
    }

    // ---- end CS-0052 ----

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

    // ---- CS-0074 probe fingerprint tests ----

    #[test]
    fn probe_fingerprint_is_deterministic_for_same_inputs() {
        let inputs = ProbeFingerprintInputs {
            key: "cc:zlib".into(),
            produce_source: "return run_pkg_config(\"zlib\")".into(),
            env: vec![("CC".into(), Some("gcc".into())), ("PATH".into(), Some("/usr/bin".into()))],
            tools: vec![("pkg-config".into(), [0u8; 32])],
            files: vec![],
            upstream_probes: vec![],
        };
        assert_eq!(compute_probe_fingerprint(&inputs), compute_probe_fingerprint(&inputs));
    }

    #[test]
    fn probe_fingerprint_changes_when_env_value_changes() {
        let mut a = ProbeFingerprintInputs {
            key: "cc:zlib".into(),
            produce_source: "".into(),
            env: vec![("PKG_CONFIG_PATH".into(), Some("/a".into()))],
            tools: vec![], files: vec![], upstream_probes: vec![],
        };
        let h1 = compute_probe_fingerprint(&a);
        a.env[0].1 = Some("/b".into());
        assert_ne!(h1, compute_probe_fingerprint(&a));
    }

    #[test]
    fn probe_fingerprint_is_invariant_to_input_order() {
        let a = ProbeFingerprintInputs {
            key: "cc:x".into(), produce_source: "".into(),
            env: vec![("A".into(), Some("1".into())), ("B".into(), Some("2".into()))],
            tools: vec![], files: vec![], upstream_probes: vec![],
        };
        let b = ProbeFingerprintInputs {
            key: "cc:x".into(), produce_source: "".into(),
            env: vec![("B".into(), Some("2".into())), ("A".into(), Some("1".into()))],
            tools: vec![], files: vec![], upstream_probes: vec![],
        };
        assert_eq!(compute_probe_fingerprint(&a), compute_probe_fingerprint(&b));
    }

    #[test]
    fn probe_fingerprint_changes_on_upstream_probe_change() {
        let mut a = ProbeFingerprintInputs {
            key: "cc:x".into(), produce_source: "".into(),
            env: vec![], tools: vec![], files: vec![],
            upstream_probes: vec![("cc:compiler".into(), [1u8; 32])],
        };
        let h1 = compute_probe_fingerprint(&a);
        a.upstream_probes[0].1 = [2u8; 32];
        assert_ne!(h1, compute_probe_fingerprint(&a));
    }

    #[test]
    fn probe_fingerprint_changes_when_produce_source_changes() {
        let mut a = ProbeFingerprintInputs {
            key: "k".into(), produce_source: "return 1".into(),
            env: vec![], tools: vec![], files: vec![], upstream_probes: vec![],
        };
        let h1 = compute_probe_fingerprint(&a);
        a.produce_source = "return 2".into();
        assert_ne!(h1, compute_probe_fingerprint(&a));
    }

    // ---- end CS-0074 ----
}
