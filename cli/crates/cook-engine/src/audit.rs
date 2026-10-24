//! `cook audit`: re-run each unit under strace and report every observed read
//! that no declared input or seal covers.
//!
//! The declared side comes from `cook_cache::resolve_declared_inputs` (the one
//! resolver the key computation uses) and the re-run from
//! [`crate::verify::rerun_in_sandbox`] (the one re-run path `cache verify`
//! uses). The audit never changes a key and never touches the artifact store.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::RegisteredWorkspace;
use cook_contracts::consumes::ConsumesFilter;
use cook_contracts::pathlaw::normalize;
use cook_contracts::WorkPayload;

pub mod baseline;
mod covers;
mod env;
pub use env::{unobserved_programs, AUDIT_IGNORED_ENV};
pub(crate) use env::{keyed_env, unkeyed_env};
use env::read_queried;

/// One path a traced process touched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Access {
    pub path: PathBuf,
    /// The lookup failed with ENOENT/ENOTDIR.
    pub absent: bool,
    pub write: bool,
}

/// Open set, so consumers must tolerate kinds they do not know. `Env`: root =
/// the variable name, `paths` empty. `EnvUnobserved`: root = a program the
/// unit ran that never loaded the env recorder (statically linked, or started
/// with a scrubbed environment), so what it reads is unknown; `paths` empty.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FindingKind {
    Read,
    Absent,
    Env,
    #[serde(rename = "env-unobserved")]
    EnvUnobserved,
}

impl FindingKind {
    /// Every kind; the test beside it fails to compile when a variant is missing.
    pub const ALL: [FindingKind; 4] = [FindingKind::Read, FindingKind::Absent, FindingKind::Env, FindingKind::EnvUnobserved];

    /// The spelling in JSON, the baseline file and the human lines.
    pub fn as_str(self) -> &'static str {
        match self {
            FindingKind::Read => "read",
            FindingKind::Absent => "absent",
            FindingKind::Env => "env",
            FindingKind::EnvUnobserved => "env-unobserved",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Finding {
    pub recipe: String,
    pub kind: FindingKind,
    pub root: String,
    pub count: usize,
    pub paths: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct UnitAudit {
    pub recipe: String,
    pub unit: String,
    pub key: String,
    /// "traced" | "error"
    pub status: &'static str,
    pub detail: String,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct AuditReport {
    pub units: Vec<UnitAudit>,
    pub findings: Vec<Finding>,
    /// Every recipe in the audited closure, with or without units.
    #[serde(skip)]
    pub recipes: BTreeSet<String>,
    /// Every recipe the Cookfile registers, in the closure or not.
    #[serde(skip)]
    pub known: BTreeSet<String>,
}

impl UnitAudit {
    /// The one test for a unit that could not be traced.
    pub fn errored(&self) -> bool {
        self.status == "error"
    }
}

impl AuditReport {
    /// The one exit rule: 1 on any errored unit; findings also fail unless
    /// `accepting` (`--baseline` takes them in).
    pub fn exit_code(&self, accepting: bool) -> i32 {
        if self.errors() > 0 || (!accepting && !self.findings.is_empty()) { 1 } else { 0 }
    }
    pub fn errors(&self) -> usize {
        self.units.iter().filter(|u| u.errored()).count()
    }
}

// ---------------------------------------------------------------------------
// strace parsing
// ---------------------------------------------------------------------------

/// Split top-level arguments. Strings, `[..]`/`{..}`/`(..)` and `-y` fd
/// annotations (`3</a,b>`) keep their commas.
fn split_args(s: &str) -> Vec<String> {
    let (mut out, mut cur, mut depth) = (Vec::new(), String::new(), 0usize);
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        match c {
            '"' => {
                cur.push(c);
                while let Some(d) = it.next() {
                    cur.push(d);
                    if d == '\\' {
                        cur.extend(it.next());
                    } else if d == '"' {
                        break;
                    }
                }
            }
            '<' if depth == 0 && {
                let t = cur.trim();
                t == "AT_FDCWD" || (!t.is_empty() && t.bytes().all(|b| b.is_ascii_digit()))
            } => {
                cur.push(c);
                while let Some(d) = it.next() {
                    cur.push(d);
                    if d == '>' && matches!(it.peek(), None | Some(',')) {
                        break;
                    }
                }
            }
            '[' | '{' | '(' => {
                depth += 1;
                cur.push(c);
            }
            ']' | '}' | ')' => {
                depth = depth.saturating_sub(1);
                cur.push(c);
            }
            ',' if depth == 0 => out.push(std::mem::take(&mut cur).trim().to_string()),
            _ => cur.push(c),
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

/// Decode a strace string literal (`"..."`, optional trailing `...`).
fn unquote(arg: &str) -> Option<String> {
    let inner = arg.strip_prefix('"')?;
    let mut bytes = Vec::new();
    let mut it = inner.bytes().peekable();
    while let Some(b) = it.next() {
        match b {
            b'"' => break,
            b'\\' => match it.next()? {
                b'n' => bytes.push(b'\n'),
                b't' => bytes.push(b'\t'),
                b'r' => bytes.push(b'\r'),
                b'x' => {
                    let h: String = (0..2).filter_map(|_| it.next()).map(char::from).collect();
                    bytes.push(u8::from_str_radix(&h, 16).ok()?);
                }
                other => bytes.push(other),
            },
            b => bytes.push(b),
        }
    }
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// The `/p` of `AT_FDCWD</p>` / `3</p>`.
fn fd_path(arg: &str) -> Option<&str> {
    let (head, rest) = arg.split_once('<')?;
    let ok = head == "AT_FDCWD" || (!head.is_empty() && head.bytes().all(|b| b.is_ascii_digit()));
    rest.strip_suffix('>').filter(|_| ok)
}

const OPEN_CALLS: &[&str] = &["open", "openat", "openat2"];
/// Writes of the first path only.
/// Only calls that create a name or replace its contents: a path first met
/// this way was never an input. A write that keeps existing bytes (an
/// `O_APPEND`/`O_RDWR`/plain `O_CREAT` open, `truncate`) is a read.
const WRITE_FIRST: &[&str] = &["creat", "mkdir", "mkdirat", "unlink", "unlinkat", "rmdir", "mknod", "mknodat"];
/// Writes of every path argument.
const WRITE_ALL: &[&str] = &["rename", "renameat", "renameat2"];
/// Writes of the LAST path argument only: the first is a link target, not touched.
const WRITE_LAST: &[&str] = &["link", "linkat", "symlink", "symlinkat"];
/// Metadata-only calls read no content and create nothing.
const NOT_ACCESSES: &[&str] = &[
    "chdir", "fchdir", "clone", "clone3", "fork", "vfork", "getcwd", "chmod", "fchmodat", "fchmodat2",
    "chown", "lchown", "fchownat", "utime", "utimes", "utimensat", "futimesat",
];

/// Parse an `strace -f -qq -x -y` log into path accesses, in trace order.
/// `start_cwd` is where the first traced process started.
pub fn parse_trace(text: &str, start_cwd: &Path) -> Vec<Access> {
    trace_calls(text, start_cwd).into_iter().map(|(_, _, a)| a).collect()
}

/// [`parse_trace`]'s body: each access with the pid and syscall that made it.
fn trace_calls(text: &str, start_cwd: &Path) -> Vec<(u32, String, Access)> {
    let mut out = Vec::new();
    let mut cwds: BTreeMap<u32, PathBuf> = BTreeMap::new();
    let mut pending: BTreeMap<u32, String> = BTreeMap::new();
    for line in text.lines() {
        let (pid, rest) = match line.split_once(' ') {
            Some((p, r)) if p.bytes().all(|b| b.is_ascii_digit()) && !p.is_empty() => {
                (p.parse().unwrap_or(0), r.trim_start())
            }
            _ => (0, line),
        };
        if rest.starts_with("+++") || rest.starts_with("---") {
            continue;
        }
        let call = if let Some(head) = rest.strip_suffix("<unfinished ...>") {
            pending.insert(pid, head.to_string());
            continue;
        } else if rest.starts_with("<...") {
            let Some((_, tail)) = rest.split_once("resumed>") else { continue };
            match pending.remove(&pid) {
                Some(head) => head + tail,
                None => continue,
            }
        } else {
            rest.to_string()
        };
        let Some((name, after)) = call.split_once('(') else { continue };
        let Some(split) = after.rfind(") = ") else { continue };
        let (args, ret) = (&after[..split], after[split + 4..].trim());
        let cwd = cwds.entry(pid).or_insert_with(|| start_cwd.to_path_buf()).clone();
        let args = split_args(args);
        let mut base = cwd.clone();
        let mut new_cwd = cwd.clone();
        let mut strings = Vec::new();
        let mut flags = String::new();
        for a in &args {
            if let Some(p) = fd_path(a).filter(|p| p.starts_with('/')) {
                base = PathBuf::from(p);
                if a.starts_with("AT_FDCWD") {
                    new_cwd = base.clone();
                }
            } else if a == "AT_FDCWD" {
                base = cwd.clone();
            } else if a.starts_with('"') {
                strings.push(unquote(a).map(|s| (s, base.clone())));
            } else {
                flags.push_str(a);
                flags.push('|');
            }
        }
        cwds.insert(pid, new_cwd.clone());
        let mut tokens = ret.split_whitespace();
        let status = tokens.next().unwrap_or("?");
        let failed = status == "-1";
        let errno = tokens.next().unwrap_or("");
        match name {
            "chdir" if !failed => {
                if let Some(Some((p, b))) = strings.first() {
                    cwds.insert(pid, normalize(&b.join(p)));
                }
            }
            "fchdir" if !failed => {
                if let Some(p) = args.first().and_then(|a| fd_path(a)) {
                    cwds.insert(pid, PathBuf::from(p));
                }
            }
            "clone" | "clone3" | "fork" | "vfork" => {
                if let Ok(child) = status.parse::<u32>() {
                    if child > 0 {
                        cwds.insert(child, new_cwd);
                    }
                }
            }
            _ => {}
        }
        if NOT_ACCESSES.contains(&name) || (failed && !matches!(errno, "ENOENT" | "ENOTDIR")) || status == "?" {
            continue;
        }
        let all: Vec<PathBuf> =
            strings.into_iter().flatten().filter(|(p, _)| !p.is_empty()).map(|(p, b)| normalize(&b.join(p))).collect();
        let paths: Vec<(PathBuf, bool)> = if WRITE_ALL.contains(&name) {
            all.into_iter().map(|p| (p, true)).collect()
        } else if WRITE_LAST.contains(&name) {
            all.into_iter().last().map(|p| (p, true)).into_iter().collect()
        } else if OPEN_CALLS.contains(&name)
            && ["O_WRONLY", "O_RDWR", "O_CREAT", "O_TRUNC"].iter().any(|f| flags.contains(f))
        {
            let first = all.into_iter().next();
            if flags.contains("O_TRUNC") || (flags.contains("O_CREAT") && flags.contains("O_EXCL")) {
                first.map(|p| (p, true)).into_iter().collect()
            } else {
                // Keeps whatever bytes are there (append, read-write, plain
                // create): a read of an existing file, then a write.
                first.into_iter().flat_map(|p| [(p.clone(), false), (p, true)]).collect()
            }
        } else {
            let write = WRITE_FIRST.contains(&name);
            // Anything else (reads, statfs, xattr, inotify, truncate, ...): its first path.
            all.into_iter().next().map(|p| (p, write)).into_iter().collect()
        };
        for (path, write) in paths {
            out.push((pid, name.to_string(), Access { path, absent: failed, write }));
        }
    }
    out
}

// ---------------------------------------------------------------------------
// collapse
// ---------------------------------------------------------------------------

/// Project-relative, `~/rel` under `home`, else absolute. A project or home
/// path embedded in a relative result (a tool joining an absolute path under a
/// relative dir: `public/home/me/proj/a.svg`) is replaced too, so a finding
/// never carries the machine's paths. A path that stayed absolute is under
/// neither, so it is left alone (`/srv/home/me/x` is not `/srv/~/x`).
fn spelling(path: &Path, project_root: &Path, home: Option<&Path>) -> String {
    let s = if let Ok(rel) = path.strip_prefix(project_root) {
        let s = rel.to_string_lossy();
        if s.is_empty() { ".".into() } else { s.into_owned() }
    } else if let Some(rel) = home.and_then(|h| path.strip_prefix(h).ok()) {
        format!("~/{}", rel.to_string_lossy())
    } else {
        return path.to_string_lossy().into_owned();
    };
    let s = elide(&s, project_root, "<project>");
    home.map_or(s.clone(), |h| elide(&s, h, "~"))
}

/// Replace each `/<root>` occurrence ending at a component boundary by `/<to>`.
fn elide(s: &str, root: &Path, to: &str) -> String {
    let root = root.to_string_lossy();
    let root = root.trim_end_matches('/');
    if root.is_empty() {
        return s.to_string();
    }
    let (mut out, mut at) = (String::new(), 0);
    for (i, _) in s.match_indices(root) {
        let end = i + root.len();
        if i >= at && (end == s.len() || s[end..].starts_with('/')) {
            out.push_str(&s[at..i]);
            out.push('/');
            out.push_str(to);
            at = end;
        }
    }
    out.push_str(&s[at..]);
    out
}

fn root_of(spelled: &str) -> String {
    let comps: Vec<&str> = spelled.split('/').filter(|c| !c.is_empty()).collect();
    if spelled.starts_with("~/") {
        comps.iter().take(2).copied().collect::<Vec<_>>().join("/")
    } else if spelled.starts_with('/') {
        format!("/{}", comps.iter().take(2).copied().collect::<Vec<_>>().join("/"))
    } else {
        comps.first().copied().unwrap_or(".").to_string()
    }
}

/// Group one recipe's uncovered hits per (kind, root): sorted distinct
/// spellings, count = how many.
pub fn collapse(
    recipe: &str,
    hits: &[(PathBuf, FindingKind)],
    project_root: &Path,
    home: Option<&Path>,
) -> Vec<Finding> {
    let mut groups: BTreeMap<(FindingKind, String), BTreeSet<String>> = BTreeMap::new();
    for (path, kind) in hits {
        let s = spelling(path, project_root, home);
        groups.entry((*kind, root_of(&s))).or_default().insert(s);
    }
    groups
        .into_iter()
        .map(|((kind, root), paths)| Finding {
            recipe: recipe.to_string(),
            kind,
            root,
            count: paths.len(),
            paths: paths.into_iter().collect(),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// audit
// ---------------------------------------------------------------------------

fn ignored(p: &Path, ignore: &[PathBuf]) -> bool {
    p.components().any(|c| c.as_os_str() == cook_contracts::layout::DOT_COOK_DIR)
        || ignore.iter().any(|i| p.starts_with(i))
}

/// What a unit's declarations cover. `exact` paths cover themselves; a
/// `subtrees` entry covers itself and everything under it (a `covers` clause, a
/// toolchain root). A `patterns` entry is an absolute glob (`*` stays within a
/// component, `**` crosses) whose every match covers itself and everything
/// under it. Ancestors of exact paths and subtrees are covered too (see
/// [`uncovered_reads`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Coverage {
    pub exact: BTreeSet<PathBuf>,
    pub subtrees: BTreeSet<PathBuf>,
    pub patterns: Vec<String>,
}

/// The reads nothing covers. Covered: a declared path (exact), a path under a
/// covered subtree, an ancestor of anything covered or of the working dir, or a path the unit itself created
/// or overwrote before ever looking at it (first access a write or an absent
/// lookup; read-then-rewrite, as `sed -i` does, stays a read). No path covers
/// its descendants: cook keys a declared input per file (a directory path is
/// not walked), so a directory's contents are not keyed by declaring it.
pub(crate) fn uncovered_reads(
    accesses: &[Access],
    coverage: &Coverage,
    wd: &Path,
    project_root: &Path,
    ignore: &[PathBuf],
) -> Vec<(PathBuf, FindingKind)> {
    // path -> was its first access a write or an absent lookup
    let mut fresh: BTreeMap<&Path, bool> = BTreeMap::new();
    for a in accesses {
        fresh.entry(&a.path).or_insert(a.write || a.absent);
    }
    let mut covered = coverage.exact.clone();
    covered.extend(accesses.iter().filter(|a| a.write && fresh[a.path.as_path()]).map(|a| a.path.clone()));
    let ancestors: BTreeSet<PathBuf> = covered
        .iter()
        .chain(&coverage.subtrees)
        .map(PathBuf::as_path)
        .chain([wd])
        .flat_map(Path::ancestors)
        .map(Path::to_path_buf)
        .collect();
    // validated by `unit_coverage`, which refuses a unit with a bad entry
    let patterns = ConsumesFilter::compile(&coverage.patterns).expect("coverage patterns are validated");
    let mut hits = Vec::new();
    for a in accesses.iter().filter(|a| !a.write) {
        let inside = a.path.starts_with(project_root);
        if (!inside && ignored(&a.path, ignore)) || ignored(&a.path, &[]) || (a.absent && !inside) {
            continue;
        }
        if ancestors.contains(&a.path)
            || coverage.subtrees.iter().any(|t| a.path.starts_with(t))
            || a.path.ancestors().any(|p| patterns.matches(&p.to_string_lossy()))
        {
            continue;
        }
        hits.push((a.path.clone(), if a.absent { FindingKind::Absent } else { FindingKind::Read }));
    }
    hits
}

/// What a unit's seals add to its [`Coverage`]: a sealed `files` declaration's
/// resolved paths and a sealed tool's resolved locations (exact), and the
/// `covers` roots of a sealed probe or `files` declaration (a literal root a
/// subtree, a glob a pattern); an entry that is not a valid glob is an error.
/// Probes carry no recipe scope: every recipe's list holds the whole registry,
/// so the unit's own list is the one place to look.
fn unit_coverage(
    coverage: &mut Coverage,
    seal_keys: &BTreeSet<cook_contracts::LocalProbeKey>,
    units: &cook_contracts::RecipeUnits,
    workspace: &RegisteredWorkspace,
    probe_values: &cook_probe::store::ProbeValueStore,
    recipe_name: &str,
    home: Option<&Path>,
) -> Result<(), String> {
    let wd = &units.working_dir;
    let store = probe_values.for_recipe(recipe_name);
    for k in seal_keys {
        let probe = units.probes.iter().find(|p| &p.key == k);
        let prefix = cook_contracts::probe_key::qualify_for_recipe(recipe_name, k);
        let dir = workspace.working_dir_by_prefix.get(prefix.import_prefix()).unwrap_or(wd);
        for entry in probe.map_or(&[][..], |p| &p.covers[..]) {
            match covers::resolve(entry, dir, home)? {
                Some(covers::Cover::Subtree(t)) => drop(coverage.subtrees.insert(t)),
                Some(covers::Cover::Pattern(p)) => coverage.patterns.push(p),
                None => {}
            }
        }
        if probe.is_some_and(|p| p.produce_source == cook_contracts::probe_value::FILES_MANIFEST_PRODUCE) {
            if let Some(serde_json::Value::Object(m)) = store.get(k).and_then(|b| serde_json::from_slice(&b).ok()) {
                coverage.exact.extend(m.keys().map(|p| normalize(&dir.join(p))));
            }
        }
        if let Some(tools) = store.tool_paths(k) {
            coverage.exact.extend(tools.values().map(PathBuf::from));
        }
    }
    Ok(())
}

/// Re-run every shell unit of the reachable recipes under strace and report
/// the reads nothing declares. Must run inside the isolated project view
/// ([`crate::sandbox::isolate`]): the re-runs execute in place. Sequential, and a failed re-run's trace is
/// discarded (ponytail: parallel re-runs / partial traces if audit time or
/// failing units become a problem).
pub fn audit(
    project_root: &Path,
    workspace: &RegisteredWorkspace,
    edges: &BTreeMap<String, Vec<String>>,
    reachable: &BTreeSet<String>,
    num_jobs: usize,
    cache_ctx: Arc<cook_cache::CacheContext>,
    failed_puts: Arc<cook_cache::cache_ctx::FailedPuts>,
) -> Result<AuditReport, String> {
    let isolated = crate::sandbox::isolated_root().and_then(|r| r.canonicalize().ok());
    if isolated.is_none() || isolated != project_root.canonicalize().ok() {
        return Err(NOT_ISOLATED.to_string());
    }
    // The caller refuses members symlinked out of the project before
    // registration (cook-cli `audit_isolated`): registration already writes.
    // An output reached through a symlink out of the project would be written
    // by the populate run before anything else looked, so refuse it first.
    let root = isolated.as_deref().unwrap_or(project_root);
    for recipe in reachable {
        let Some(units) = workspace.units_by_recipe.get(recipe) else { continue };
        for unit in &units.units {
            let Some(meta) = &unit.cache_meta else { continue };
            for out in &meta.output_paths {
                if let Some(real) = crate::verify::resolves_outside(root, &units.working_dir, out) {
                    return Err(format!(
                        "recipe `{recipe}` declares output `{out}`, which resolves outside the project ({}); the audit cannot isolate it",
                        real.display()
                    ));
                }
            }
        }
    }
    audit_in_place(project_root, workspace, edges, reachable, num_jobs, cache_ctx, failed_puts)
}

/// Refuse a project whose `.cook` — the project root's, or that of any
/// working dir in `dirs` (every workspace member's, the entry Cookfile's
/// included) — resolves outside it (`proj/.cook -> ../state`): registration and the
/// populate run write cook's own state there, past the overlay. `root` must
/// be canonical.
pub fn refuse_state_outside<'a>(root: &'a Path, dirs: impl IntoIterator<Item = &'a Path>) -> Result<(), String> {
    for dir in std::iter::once(root).chain(dirs) {
        if let Some(real) = crate::verify::resolves_outside(root, dir, cook_contracts::layout::DOT_COOK_DIR) {
            return Err(format!(
                "`.cook` resolves outside the project: {}/.cook is {}; the audit cannot isolate it",
                dir.display(),
                real.display()
            ));
        }
    }
    Ok(())
}

/// What the CLI puts before the reason an audit cannot run on this host.
pub const UNSUPPORTED_HERE: &str = "audit: unsupported here: ";

/// Refuse a workspace member whose directory resolves outside the project
/// (a symlinked member, or `..` past the root): it would be registered, built
/// and cleared in the real tree. `root` must be canonical.
pub fn refuse_members_outside<'a>(
    root: &'a Path,
    members: impl IntoIterator<Item = (&'a str, &'a Path)>,
) -> Result<(), String> {
    for (name, dir) in members {
        if crate::verify::resolves_outside(root, dir, ".").is_some() {
            return Err(format!(
                "workspace member `{name}` is {}, outside the project; the audit cannot isolate it",
                dir.display()
            ));
        }
    }
    Ok(())
}

/// Why an audit refuses to run outside [`crate::sandbox::isolate`]'s view.
pub const NOT_ISOLATED: &str = "audit must run inside the isolated project view";

/// The audit's one cache context, for registration and the populate run
/// alike: a throwaway local store at `store`, publishing off, so the user's
/// artifact store (local or remote) is neither read nor written.
pub fn scratch_cache_ctx(project_root: &Path, store: &Path) -> Result<Arc<cook_cache::CacheContext>, String> {
    crate::run::build_cache_ctx_with(
        project_root,
        true,
        Some(&|c: &cook_cache::CloudConfig| -> Arc<dyn cook_cache::CacheBackend> {
            Arc::new(cook_cache::LocalBackend::with_config(store.to_path_buf(), c.backend_config()))
        }),
    )
    .map_err(|e| format!("audit cache: {e}"))
}

/// [`audit`]'s body. The caller guarantees the tree under `project_root` is
/// disposable: the populate run and every re-run write into it.
pub(crate) fn audit_in_place(
    project_root: &Path,
    workspace: &RegisteredWorkspace,
    edges: &BTreeMap<String, Vec<String>>,
    reachable: &BTreeSet<String>,
    num_jobs: usize,
    cache_ctx: Arc<cook_cache::CacheContext>,
    failed_puts: Arc<cook_cache::cache_ctx::FailedPuts>,
) -> Result<AuditReport, String> {
    let denylist = cache_ctx.denylist.clone();
    let run_result = crate::run::run_with_cache_ctx(cache_ctx, workspace, edges, reachable, num_jobs, &[], true, failed_puts, |_e| {})
        .map_err(|e| format!("populate run failed: {e}"))?;

    let mut ignore: Vec<PathBuf> = ["/proc", "/dev", "/sys", "/tmp", "/var/tmp"].map(PathBuf::from).into();
    ignore.push(std::env::temp_dir());
    ignore.push(cook_cache::cloud_config::user_cache_root());
    // A HOME of `/` (CI containers), empty or relative is no home: everything, or
    // any matching component, would be spelled `~`.
    let home = std::env::var_os("HOME").map(PathBuf::from).filter(|h| h.is_absolute() && h.parent().is_some());

    let order = cook_contracts::unit_graph::toposort_recipes(edges, reachable).map_err(|e| e.to_string())?;
    // The recorder is preloaded into every traced re-run; off Linux there is
    // none and the audit is unsupported anyway.
    // Same scratch the re-run uses (verify's fallback outside an isolated
    // view), as a directory of our own that is removed when the audit ends.
    let scratch_root = crate::verify::scratch_root()?;
    let scratch_guard = tempfile::tempdir_in(&scratch_root).map_err(|e| format!("env recorder: {e}"))?;
    let scratch = scratch_guard.path();
    let recorder = cook_envtrace::install(scratch).map_err(|e| match e.kind() {
        std::io::ErrorKind::Unsupported => format!("{UNSUPPORTED_HERE}{e}"),
        _ => format!("env recorder: {e}"),
    })?;
    let mut report = AuditReport { known: workspace.names.iter().map(|n| n.name.clone()).collect(), ..Default::default() };
    for recipe_name in order {
        report.recipes.insert(recipe_name.clone());
        let Some(units) = workspace.units_by_recipe.get(&recipe_name) else { continue };
        let wd = &units.working_dir;
        let mut hits: Vec<(PathBuf, FindingKind)> = Vec::new();
        let mut env_names: BTreeSet<String> = BTreeSet::new();
        let mut unobserved: BTreeSet<String> = BTreeSet::new();
        for (n, unit) in units.units.iter().enumerate() {
            let (Some(meta), WorkPayload::Shell { cmd, .. }) = (&unit.cache_meta, &unit.payload) else {
                continue;
            };
            let label = meta.output_paths.first().cloned().unwrap_or_else(|| unit.payload.display_name());
            let mut ua = UnitAudit {
                recipe: recipe_name.clone(),
                unit: label,
                key: meta.cache_key.clone(),
                status: "traced",
                detail: String::new(),
            };
            let inputs: BTreeSet<PathBuf> = cook_cache::resolve_declared_inputs(&meta.inputs, &meta.consumes, wd)
                .iter()
                .map(|p| normalize(&wd.join(p)))
                .collect();
            // Steps keep the environment they inherit; only the recorder is added.
            let names_file = scratch.join(format!("envtrace-{recipe_name}-{n}"));
            let env = recorder.as_ref().map_or_else(BTreeMap::new, |so| {
                cook_envtrace::env_for(so, &names_file)
            });
            let rerun = crate::verify::rerun_in_sandbox(
                cmd,
                &run_result.probe_values,
                &recipe_name,
                wd,
                &env,
                &meta.output_paths,
                &inputs,
                true,
            );
            match rerun {
                Err(e) => {
                    let _ = std::fs::remove_file(&names_file);
                    ua.status = "error";
                    ua.detail = e;
                }
                Ok(rerun) => {
                    let trace = rerun.trace.as_deref().unwrap_or("");
                    let accesses = parse_trace(trace, wd);
                    if let Some(so) = &recorder {
                        unobserved.extend(
                            unobserved_programs(trace, wd, so)
                                .into_iter()
                                .map(|p| spelling(&p, project_root, home.as_deref())),
                        );
                    }
                    // Env findings come from the names file and the trace, not
                    // from coverage, so a coverage error still reports them.
                    let keyed = keyed_env(meta, units);
                    env_names.extend(unkeyed_env(&read_queried(&names_file), &keyed, &denylist));
                    let mut coverage = Coverage { exact: inputs, ..Coverage::default() };
                    coverage.exact.extend(rerun.outputs.keys().map(|p| normalize(&wd.join(p))));
                    match unit_coverage(&mut coverage, &meta.seal_keys, units, workspace, &run_result.probe_values, &recipe_name, home.as_deref()) {
                        Ok(()) => hits.extend(uncovered_reads(&accesses, &coverage, wd, project_root, &ignore)),
                        Err(e) => {
                            ua.status = "error";
                            ua.detail = e;
                        }
                    }
                }
            }
            report.units.push(ua);
        }
        report.findings.extend(collapse(&recipe_name, &hits, project_root, home.as_deref()));
        report.findings.extend(env_names.into_iter().map(|root| Finding {
            recipe: recipe_name.clone(),
            kind: FindingKind::Env,
            root,
            count: 1,
            paths: Vec::new(),
        }));
        report.findings.extend(unobserved.into_iter().map(|root| Finding {
            recipe: recipe_name.clone(),
            kind: FindingKind::EnvUnobserved,
            root,
            count: 1,
            paths: Vec::new(),
        }));
    }
    report.findings.sort_by(|a, b| (&a.recipe, a.kind, &a.root).cmp(&(&b.recipe, b.kind, &b.root)));
    Ok(report)
}

#[cfg(test)]
#[path = "tests/audit_tests.rs"]
mod tests;
