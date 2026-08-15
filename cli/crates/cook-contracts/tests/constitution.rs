//! The constitution in this crate's README, as a gate.
//!
//! `README.md` beside this file is normative for the workspace and states one
//! enforceable rule: **no decision is implemented twice**. Until this file
//! existed nothing enforced it. Every violation was found by a human or an
//! agent reading code, in five separate hand audits, and nothing caught
//! anything in between.
//!
//! It lives here, next to the document it enforces, for the reason that
//! document gives about itself: prose asks, a test refuses. Two purity rules
//! were written in that README with equal authority; the one backed by
//! `tests/layout.rs` held for the life of the crate, and the one backed by
//! nothing drifted to seven dependencies and two `remove_*` calls without
//! anyone noticing.
//!
//! # How a rule is shaped
//!
//! Every rule is a pure function from text to findings, and every rule carries
//! a **mutation test**: a synthetic input containing a deliberate violation
//! that must produce a finding. This is not ceremony. The filesystem ban was
//! green and blind at the same time for as long as it scanned `use`
//! statements only, because `path.is_dir()` needs no import; a reviewer caught
//! what the gate could not. A rule nobody mutated is decoration.
//!
//! The same blind spot has a general form, and [`reached_paths`] is the answer
//! to it: a rule that pattern-matches source text sees the spelling it
//! imagined. `use std::time::Instant` and `use std::time::{Duration, Instant}`
//! are the same reach and look nothing alike, so paths are expanded once,
//! centrally, and rules ask about reaches rather than about text.
//!
//! # Why a baseline instead of a clean slate
//!
//! A gate that lands red blocks everything behind it and gets disabled. The
//! rules that already hold are enforced outright; the rules that describe an
//! intent this tree has not reached yet read a tracked waiver file of the
//! violations that already exist, so the gate lands green and fails only on
//! what is *new*. That file is then the shrinking to-do list, and it is
//! deliberately costly to add to: an entry without a written justification is
//! itself a failure, because the deliberate-copy protocol permits a copy only
//! when somebody justifies it, and every duplication this repo has found "was
//! once labelled a deliberate copy by someone with a good reason that later
//! expired".
//!
//! A waiver whose violation has *gone* is also a failure. Otherwise the list
//! only ever grows, and a list that never shrinks stops being read.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

// ---------------------------------------------------------------- the corpus

/// One production source file: read once, shared by every rule.
///
/// "Production" excludes anything under a `tests/` directory, matching the
/// test-layout convention that puts every test body behind one `**/tests/**`
/// glob. A rule that fired on test code would be unusable: tests duplicate
/// each other on purpose.
pub struct Source {
    /// The crate directory name, e.g. `cook-engine`.
    pub krate: String,
    /// Workspace-relative, forward slashes, e.g. `crates/cook-engine/src/run.rs`.
    pub path: String,
    pub text: String,
}

/// One rule's answer about one place in the tree.
pub struct Finding {
    /// What the waiver file is keyed on.
    ///
    /// Deliberately not a line number: an unrelated edit above a known
    /// violation must not re-fire it, or the baseline needs rewriting on every
    /// commit and stops being reviewed.
    pub key: String,
    /// Where to look, in the order a reader should look.
    pub sites: Vec<String>,
    /// One line saying what is wrong, for the failure output.
    pub detail: String,
}

/// The `cli/` workspace root, from this crate's manifest directory.
pub fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("cook-contracts sits at <workspace>/crates/cook-contracts")
        .to_path_buf()
}

/// Every production source in the workspace, sorted, read once.
pub fn corpus() -> Vec<Source> {
    fn visit(dir: &Path, krate: &str, root: &Path, out: &mut Vec<Source>) {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
            .unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
            .map(|entry| entry.expect("read source entry").path())
            .collect();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                if path.file_name().is_none_or(|name| name != "tests") {
                    visit(&path, krate, root, out);
                }
                continue;
            }
            if path.extension().is_none_or(|extension| extension != "rs") {
                continue;
            }
            out.push(Source {
                krate: krate.to_string(),
                path: relative(&path, root),
                text: std::fs::read_to_string(&path)
                    .unwrap_or_else(|e| panic!("read {}: {e}", path.display())),
            });
        }
    }

    let root = workspace_root();
    let mut out = Vec::new();
    for crate_dir in workspace_crate_dirs() {
        let src = crate_dir.join("src");
        if !src.is_dir() {
            continue;
        }
        visit(&src, &crate_name(&crate_dir), &root, &mut out);
    }
    assert!(
        out.len() > 100,
        "the corpus walk found only {} files, which means it is looking in the wrong place; \
         a rule that scans nothing passes everything",
        out.len()
    );
    out
}

fn workspace_crate_dirs() -> Vec<PathBuf> {
    let mut crates: Vec<PathBuf> = std::fs::read_dir(workspace_root().join("crates"))
        .expect("read crates directory")
        .map(|entry| entry.expect("read crate entry").path())
        .filter(|path| path.join("Cargo.toml").is_file())
        .collect();
    crates.sort();
    crates
}

fn crate_name(dir: &Path) -> String {
    dir.file_name()
        .expect("crate directory has a name")
        .to_string_lossy()
        .into_owned()
}

fn relative(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

// ------------------------------------------------------------ reading source

/// One pass over a source that blanks what a rule must not read, replacing it
/// with spaces so every byte offset, and therefore every line number, survives.
///
/// Two rules need two views of the same file and must not disagree about where
/// a comment ends, so there is one scanner with one switch. `Comments` keeps
/// string contents, for the rule that compares literals across crates.
/// `CommentsAndStrings` blanks them too, for every rule that reads code: a
/// module path named inside a diagnostic message is prose, not a reach, and a
/// scanner that cannot tell the difference reports `std::fs` in a sentence
/// explaining that `std::fs` is banned.
#[derive(Clone, Copy, PartialEq)]
pub enum Scrub {
    Comments,
    CommentsAndStrings,
}

/// Blanks comments (and optionally string contents), preserving byte length.
pub fn scrub(text: &str, what: Scrub) -> String {
    let bytes = text.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;

    fn blank(out: &mut Vec<u8>, span: &[u8]) {
        out.extend(
            span.iter()
                .map(|byte| if *byte == b'\n' { b'\n' } else { b' ' }),
        );
    }

    while i < bytes.len() {
        if bytes[i..].starts_with(b"//") {
            let end = bytes[i..]
                .iter()
                .position(|byte| *byte == b'\n')
                .map_or(bytes.len(), |at| i + at);
            blank(&mut out, &bytes[i..end]);
            i = end;
            continue;
        }
        if bytes[i..].starts_with(b"/*") {
            let start = i;
            let mut depth = 0usize;
            while i < bytes.len() {
                if bytes[i..].starts_with(b"/*") {
                    depth += 1;
                    i += 2;
                } else if bytes[i..].starts_with(b"*/") {
                    depth -= 1;
                    i += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    i += 1;
                }
            }
            blank(&mut out, &bytes[start..i]);
            continue;
        }
        if let Some(end) = char_literal_end(bytes, i) {
            // Before the string branch: `'"'` is a quote character, not the
            // start of a literal, and mistaking it swallows the rest of the
            // file up to the next quote.
            out.extend_from_slice(&bytes[i..end]);
            i = end;
            continue;
        }
        if let Some((content, end)) = string_span(bytes, i) {
            match what {
                Scrub::Comments => out.extend_from_slice(&bytes[i..end]),
                Scrub::CommentsAndStrings => {
                    out.extend_from_slice(&bytes[i..content.start]);
                    blank(&mut out, &bytes[content.clone()]);
                    out.extend_from_slice(&bytes[content.end..end]);
                }
            }
            i = end;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).expect("blanking preserves byte lengths, so UTF-8 survives")
}

/// The content range and the offset just past a string literal starting at
/// `at`, covering both `"…"` and the raw `r#"…"#` form.
fn string_span(bytes: &[u8], at: usize) -> Option<(std::ops::Range<usize>, usize)> {
    if bytes[at] == b'"' {
        let mut i = at + 1;
        while i < bytes.len() {
            match bytes[i] {
                b'\\' => i += 2,
                b'"' => return Some((at + 1..i, i + 1)),
                _ => i += 1,
            }
        }
        return Some((at + 1..bytes.len(), bytes.len()));
    }
    if bytes[at] != b'r' || (at > 0 && is_ident_char(bytes[at - 1])) {
        return None;
    }
    let hashes = bytes[at + 1..]
        .iter()
        .take_while(|byte| **byte == b'#')
        .count();
    if bytes.get(at + 1 + hashes) != Some(&b'"') {
        return None;
    }
    let start = at + hashes + 2;
    let mut close = vec![b'"'];
    close.extend(std::iter::repeat_n(b'#', hashes));
    bytes[start..]
        .windows(close.len())
        .position(|window| window == close.as_slice())
        .map_or(Some((start..bytes.len(), bytes.len())), |offset| {
            Some((start..start + offset, start + offset + close.len()))
        })
}

/// The offset just past a `'x'` / `'\n'` character literal starting at `at`.
///
/// A lifetime (`'a`) is not one, and neither is the `'` in `&'_ str`, so the
/// closing quote has to be there before this claims a span.
fn char_literal_end(bytes: &[u8], at: usize) -> Option<usize> {
    if bytes[at] != b'\'' {
        return None;
    }
    let end = if bytes.get(at + 1) == Some(&b'\\') {
        (at + 2..bytes.len().min(at + 8)).find(|i| bytes[*i] == b'\'')?
    } else {
        let width = std::str::from_utf8(&bytes[at + 1..bytes.len().min(at + 5)])
            .ok()
            .and_then(|rest| rest.chars().next())
            .map_or(1, char::len_utf8);
        (bytes.get(at + 1 + width) == Some(&b'\'')).then_some(at + 1 + width)?
    };
    Some(end + 1)
}

/// Every path this source reaches, as `(line, path)`, comments removed.
///
/// `use` trees are expanded, so `use std::time::{Duration, Instant}` yields
/// `std::time::Duration` *and* `std::time::Instant` rather than `std::time` —
/// exactly the distinction the effect budget turns on, since a `Duration` is a
/// value and an `Instant` is a clock. Inline paths (`std::fs::read_to_string(…)`)
/// are collected the same way, so a rule never has to care which spelling a
/// reach was written in. That indifference is the point: the guard this
/// replaces matched one spelling and let the other through.
pub fn reached_paths(text: &str) -> Vec<(usize, String)> {
    let stripped = scrub(text, Scrub::CommentsAndStrings);
    let bytes = stripped.as_bytes();
    let mut line_of = LineIndex::new(&stripped);
    let mut out = Vec::new();
    let mut i = 0;

    while i < bytes.len() {
        if let Some(after) = use_keyword_at(&stripped, i) {
            let end = statement_end(&stripped, after);
            expand_use_tree(&stripped[after..end], "", &mut |path| {
                out.push((line_of.line(i), path));
            });
            i = end;
            continue;
        }
        if is_ident_start(bytes[i]) && !preceded_by_ident_char(bytes, i) {
            let (path, next) = inline_path(&stripped, i);
            if path.contains("::") {
                out.push((line_of.line(i), path));
            }
            i = next;
            continue;
        }
        i += 1;
    }
    out
}

/// Byte offset just past a `use` keyword starting a statement at `at`.
fn use_keyword_at(text: &str, at: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    if !bytes[at..].starts_with(b"use") || preceded_by_ident_char(bytes, at) {
        return None;
    }
    let after = at + 3;
    (bytes
        .get(after)
        .is_some_and(|byte| byte.is_ascii_whitespace()))
    .then(|| after + text[after..].len() - text[after..].trim_start().len())
}

fn statement_end(text: &str, from: usize) -> usize {
    let bytes = text.as_bytes();
    let mut depth = 0usize;
    for (offset, byte) in bytes[from..].iter().enumerate() {
        match byte {
            b'{' => depth += 1,
            b'}' => depth = depth.saturating_sub(1),
            b';' if depth == 0 => return from + offset,
            _ => {}
        }
    }
    bytes.len()
}

/// Expands one `use` tree body into full paths, dropping `as` renames and `*`.
fn expand_use_tree(tree: &str, prefix: &str, emit: &mut impl FnMut(String)) {
    let tree = tree.trim();
    if tree.is_empty() {
        return;
    }
    if let Some(open) = find_group_open(tree) {
        let head = tree[..open].trim().trim_end_matches("::").trim();
        let close = matching_brace(tree, open);
        let joined = join(prefix, head);
        for member in split_top_level(&tree[open + 1..close]) {
            expand_use_tree(member, &joined, emit);
        }
        return;
    }
    let leaf = tree.split(" as ").next().unwrap_or(tree).trim();
    if leaf == "*" || leaf.is_empty() {
        return;
    }
    let joined = join(prefix, leaf);
    if !joined.is_empty() {
        emit(joined);
    }
}

fn join(prefix: &str, tail: &str) -> String {
    match (prefix.is_empty(), tail.is_empty()) {
        (true, _) => tail.to_string(),
        (_, true) => prefix.to_string(),
        _ => format!("{prefix}::{tail}"),
    }
}

fn find_group_open(tree: &str) -> Option<usize> {
    tree.find('{')
}

fn matching_brace(text: &str, open: usize) -> usize {
    let bytes = text.as_bytes();
    let mut depth = 0usize;
    for (offset, byte) in bytes[open..].iter().enumerate() {
        match byte {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return open + offset;
                }
            }
            _ => {}
        }
    }
    bytes.len()
}

fn split_top_level(members: &str) -> Vec<&str> {
    let bytes = members.as_bytes();
    let (mut depth, mut start, mut out) = (0usize, 0usize, Vec::new());
    for (offset, byte) in bytes.iter().enumerate() {
        match byte {
            b'{' => depth += 1,
            b'}' => depth = depth.saturating_sub(1),
            b',' if depth == 0 => {
                out.push(&members[start..offset]);
                start = offset + 1;
            }
            _ => {}
        }
    }
    out.push(&members[start..]);
    out.into_iter()
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .collect()
}

/// The `a::b::c` chain starting at `at`, and the offset just past it.
fn inline_path(text: &str, at: usize) -> (String, usize) {
    let bytes = text.as_bytes();
    let mut end = at;
    let mut path = String::new();
    loop {
        let ident_end = ident_end(bytes, end);
        path.push_str(&text[end..ident_end]);
        end = ident_end;
        if text[end..].starts_with("::") && bytes.get(end + 2).is_some_and(|b| is_ident_start(*b)) {
            path.push_str("::");
            end += 2;
            continue;
        }
        return (path, end.max(at + 1));
    }
}

fn ident_end(bytes: &[u8], from: usize) -> usize {
    let mut end = from;
    while end < bytes.len() && is_ident_char(bytes[end]) {
        end += 1;
    }
    end
}

fn is_ident_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_'
}

fn is_ident_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn preceded_by_ident_char(bytes: &[u8], at: usize) -> bool {
    at > 0 && (is_ident_char(bytes[at - 1]) || bytes[at - 1] == b':')
}

/// Byte offset to 1-based line, walked forward once per source.
struct LineIndex<'a> {
    text: &'a str,
    at: usize,
    line: usize,
}

impl<'a> LineIndex<'a> {
    fn new(text: &'a str) -> Self {
        Self {
            text,
            at: 0,
            line: 1,
        }
    }

    fn line(&mut self, offset: usize) -> usize {
        if offset < self.at {
            self.at = 0;
            self.line = 1;
        }
        self.line += self.text[self.at..offset.min(self.text.len())]
            .matches('\n')
            .count();
        self.at = offset.min(self.text.len());
        self.line
    }
}

// --------------------------------------------------------------- the ratchet

/// The tracked file recording violations that already existed when a rule
/// landed, each with the justification that keeps it.
pub struct Waivers {
    rule: &'static str,
    path: PathBuf,
    entries: BTreeMap<String, String>,
}

impl Waivers {
    /// Loads `crates/cook-contracts/constitution/<rule>.jsonl`.
    ///
    /// One JSON object per line: `{"key": …, "why": …}`, plus an optional
    /// `"sites"` carried for the reader and ignored here. A missing file means
    /// no waivers, which is the right reading for a rule that has never had a
    /// violation.
    pub fn load(rule: &'static str) -> Self {
        let path = waiver_path(rule);
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        Self {
            rule,
            entries: parse_waivers(rule, &relative(&path, &workspace_root()), &text),
            path,
        }
    }
}

fn waiver_path(rule: &str) -> PathBuf {
    workspace_root()
        .join("crates/cook-contracts/constitution")
        .join(format!("{rule}.jsonl"))
}

/// Parses the waiver format, refusing an entry that waives without saying why.
///
/// Separate from IO so the format's rules are testable without a file.
pub fn parse_waivers(rule: &str, origin: &str, text: &str) -> BTreeMap<String, String> {
    let mut entries = BTreeMap::new();
    for (index, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let at = format!("{origin}:{}", index + 1);
        let value: serde_json::Value = serde_json::from_str(line)
            .unwrap_or_else(|e| panic!("{at}: waiver line is not JSON: {e}"));
        let key = value
            .get("key")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_else(|| panic!("{at}: waiver has no \"key\""));
        let why = value
            .get("why")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .trim();
        assert!(
            !why.is_empty(),
            "{at}: this {rule} waiver has no \"why\". A waived violation is a decision \
             somebody made; write down why it is deliberate and what would have to change \
             for it to stop being deliberate. Every duplication this repo has found was once \
             a deliberate copy with a good reason that later expired."
        );
        if let Some(previous) = entries.insert(key.to_string(), why.to_string()) {
            panic!("{at}: duplicate waiver for {key:?} (previously waived: {previous})");
        }
    }
    entries
}

/// Compares a rule's findings against its waivers, returning the failure
/// report or `None` when the rule is satisfied.
///
/// Two ways to fail, and the second matters as much as the first: a finding
/// nobody waived is new duplication, and a waiver nothing matches is a list
/// that has stopped describing the tree.
pub fn verdict(waivers: &Waivers, findings: &[Finding]) -> Option<String> {
    let found: BTreeMap<&str, &Finding> = findings
        .iter()
        .map(|finding| (finding.key.as_str(), finding))
        .collect();
    let file = relative(&waivers.path, &workspace_root());

    let mut report = String::new();
    for (key, finding) in found
        .iter()
        .filter(|(key, _)| !waivers.entries.contains_key(**key))
    {
        let _ = writeln!(report, "\n  NEW  {}", finding.detail);
        for site in &finding.sites {
            let _ = writeln!(report, "         {site}");
        }
        let _ = writeln!(
            report,
            "       If this really is a deliberate copy, waive it by adding one line to\n       \
             {file}:\n         {}",
            serde_json::json!({
                "key": key,
                "sites": finding.sites,
                "why": "<why this copy is deliberate, and what would end that>",
            })
        );
    }
    for key in waivers
        .entries
        .keys()
        .filter(|key| !found.contains_key(key.as_str()))
    {
        let _ = writeln!(
            report,
            "\n  GONE {key}\n       This waiver no longer matches anything in the tree. Delete \
             the line from\n       {file} — a list that only grows stops being read.",
        );
    }

    (!report.is_empty()).then(|| {
        format!(
            "the constitution's `{}` rule is not satisfied.\n\
             See crates/cook-contracts/README.md — no decision is implemented twice.\n{report}",
            waivers.rule
        )
    })
}

/// Runs one rule against its waiver file and fails the test with the report.
pub fn enforce(rule: &'static str, findings: &[Finding]) {
    let waivers = Waivers::load(rule);
    if let Some(report) = verdict(&waivers, findings) {
        panic!("{report}");
    }
    println!(
        "constitution: {rule} clean ({} waived; that file is the to-do list)",
        waivers.entries.len()
    );
}

// ------------------------------------------------- tier 1: the effect budget

/// Reaches that are effects, and therefore inadmissible in this crate.
///
/// The bar is about effects, not dependencies: `xxhash`, `sha2` and `globset`
/// compute, they do not act, and holding a hash out of this crate for the
/// dependency it needs is the exact mistake the README records as having cost
/// a year. `std::time::Duration` is a value and stays legal; `Instant` and
/// `SystemTime` are clocks and do not.
const BANNED_REACHES: [&str; 6] = [
    "std::fs",
    "std::env",
    "std::process",
    "std::time::Instant",
    "std::time::SystemTime",
    "mlua",
];

/// Effects with no path to ban, reached as a method on a value.
///
/// This list is the reason the rule exists in this shape. `path.is_dir()`
/// imports nothing, so an import-scanning guard reports clean while filesystem
/// access sits in the crate; that is not hypothetical, it is what shipped past
/// the previous guard and was caught by a human reviewer instead.
const BANNED_METHODS: [&str; 10] = [
    ".canonicalize()",
    ".exists()",
    ".is_dir()",
    ".is_file()",
    ".metadata()",
    ".read_dir()",
    ".symlink_metadata()",
    ".try_exists()",
    "Instant::now()",
    "SystemTime::now()",
];

/// Mutable process-global state, which the README counts as reaching the world
/// "even when the grep comes back clean".
const BANNED_GLOBALS: [&str; 5] = [
    "static mut ",
    "thread_local!",
    "OnceLock",
    "LazyLock",
    "AtomicUsize",
];

/// Every effect this source reaches, as `line: reason`.
pub fn effect_violations(text: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    for (line, path) in reached_paths(text) {
        for banned in BANNED_REACHES {
            if path == banned || path.starts_with(&format!("{banned}::")) {
                out.push((line, format!("reaches `{path}`")));
            }
        }
    }
    let stripped = scrub(text, Scrub::CommentsAndStrings);
    for (index, line) in stripped.lines().enumerate() {
        let compact: String = line.chars().filter(|ch| !ch.is_whitespace()).collect();
        for banned in BANNED_METHODS {
            if compact.contains(banned) {
                out.push((index + 1, format!("reaches the world via `{banned}`")));
            }
        }
        for banned in BANNED_GLOBALS {
            if line.contains(banned) {
                out.push((
                    index + 1,
                    format!("holds mutable process-global state (`{}`)", banned.trim()),
                ));
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// This crate is pure, and nothing but a test says so.
///
/// Enforced outright rather than baselined: the tree satisfies it today, so a
/// waiver file here would only ever be a place to put the first violation.
#[test]
fn cook_contracts_reaches_no_effects() {
    let violations: Vec<String> = corpus()
        .iter()
        .filter(|source| source.krate == "cook-contracts")
        .flat_map(|source| {
            effect_violations(&source.text)
                .into_iter()
                .map(|(line, reason)| format!("{}:{line} {reason}", source.path))
        })
        .collect();

    assert!(
        violations.is_empty(),
        "cook-contracts must stay effect-free — its admission bar is purity, and a law that \
         needs the world belongs in the crate that owns the world:\n  {}",
        violations.join("\n  ")
    );
}

/// What this crate may depend on, so the budget cannot widen by accident.
///
/// The list is not a purity claim about each entry — it is the review record.
/// Nothing checked this before, so a dependency could arrive with an effect
/// inside it and the effect rule above would never see the reach.
const ALLOWED_DEPENDENCIES: [&str; 6] = [
    "serde",
    "serde_json",
    "xxhash-rust",
    "sha2",
    "globset",
    "glob",
];

/// Dependency names in a manifest's `[dependencies]` section.
pub fn declared_dependencies(manifest: &str) -> Vec<String> {
    manifest
        .lines()
        .skip_while(|line| line.trim() != "[dependencies]")
        .skip(1)
        .take_while(|line| !line.trim_start().starts_with('['))
        .filter_map(|line| {
            let line = line.trim();
            (!line.is_empty() && !line.starts_with('#'))
                .then(|| line.split(['=', ' ']).next().unwrap_or_default().trim())
                .filter(|name| !name.is_empty())
                .map(str::to_string)
        })
        .collect()
}

#[test]
fn cook_contracts_dependencies_are_allowlisted() {
    let manifest =
        std::fs::read_to_string(workspace_root().join("crates/cook-contracts/Cargo.toml"))
            .expect("read cook-contracts manifest");
    let declared = declared_dependencies(&manifest);
    let allowed: BTreeSet<&str> = ALLOWED_DEPENDENCIES.into_iter().collect();

    let added: Vec<&String> = declared
        .iter()
        .filter(|name| !allowed.contains(name.as_str()))
        .collect();
    assert!(
        added.is_empty(),
        "cook-contracts gained {added:?}. A dependency here is admissible when it computes \
         and inadmissible when it acts, and that is a judgement somebody has to make out \
         loud: add it to ALLOWED_DEPENDENCIES in this file with the reason, or find the \
         law a home that already owns the effect."
    );
    let removed: Vec<&&str> = ALLOWED_DEPENDENCIES
        .iter()
        .filter(|name| !declared.iter().any(|declared| declared == *name))
        .collect();
    assert!(
        removed.is_empty(),
        "ALLOWED_DEPENDENCIES still lists {removed:?}, which the manifest no longer has. \
         An allowlist that outlives what it allows is how a budget widens unnoticed."
    );
}

// ------------------------------------------- tier 1: stratum and re-exports

/// The declared layering, lowest first. A crate may depend only on crates in a
/// strictly lower stratum.
///
/// Cargo already refuses a cycle; what it cannot refuse is an edge that
/// inverts the pipeline — parsing reaching into orchestration, or the shared
/// language reaching into anything at all. Placing a new crate here is a
/// decision somebody makes on purpose, which is the point: the table is
/// checked to cover the workspace, so a new crate cannot arrive unplaced.
const STRATA: [(&str, &[&str]); 6] = [
    ("shared language", &["cook-contracts", "cook-dag"]),
    (
        "mechanism",
        &[
            "cook-shell",
            "cook-cache",
            "cook-lang",
            "cook-cookfile",
            "cook-progress",
            // A package manager: filesystem, subprocess, and one workspace
            // edge. It reaches nothing above it and nothing above it reaches
            // it except the surface that dispatches `cook modules`, which is
            // the whole reason it could leave cook-cli (COOK-420).
            "cook-modules",
        ],
    ),
    (
        "translation",
        &[
            "cook-probe",
            "cook-luagen",
            "cook-lua-stdlib",
            "cook-graph",
            "cook-logs",
        ],
    ),
    ("execution", &["cook-execute", "cook-register"]),
    ("orchestration", &["cook-engine", "cook-plan"]),
    ("surface", &["cook-cli"]),
];

fn stratum_of(krate: &str) -> Option<usize> {
    STRATA
        .iter()
        .position(|(_, members)| members.contains(&krate))
}

/// Workspace-internal dependencies declared by each crate.
pub fn internal_edges(manifests: &BTreeMap<String, String>) -> Vec<(String, String)> {
    let members: BTreeSet<&str> = manifests.keys().map(String::as_str).collect();
    let mut edges = Vec::new();
    for (krate, manifest) in manifests {
        for dependency in declared_dependencies(manifest) {
            if members.contains(dependency.as_str()) {
                edges.push((krate.clone(), dependency));
            }
        }
    }
    edges.sort();
    edges
}

fn workspace_manifests() -> BTreeMap<String, String> {
    workspace_crate_dirs()
        .into_iter()
        .map(|dir| {
            let manifest = std::fs::read_to_string(dir.join("Cargo.toml"))
                .unwrap_or_else(|e| panic!("read {}: {e}", dir.display()));
            (crate_name(&dir), manifest)
        })
        .collect()
}

#[test]
fn every_crate_is_placed_in_a_stratum() {
    let unplaced: Vec<String> = workspace_manifests()
        .into_keys()
        .filter(|krate| stratum_of(krate).is_none())
        .collect();
    assert!(
        unplaced.is_empty(),
        "these crates are not placed in STRATA: {unplaced:?}. A crate whose layer nobody \
         declared is a crate anything may depend on, which is how a fictional boundary \
         becomes an attractor for whatever fits nowhere."
    );
    let phantom: Vec<&str> = STRATA
        .iter()
        .flat_map(|(_, members)| members.iter().copied())
        .filter(|krate| !workspace_root().join("crates").join(krate).is_dir())
        .collect();
    assert!(
        phantom.is_empty(),
        "STRATA names crates that do not exist: {phantom:?}"
    );
}

#[test]
fn no_crate_depends_upwards_or_sideways() {
    let violations: Vec<String> = internal_edges(&workspace_manifests())
        .into_iter()
        .filter(|(from, to)| !descends_a_stratum(from, to))
        .map(|(from, to)| {
            format!(
                "{from} ({}) depends on {to} ({})",
                STRATA[stratum_of(&from).expect("placed")].0,
                STRATA[stratum_of(&to).expect("placed")].0
            )
        })
        .collect();
    assert!(
        violations.is_empty(),
        "these dependency edges do not descend a stratum:\n  {}\n\
         A law lives as low as its dependencies allow. If the edge is right, the table in \
         this file is wrong and should say so.",
        violations.join("\n  ")
    );
}

/// Crate paths reached through another crate's re-export, e.g. reading
/// `cook_engine::cook_cache::parse_size` because the direct edge is refused.
///
/// A path that tunnels two crates to reach an item is a mechanical signal that
/// the item is in the wrong crate: the consumer needs it, the owner is not a
/// declared dependency, and the crate in the middle is a courier.
pub fn re_export_tunnels(source: &Source, members: &BTreeSet<String>) -> Vec<Finding> {
    let is_member = |segment: &str| members.contains(&segment.replace('_', "-"));
    let mut findings = BTreeMap::new();
    for (line, path) in reached_paths(&source.text) {
        let mut segments = path.split("::");
        let (Some(first), Some(second)) = (segments.next(), segments.next()) else {
            continue;
        };
        if !is_member(first) || !is_member(second) {
            continue;
        }
        let key = format!("{} -> {first}::{second}", source.krate);
        let site = format!("{}:{line}", source.path);
        let sites = &mut findings
            .entry(key.clone())
            .or_insert_with(|| Finding {
                key,
                sites: Vec::new(),
                detail: format!(
                    "{} reaches {} through {}'s re-export, so the item's home is wrong or \
                     the edge it needs is missing",
                    source.krate,
                    second.replace('_', "-"),
                    first.replace('_', "-"),
                ),
            })
            .sites;
        // One line can name the same tunnel more than once; the reader wants
        // the places, not the occurrences.
        if sites.last() != Some(&site) {
            sites.push(site);
        }
    }
    findings.into_values().collect()
}

#[test]
fn no_item_is_reached_through_another_crates_re_export() {
    let corpus = corpus();
    let members: BTreeSet<String> = corpus.iter().map(|source| source.krate.clone()).collect();
    let mut findings: BTreeMap<String, Finding> = BTreeMap::new();
    for source in &corpus {
        for finding in re_export_tunnels(source, &members) {
            findings
                .entry(finding.key.clone())
                .and_modify(|existing| existing.sites.extend(finding.sites.iter().cloned()))
                .or_insert(finding);
        }
    }
    enforce(
        "re-export-tunnels",
        &findings.into_values().collect::<Vec<_>>(),
    );
}

// ------------------------------------------ tier 2: cross-crate duplicates

/// Below this, a shared literal is a coincidence rather than a contract.
const SHARED_LITERAL_MIN: usize = 8;

/// Every string literal in a source, with its line, comments already gone.
pub fn string_literals(text: &str) -> Vec<(usize, String)> {
    let scrubbed = scrub(text, Scrub::Comments);
    let bytes = scrubbed.as_bytes();
    let mut line_of = LineIndex::new(&scrubbed);
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if let Some(end) = char_literal_end(bytes, i) {
            i = end;
            continue;
        }
        if let Some((content, end)) = string_span(bytes, i) {
            out.push((line_of.line(i), scrubbed[content].to_string()));
            i = end;
            continue;
        }
        i += 1;
    }
    out
}

/// Literals that name a Rust item rather than state a decision.
///
/// These are structural noise in every Rust workspace, and excluding them by
/// construction is better than waiving them: a list padded with noise is a
/// list nobody reads. `#[path = "tests/naming_tests.rs"]` collides whenever
/// two crates have a module of the same name, which is a coincidence of
/// naming. `skip_serializing_if = "Option::is_none"` is a function path that
/// serde requires as a string; two crates writing it agree about nothing.
/// `rename_all = "kebab-case"` is serde's own vocabulary for a case
/// convention — the two ends of a wire format do have to agree on it, but they
/// agree by being ONE derive on ONE type, which is what moving a shared enum
/// into this crate achieves; two crates naming the convention separately for
/// unrelated types is the coincidence, not the agreement (COOK-421).
///
/// The `rename_all` case takes the LITERAL as well as the line, and that is
/// the point: `#[serde(rename_all = "kebab-case", rename = "some-wire-name")]`
/// must lose the convention and keep the wire name. A line-scoped exclusion
/// would silence both, which is how a noise filter starts hiding findings.
fn names_an_item_not_a_decision(line: &str, literal: &str) -> bool {
    if line.contains("#[path") || line.contains("skip_serializing_if") {
        return true;
    }
    // The value of a `rename_all = "..."`, and nothing else on the line.
    line.split("rename_all")
        .skip(1)
        .filter_map(|after| after.split_once('"'))
        .filter_map(|(before, rest)| before.trim().starts_with('=').then_some(rest))
        .any(|rest| rest.split('"').next() == Some(literal))
}

/// String literals of substance appearing in two or more crates.
///
/// This is the class the constitution names outright — "an emitter and a
/// consumer of the same literal" — and the one that produced `COOK_CMD_FAILED`
/// and `REGISTER_SURFACE_NAME` before anyone was looking for it. A shared
/// literal is a wire format whether or not anybody called it one, and the two
/// ends agree only by luck until one of them is edited.
pub fn duplicate_literals(corpus: &[Source]) -> Vec<Finding> {
    let mut seen: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    for source in corpus {
        let scrubbed = scrub(&source.text, Scrub::Comments);
        let lines: Vec<&str> = scrubbed.lines().collect();
        for (line, literal) in string_literals(&source.text) {
            if literal.chars().count() < SHARED_LITERAL_MIN
                || lines
                    .get(line - 1)
                    .is_some_and(|text| names_an_item_not_a_decision(text, &literal))
            {
                continue;
            }
            seen.entry(literal)
                .or_default()
                .entry(source.krate.clone())
                .or_insert_with(|| format!("{}:{line}", source.path));
        }
    }

    seen.into_iter()
        .filter(|(_, crates)| crates.len() > 1)
        .map(|(literal, crates)| Finding {
            key: literal.clone(),
            sites: crates.values().cloned().collect(),
            detail: format!(
                "the literal {literal:?} is written in {} crates; whichever end is edited \
                 first, the others keep the old bytes",
                crates.len()
            ),
        })
        .collect()
}

#[test]
fn no_literal_is_written_in_two_crates() {
    enforce("duplicate-literals", &duplicate_literals(&corpus()));
}

// ----------------------------------------------- tier 3: cross-crate clones

/// How many tokens must match before a run of code is a copy rather than an
/// idiom.
///
/// Measured, not chosen: at 30 tokens the workspace has 24 cross-crate groups
/// and roughly ten are real; at 45 it has one; at 20 it has 606 and is
/// unusable. Below this floor the matches are `.iter().map(…).collect()` and
/// the `Display` impl every crate writes.
const CLONE_WINDOW: usize = 30;

/// How far a matching run must spread before it is logic rather than a
/// signature.
///
/// Without this the widest finding in the workspace is thirteen files sharing
/// `impl std::fmt::Display for … { fn fmt(&self, f: &mut Formatter<'_>) ->
/// Result { match self {`, which is thirty tokens of Rust and no decision at
/// all. A declaration packs its tokens into two or three lines; a copied piece
/// of reasoning spreads them out. Measuring the spread separates the two
/// without needing to know what either one says.
const CLONE_MIN_LINES: usize = 4;

/// A source's tokens, as `(line, text)`, comments gone and imports skipped.
///
/// Identifiers are NOT normalised. Both were measured: normalising them finds
/// 53 cross-crate groups dominated by struct field lists that merely rhyme
/// (`RecipeCompleted { name, elapsed, cached_nodes }` against
/// `RecipeSkipped { recipe, elapsed, skipped }`), which is the kind of noise
/// that gets a lint disabled. Exact tokens find fewer clones and lie less.
///
/// Import blocks are skipped for the same reason: two crates that both open
/// with `use std::collections::{BTreeMap, BTreeSet};` have agreed on nothing.
pub fn tokens(text: &str) -> Vec<(usize, String)> {
    let scrubbed = scrub(text, Scrub::Comments);
    let bytes = scrubbed.as_bytes();
    let mut line_of = LineIndex::new(&scrubbed);
    let mut out = Vec::new();
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i].is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if let Some(after) = use_keyword_at(&scrubbed, i) {
            i = statement_end(&scrubbed, after) + 1;
            continue;
        }
        let line = line_of.line(i);
        if let Some(end) = char_literal_end(bytes, i) {
            out.push((line, scrubbed[i..end].to_string()));
            i = end;
            continue;
        }
        if let Some((_, end)) = string_span(bytes, i) {
            out.push((line, scrubbed[i..end].to_string()));
            i = end;
            continue;
        }
        if is_ident_char(bytes[i]) {
            let end = ident_end(bytes, i);
            out.push((line, scrubbed[i..end].to_string()));
            i = end;
            continue;
        }
        out.push((line, scrubbed[i..i + 1].to_string()));
        i += 1;
    }
    out
}

/// Runs of [`CLONE_WINDOW`] identical tokens shared by two or more crates.
///
/// Keyed on the set of files involved rather than on the matching tokens, so
/// editing inside a known clone does not churn the baseline into a rewrite
/// nobody reviews. The cost of that choice, stated rather than hidden: a
/// *second*, unrelated clone between two files already listed here is absorbed
/// by the existing entry. Those two files are already flagged as sharing code,
/// so the pair is known-dirty either way, but the rule does not claim to count
/// them.
pub fn cross_crate_clones(corpus: &[Source]) -> Vec<Finding> {
    let mut windows: BTreeMap<u64, BTreeMap<String, (String, usize)>> = BTreeMap::new();
    for source in corpus {
        let tokens = tokens(&source.text);
        for window in tokens.windows(CLONE_WINDOW) {
            if window[CLONE_WINDOW - 1].0.saturating_sub(window[0].0) < CLONE_MIN_LINES {
                continue;
            }
            let text: Vec<&str> = window.iter().map(|(_, token)| token.as_str()).collect();
            windows
                .entry(cook_contracts::hash::hash_str(&text.join("\u{1}")))
                .or_default()
                .entry(source.path.clone())
                .or_insert((source.krate.clone(), window[0].0));
        }
    }

    let mut groups: BTreeMap<Vec<String>, BTreeMap<String, usize>> = BTreeMap::new();
    for sites in windows.into_values() {
        let crates: BTreeSet<&str> = sites.values().map(|(krate, _)| krate.as_str()).collect();
        if crates.len() < 2 {
            continue;
        }
        let key: Vec<String> = sites.keys().cloned().collect();
        let group = groups.entry(key).or_default();
        for (path, (_, line)) in sites {
            group
                .entry(path)
                .and_modify(|at| *at = (*at).min(line))
                .or_insert(line);
        }
    }

    groups
        .into_iter()
        .map(|(paths, lines)| Finding {
            key: paths.join(" == "),
            sites: lines
                .iter()
                .map(|(path, line)| format!("{path}:{line}"))
                .collect(),
            detail: format!(
                "{} tokens or more of identical code in {} files across crates; the copy that \
                 is not edited is the one that goes wrong",
                CLONE_WINDOW,
                paths.len()
            ),
        })
        .collect()
}

#[test]
fn no_run_of_code_is_copied_across_crates() {
    enforce("cross-crate-clones", &cross_crate_clones(&corpus()));
}

// ------------------------------------------------------- tracked-tree guard

/// Paths `git` reports as tracked, relative to the workspace root, or `None`
/// when this tree is not a git checkout.
fn tracked_paths() -> Option<BTreeSet<String>> {
    let root = workspace_root();
    let inside = Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .current_dir(&root)
        .output()
        .ok()?;
    if !inside.status.success() {
        return None;
    }
    let listed = Command::new("git")
        .args(["ls-files", "-z", "--", "."])
        .current_dir(&root)
        .output()
        .expect("git ls-files in a git work tree");
    assert!(
        listed.status.success(),
        "git ls-files failed inside a git work tree: {}",
        String::from_utf8_lossy(&listed.stderr)
    );
    Some(
        String::from_utf8_lossy(&listed.stdout)
            .split('\0')
            .filter(|path| !path.is_empty())
            .map(str::to_string)
            .collect(),
    )
}

/// What this gate lints must be what CI builds.
///
/// A local suite reads the working tree and CI reads the tracked tree. A file
/// that is present-but-ignored passes here and does not exist there, so a
/// source the gate scanned, or a waiver file it read, can be invisible to the
/// machine that has to reproduce the verdict. That gap is not theoretical: it
/// is how a release shipped with an ignored-but-required file, and the fix is
/// a gitignore negation, never `git add -f`.
#[test]
fn everything_the_gate_reads_is_tracked() {
    let Some(tracked) = tracked_paths() else {
        println!(
            "constitution: not a git checkout, so the tracked-tree guard cannot run; \
             the rules themselves still did"
        );
        return;
    };

    let root = workspace_root();
    let mut untracked: Vec<String> = corpus()
        .into_iter()
        .map(|source| source.path)
        .filter(|path| !tracked.contains(path))
        .collect();

    let constitution = root.join("crates/cook-contracts/constitution");
    if constitution.is_dir() {
        for entry in std::fs::read_dir(&constitution).expect("read constitution directory") {
            let path = relative(&entry.expect("read constitution entry").path(), &root);
            if !tracked.contains(&path) {
                untracked.push(path);
            }
        }
    }
    untracked.sort();

    assert!(
        untracked.is_empty(),
        "these files are read by the constitution gate but are not tracked by git, so CI \
         lints a different tree than you do:\n  {}\n\
         Track them (a .gitignore negation, not `git add -f`) or delete them.",
        untracked.join("\n  ")
    );
}

// ------------------------------------------------------------ mutation tests
//
// Every rule above is represented here by a deliberate violation that must be
// caught, and by the near-miss that must not be. A guard nobody mutated can be
// green and blind at once; that is not a hypothetical, it is what happened to
// the guard this file replaces.

fn finding(key: &str) -> Finding {
    Finding {
        key: key.to_string(),
        sites: vec!["crates/a/src/lib.rs:1".to_string()],
        detail: format!("{key} is implemented twice"),
    }
}

fn waivers_from(text: &str) -> Waivers {
    Waivers {
        rule: "example",
        path: waiver_path("example"),
        entries: parse_waivers("example", "<test>", text),
    }
}

#[test]
fn an_unwaived_finding_fails_and_the_report_says_how_to_waive_it() {
    let report = verdict(&waivers_from(""), &[finding("alpha")])
        .expect("a finding with no waiver must fail the gate");
    assert!(report.contains("NEW"), "{report}");
    assert!(report.contains("alpha"), "{report}");
    assert!(report.contains("constitution/example.jsonl"), "{report}");
}

#[test]
fn a_waived_finding_passes() {
    let waivers =
        waivers_from(r#"{"key": "alpha", "why": "the edge is refused; agreement test in x"}"#);
    assert!(verdict(&waivers, &[finding("alpha")]).is_none());
}

#[test]
fn a_waiver_whose_violation_is_gone_fails_so_the_list_shrinks() {
    let waivers = waivers_from(r#"{"key": "alpha", "why": "the edge is refused"}"#);
    let report = verdict(&waivers, &[]).expect("a stale waiver must fail the gate");
    assert!(report.contains("GONE"), "{report}");
}

#[test]
#[should_panic(expected = "has no \"why\"")]
fn a_waiver_without_a_justification_is_refused() {
    parse_waivers("example", "<test>", r#"{"key": "alpha"}"#);
}

#[test]
#[should_panic(expected = "has no \"why\"")]
fn a_blank_justification_does_not_count_as_one() {
    parse_waivers("example", "<test>", r#"{"key": "alpha", "why": "   "}"#);
}

#[test]
#[should_panic(expected = "duplicate waiver")]
fn the_same_key_cannot_be_waived_twice() {
    parse_waivers(
        "example",
        "<test>",
        "{\"key\": \"a\", \"why\": \"one\"}\n{\"key\": \"a\", \"why\": \"two\"}\n",
    );
}

#[test]
fn comments_and_blank_lines_are_allowed_in_a_waiver_file() {
    let entries = parse_waivers(
        "example",
        "<test>",
        "# the to-do list\n\n{\"key\": \"alpha\", \"why\": \"deliberate\"}\n",
    );
    assert_eq!(entries.len(), 1);
}

#[test]
fn a_use_tree_is_expanded_to_full_paths_however_it_is_spelled() {
    let reached = |text| {
        reached_paths(text)
            .into_iter()
            .map(|(_, path)| path)
            .collect::<BTreeSet<_>>()
    };

    assert!(reached("use std::time::Instant;").contains("std::time::Instant"));
    // The spelling that walked past the previous guard's sibling check.
    let grouped = reached("use std::time::{Duration, Instant};");
    assert!(grouped.contains("std::time::Instant"), "{grouped:?}");
    assert!(grouped.contains("std::time::Duration"), "{grouped:?}");
    // Nested groups, renames, and globs.
    let nested = reached("use std::{fmt::Write as _, collections::{BTreeMap, BTreeSet}};");
    assert!(nested.contains("std::collections::BTreeSet"), "{nested:?}");
    assert!(nested.contains("std::fmt::Write"), "{nested:?}");
    // Inline, with no import at all.
    assert!(reached("let t = std::time::Instant::now();").contains("std::time::Instant::now"));
}

#[test]
fn comments_and_strings_do_not_produce_reaches() {
    assert!(effect_violations("// use std::fs::read_to_string;\n").is_empty());
    assert!(effect_violations("/* std::process::Command */\n").is_empty());
    assert!(effect_violations("let s = \"std::fs is not reached by naming it\";\n").is_empty());
    assert!(effect_violations("let s = r#\"std::env::var\"#;\n").is_empty());
    // A `//` inside a string is not a comment, and must not swallow the line.
    assert!(!effect_violations("let s = \"http://x\"; let f = std::fs::metadata;\n").is_empty());
}

#[test]
fn every_banned_effect_is_caught_in_every_spelling() {
    for banned in BANNED_REACHES {
        for source in [
            format!("use {banned};\n"),
            format!("use {banned}::something;\n"),
            format!("use {}::{{{}}};\n", parent(banned), leaf(banned)),
            format!("let x = {banned}::something();\n"),
        ] {
            assert!(
                !effect_violations(&source).is_empty(),
                "the effect budget does not catch {banned} spelled as: {source}"
            );
        }
    }
    for banned in BANNED_METHODS {
        assert!(
            !effect_violations(&format!("let x = value{banned};\n")).is_empty(),
            "the effect budget does not catch {banned}"
        );
    }
    for banned in BANNED_GLOBALS {
        assert!(
            !effect_violations(&format!("{banned} thing\n")).is_empty(),
            "the effect budget does not catch {banned}"
        );
    }
}

fn parent(path: &str) -> &str {
    path.rsplit_once("::").map_or(path, |(parent, _)| parent)
}

fn leaf(path: &str) -> &str {
    path.rsplit_once("::").map_or(path, |(_, leaf)| leaf)
}

#[test]
fn a_duration_is_a_value_and_stays_legal() {
    assert!(effect_violations("use std::time::Duration;\n").is_empty());
    assert!(effect_violations("use std::time::{Duration};\n").is_empty());
}

#[test]
fn a_dependency_section_is_read_without_its_neighbours() {
    let manifest = "[package]\nname = \"x\"\n\n[dependencies]\n# a comment\nserde = { version = \"1\" }\nglob = \"0.3\"\n\n[dev-dependencies]\ntempfile = \"3\"\n";
    assert_eq!(declared_dependencies(manifest), ["serde", "glob"]);
}

/// The rule the two stratum tests share, so the mutation test exercises the
/// same comparison the tree is judged by rather than a restatement of it.
fn descends_a_stratum(from: &str, to: &str) -> bool {
    matches!((stratum_of(from), stratum_of(to)), (Some(low), Some(high)) if high < low)
}

#[test]
fn an_edge_that_does_not_descend_a_stratum_is_caught() {
    // Only internal edges count, and they are found however the dependency is
    // spelled — a path dependency, a version, or a table.
    let manifests = BTreeMap::from([
        (
            "cook-cli".to_string(),
            "[dependencies]\ncook-plan = { path = \"../cook-plan\" }\nclap = \"4\"\n".to_string(),
        ),
        ("cook-plan".to_string(), "[dependencies]\n".to_string()),
    ]);
    assert_eq!(
        internal_edges(&manifests),
        [("cook-cli".to_string(), "cook-plan".to_string())],
        "an external dependency must not be mistaken for a workspace edge"
    );

    assert!(
        descends_a_stratum("cook-cli", "cook-plan"),
        "the surface may reach down"
    );
    assert!(
        !descends_a_stratum("cook-plan", "cook-cli"),
        "the inversion must be refused"
    );
    assert!(
        !descends_a_stratum("cook-engine", "cook-plan"),
        "a sideways edge between peers must be refused: two crates in one stratum that reach \
         for each other are one crate arguing with itself"
    );
}

#[test]
fn a_re_export_tunnel_is_caught_and_a_direct_reach_is_not() {
    let members: BTreeSet<String> = ["cook-cli", "cook-engine", "cook-cache"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let tunnel = Source {
        krate: "cook-cli".to_string(),
        path: "crates/cook-cli/src/x.rs".to_string(),
        text: "use cook_engine::cook_cache::parse_size;\n".to_string(),
    };
    let findings = re_export_tunnels(&tunnel, &members);
    assert_eq!(
        findings.len(),
        1,
        "{:?}",
        findings.iter().map(|f| &f.key).collect::<Vec<_>>()
    );
    assert_eq!(findings[0].key, "cook-cli -> cook_engine::cook_cache");

    let direct = Source {
        text: "use cook_engine::run::RunResult;\n".to_string(),
        ..tunnel
    };
    assert!(re_export_tunnels(&direct, &members).is_empty());
}

fn source(krate: &str, text: &str) -> Source {
    Source {
        krate: krate.to_string(),
        path: format!("crates/{krate}/src/lib.rs"),
        text: text.to_string(),
    }
}

#[test]
fn a_literal_in_two_crates_is_caught_and_one_crate_is_not() {
    let shared = |text: &str| {
        duplicate_literals(&[source("cook-a", text), source("cook-b", text)])
            .into_iter()
            .map(|finding| finding.key)
            .collect::<Vec<_>>()
    };

    assert_eq!(
        shared("let x = \"registration_v2\";\n"),
        ["registration_v2"]
    );
    assert!(
        duplicate_literals(&[
            source("cook-a", "let x = \"registration_v2\";\n"),
            source("cook-b", "let y = \"something_else\";\n"),
        ])
        .is_empty(),
        "one literal per crate is not a shared decision"
    );
    assert!(
        duplicate_literals(&[
            source("cook-a", "let x = \"a\";\n"),
            source("cook-b", "let y = \"a\";\n")
        ])
        .is_empty(),
        "below the length floor a match is a coincidence"
    );
    // Twice in ONE crate is that crate's business.
    assert!(duplicate_literals(&[source(
        "cook-a",
        "let x = \"registration_v2\"; let y = \"registration_v2\";\n"
    )])
    .is_empty());
    // Comments and doc comments are not code.
    assert!(shared("// let x = \"registration_v2\";\n").is_empty());
    // The exclusions, which must not need a waiver.
    assert!(shared("#[path = \"tests/naming_tests.rs\"]\nmod naming;\n").is_empty());
    assert!(shared("#[serde(skip_serializing_if = \"Option::is_none\")]\n").is_empty());
    assert!(shared("#[serde(rename_all = \"kebab-case\")]\n").is_empty());
    // The exclusion is the attribute, not the word: a case convention named
    // in ordinary code is still a literal two crates share.
    assert_eq!(shared("let style = \"kebab-case\";\n"), ["kebab-case"]);
    // ...and it is the rename_all VALUE, not the line. A wire name sharing a
    // line with the convention must still be caught, or the filter that keeps
    // the list readable starts deleting entries from it.
    assert_eq!(
        shared("#[serde(rename_all = \"kebab-case\", rename = \"some-wire-name\")]\n"),
        ["some-wire-name"]
    );
}

#[test]
fn a_literal_is_read_as_written_including_its_escapes_and_raw_form() {
    let literals = |text: &str| {
        string_literals(text)
            .into_iter()
            .map(|(_, literal)| literal)
            .collect::<Vec<_>>()
    };
    assert_eq!(literals("let x = \"a\\\"b\";\n"), ["a\\\"b"]);
    assert_eq!(
        literals("let x = r#\"raw \"quoted\" text\"#;\n"),
        ["raw \"quoted\" text"]
    );
    // A quote character must not open a literal and swallow the rest of the file.
    assert_eq!(
        literals("let q = '\"'; let after = \"visible\";\n"),
        ["visible"]
    );
}

/// A run of code long enough and spread out enough to count as a clone.
fn copied_body(name: &str) -> String {
    format!(
        "fn {name}(items: &[Item]) -> Vec<String> {{\n\
        \x20   let mut out = Vec::new();\n\
        \x20   for item in items {{\n\
        \x20       if item.enabled {{\n\
        \x20           out.push(item.name.clone());\n\
        \x20       }}\n\
        \x20   }}\n\
        \x20   out.sort();\n\
        \x20   out\n\
        }}\n"
    )
}

#[test]
fn a_run_of_code_copied_across_crates_is_caught_and_one_crate_is_not() {
    let body = copied_body("collect");
    let findings = cross_crate_clones(&[source("cook-a", &body), source("cook-b", &body)]);
    assert_eq!(
        findings.len(),
        1,
        "a copied body across crates must be one finding, got {:?}",
        findings.iter().map(|f| &f.key).collect::<Vec<_>>()
    );
    assert_eq!(
        findings[0].key,
        "crates/cook-a/src/lib.rs == crates/cook-b/src/lib.rs"
    );

    assert!(
        cross_crate_clones(&[source("cook-a", &format!("{body}{}", copied_body("again")))])
            .is_empty(),
        "a crate repeating itself is that crate's business"
    );
    assert!(
        cross_crate_clones(&[
            source("cook-a", &body),
            source(
                "cook-b",
                &copied_body("collect").replace("item.enabled", "item.ready")
            ),
        ])
        .is_empty(),
        "an edited copy is no longer an exact run; this rule catches copies before they \
         drift, which is the limit the baseline file states"
    );
}

#[test]
fn a_signature_is_not_a_clone_however_many_tokens_it_has() {
    // Thirty tokens of `impl Display`, packed into two lines, in two crates.
    // Before the line-spread floor this was the workspace's widest finding.
    let preamble = "impl std::fmt::Display for Thing { fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {\n    match self { Thing::One => write!(f, \"one\"), Thing::Two => write!(f, \"two\") } } }\n";
    assert!(
        cross_crate_clones(&[source("cook-a", preamble), source("cook-b", preamble)]).is_empty(),
        "a declaration packs its tokens into a couple of lines; only spread-out runs count"
    );
}

#[test]
fn imports_and_comments_are_not_tokens() {
    let of = |text: &str| tokens(text).into_iter().map(|(_, t)| t).collect::<Vec<_>>();
    assert!(
        of("use std::collections::{BTreeMap, BTreeSet};\n").is_empty(),
        "two crates opening with the same imports have agreed on nothing"
    );
    assert!(of("// let x = 1;\n").is_empty());
    assert_eq!(of("let x = 1;\n"), ["let", "x", "=", "1", ";"]);
}

#[test]
fn the_corpus_excludes_test_bodies_and_covers_every_crate() {
    let corpus = corpus();
    assert!(
        !corpus.iter().any(|source| source.path.contains("/tests/")),
        "the corpus must exclude test bodies; rules would fire on deliberate test duplication"
    );
    let crates: BTreeSet<&str> = corpus.iter().map(|source| source.krate.as_str()).collect();
    for expected in ["cook-contracts", "cook-engine", "cook-cli", "cook-lang"] {
        assert!(crates.contains(expected), "corpus is missing {expected}");
    }
}
