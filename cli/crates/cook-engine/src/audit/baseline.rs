//! The audit baseline: findings a project has accepted, so `cook audit` fails
//! only on new ones and reports entries that no longer match anything.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use cook_contracts::consumes::ConsumesFilter;

use super::{AuditReport, FindingKind};

/// The tracked file, at the project root.
pub const BASELINE_FILE: &str = "cook-audit-baseline.json";

/// Accepted entries: recipe -> kind (open set) -> literals and `*` patterns.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Baseline {
    pub recipes: BTreeMap<String, BTreeMap<String, BTreeSet<String>>>,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct FileForm {
    version: u32,
    recipes: BTreeMap<String, BTreeMap<String, BTreeSet<String>>>,
}

impl Baseline {
    /// Refuses bad JSON, a version other than 1, unknown top-level fields and
    /// patterns that do not compile.
    pub fn parse(text: &str) -> Result<Baseline, String> {
        let f: FileForm = serde_json::from_str(text).map_err(|e| format!("{BASELINE_FILE}: {e}"))?;
        if f.version != 1 {
            return Err(format!("{BASELINE_FILE}: version {} is not supported; this cook understands version 1", f.version));
        }
        for (recipe, kinds) in &f.recipes {
            for (kind, entries) in kinds {
                for e in entries.iter().filter(|e| e.contains('*')) {
                    ConsumesFilter::compile(std::slice::from_ref(e))
                        .map_err(|(_, why)| format!("{BASELINE_FILE}: recipe {recipe}, kind {kind}: pattern `{e}` does not compile: {why}"))?;
                }
            }
        }
        Ok(Baseline { recipes: f.recipes })
    }

    /// The `*` entries per (recipe, kind): what only a human writes, so a
    /// handed-back baseline must carry exactly the old file's.
    pub fn patterns(&self) -> BTreeMap<(String, String), BTreeSet<String>> {
        let mut out = BTreeMap::new();
        for (recipe, kinds) in &self.recipes {
            for (kind, entries) in kinds {
                let pats: BTreeSet<String> = entries.iter().filter(|e| e.contains('*')).cloned().collect();
                if !pats.is_empty() {
                    out.insert((recipe.clone(), kind.clone()), pats);
                }
            }
        }
        out
    }

    /// What a run handed back, checked against the file it started from: valid
    /// UTF-8 and JSON, and with exactly the old `*` entries. The caller writes
    /// the returned baseline's own rendering, never `bytes`.
    pub fn from_handback(old: &Baseline, bytes: &[u8]) -> Result<Baseline, String> {
        let text = std::str::from_utf8(bytes).map_err(|e| format!("the handed-back baseline is not UTF-8: {e}"))?;
        let new = Self::parse(text).map_err(|e| format!("the handed-back baseline is invalid: {e}"))?;
        if new.patterns() != old.patterns() {
            return Err(format!("the handed-back baseline changes the `*` entries of {BASELINE_FILE}; only a person edits those"));
        }
        Ok(new)
    }

    /// A missing file is an empty baseline.
    pub fn load(path: &Path) -> Result<Baseline, String> {
        match std::fs::read_to_string(path) {
            Ok(t) => Self::parse(&t),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Baseline::default()),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }

    /// Pretty JSON, sorted, empty kinds and recipes omitted, trailing newline.
    pub fn render(&self) -> String {
        let recipes = self
            .recipes
            .iter()
            .filter_map(|(r, ks)| {
                let ks: BTreeMap<_, _> = ks.iter().filter(|(_, v)| !v.is_empty()).map(|(k, v)| (k.clone(), v.clone())).collect();
                (!ks.is_empty()).then(|| (r.clone(), ks))
            })
            .collect();
        let mut out = serde_json::to_string_pretty(&FileForm { version: 1, recipes }).expect("baseline serializes");
        out.push('\n');
        out
    }
}

/// A baseline entry that matched nothing this run.
#[derive(serde::Serialize, Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct StaleEntry {
    pub recipe: String,
    pub kind: String,
    pub entry: String,
}

/// What [`apply`] did: items it accepted, entries that matched nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Ratchet {
    pub accepted: usize,
    pub stale: Vec<StaleEntry>,
}

/// One recipe+kind's entries: literals by equality, `*` entries as patterns.
struct Rules<'a> {
    literals: Vec<&'a str>,
    patterns: Vec<(&'a str, ConsumesFilter)>,
}

impl<'a> Rules<'a> {
    fn of(entries: Option<&'a BTreeSet<String>>) -> Self {
        let mut r = Rules { literals: vec![], patterns: vec![] };
        for e in entries.into_iter().flatten() {
            if e.contains('*') {
                // Compiled at parse; a hand-built baseline with a bad pattern matches nothing.
                if let Ok(f) = ConsumesFilter::compile(std::slice::from_ref(e)) {
                    r.patterns.push((e, f));
                }
            } else {
                r.literals.push(e);
            }
        }
        r
    }
    fn accepts(&self, item: &str) -> bool {
        self.literals.contains(&item) || self.patterns.iter().any(|(_, f)| f.matches(item))
    }
}

type Observed = BTreeMap<(String, &'static str), BTreeSet<String>>;

/// Every item the run saw: read/absent paths, env and env-unobserved roots.
fn observed(report: &AuditReport) -> Observed {
    let mut o = Observed::new();
    for f in &report.findings {
        let set = o.entry((f.recipe.clone(), f.kind.as_str())).or_default();
        match f.kind {
            FindingKind::Read | FindingKind::Absent => set.extend(f.paths.iter().cloned()),
            FindingKind::Env | FindingKind::EnvUnobserved => {
                set.insert(f.root.clone());
            }
        }
    }
    o
}

/// Recipes in the closure none of whose units errored: only those can be judged.
fn judged(report: &AuditReport) -> impl Iterator<Item = &String> {
    report.recipes.iter().filter(|r| !report.units.iter().any(|u| &u.recipe == *r && u.errored()))
}

/// Registered in the Cookfile; an empty `known` (a hand-built report) knows everything.
fn is_known(report: &AuditReport, recipe: &str) -> bool {
    report.known.is_empty() || report.known.contains(recipe)
}

/// Drop accepted items from `report` and list stale entries.
pub fn apply(report: &mut AuditReport, baseline: &Baseline) -> Ratchet {
    let seen = observed(report);
    let mut accepted = 0;
    let findings = std::mem::take(&mut report.findings);
    for mut f in findings {
        let rules = Rules::of(baseline.recipes.get(&f.recipe).and_then(|ks| ks.get(f.kind.as_str())));
        match f.kind {
            FindingKind::Read | FindingKind::Absent => {
                let before = f.paths.len();
                f.paths.retain(|p| !rules.accepts(p));
                accepted += before - f.paths.len();
                f.count = f.paths.len();
                if f.paths.is_empty() {
                    continue;
                }
            }
            FindingKind::Env | FindingKind::EnvUnobserved => {
                if rules.accepts(&f.root) {
                    accepted += 1;
                    continue;
                }
            }
        }
        report.findings.push(f);
    }
    let mut stale = vec![];
    // A recipe the Cookfile no longer registers matches nothing, whatever the run saw.
    for (recipe, kinds) in baseline.recipes.iter().filter(|(r, _)| !is_known(report, r)) {
        for (kind, entries) in kinds {
            stale.extend(entries.iter().map(|e| StaleEntry { recipe: recipe.clone(), kind: kind.clone(), entry: e.clone() }));
        }
    }
    for recipe in judged(report) {
        for kind in FindingKind::ALL.map(FindingKind::as_str) {
            let Some(entries) = baseline.recipes.get(recipe).and_then(|ks| ks.get(kind)) else { continue };
            let items = seen.get(&(recipe.clone(), kind));
            let rules = Rules::of(Some(entries));
            let hit = |i: &String| items.is_some_and(|s| s.contains(i));
            let dead_literals = rules.literals.iter().filter(|l| !hit(&l.to_string()));
            let dead_patterns = rules.patterns.iter().filter(|(_, f)| !items.is_some_and(|s| s.iter().any(|i| f.matches(i)))).map(|(p, _)| p);
            for entry in dead_literals.chain(dead_patterns) {
                stale.push(StaleEntry { recipe: recipe.clone(), kind: kind.into(), entry: entry.to_string() });
            }
        }
    }
    stale.sort();
    Ratchet { accepted, stale }
}

/// The baseline after accepting this run (pass the unfiltered report): judged
/// recipes keep their literals and gain what was observed (an observation
/// containing `*` is left out, so it stays a finding), because tools read
/// different files on identical runs. With `prune`, their literals are
/// replaced by the observation instead, and the literals of recipes the
/// Cookfile no longer registers are dropped. Patterns, unknown kinds, errored
/// and out-of-closure recipes are carried forward unchanged.
pub fn regenerate(old: &Baseline, report: &AuditReport, prune: bool) -> Baseline {
    let seen = observed(report);
    let mut new = old.clone();
    for recipe in judged(report) {
        for kind in FindingKind::ALL.map(FindingKind::as_str) {
            let old_entries = old.recipes.get(recipe).and_then(|ks| ks.get(kind));
            let rules = Rules::of(old_entries);
            let mut set: BTreeSet<String> = rules.patterns.iter().map(|(p, _)| p.to_string()).collect();
            if !prune {
                set.extend(rules.literals.iter().map(|l| l.to_string()));
            }
            if let Some(items) = seen.get(&(recipe.clone(), kind)) {
                // A `*` in an observed spelling would be read back as a pattern: never written.
                set.extend(items.iter().filter(|i| !i.contains('*') && !rules.patterns.iter().any(|(_, f)| f.matches(i))).cloned());
            }
            let kinds = new.recipes.entry(recipe.clone()).or_default();
            if set.is_empty() {
                kinds.remove(kind);
            } else {
                kinds.insert(kind.into(), set);
            }
        }
    }
    if prune {
        for (_, kinds) in new.recipes.iter_mut().filter(|(r, _)| !is_known(report, r)) {
            for entries in kinds.values_mut() {
                entries.retain(|e| e.contains('*'));
            }
            kinds.retain(|_, v| !v.is_empty());
        }
    }
    new.recipes.retain(|_, ks| !ks.is_empty());
    new
}

/// Replace `path` with `bytes` in one rename (a crash leaves the old file),
/// keeping its mode (0644 for a new file). `Ok(false)`: it already held them.
pub fn persist(path: &Path, bytes: &[u8]) -> Result<bool, String> {
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    let fail = |e: &dyn std::fmt::Display| format!("{}: {e}", path.display());
    let old = std::fs::metadata(path).ok();
    if old.is_some() && std::fs::read(path).is_ok_and(|cur| cur == bytes) {
        return Ok(false);
    }
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let mut tmp = tempfile::NamedTempFile::new_in(dir).map_err(|e| fail(&e))?;
    tmp.write_all(bytes).map_err(|e| fail(&e))?;
    let mode = old.map_or(0o644, |m| m.permissions().mode() & 0o7777);
    tmp.as_file().set_permissions(std::fs::Permissions::from_mode(mode)).map_err(|e| fail(&e))?;
    tmp.persist(path).map_err(|e| fail(&e.error))?;
    Ok(true)
}

#[cfg(test)]
#[path = "tests/baseline_tests.rs"]
mod tests;
