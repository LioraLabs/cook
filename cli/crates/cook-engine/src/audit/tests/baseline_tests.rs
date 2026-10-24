use super::*;
use crate::audit::{Finding, UnitAudit};

fn finding(recipe: &str, kind: FindingKind, root: &str, paths: &[&str]) -> Finding {
    Finding { recipe: recipe.into(), kind, root: root.into(), count: paths.len().max(1), paths: paths.iter().map(|s| s.to_string()).collect() }
}

fn unit(recipe: &str, status: &'static str) -> UnitAudit {
    UnitAudit { recipe: recipe.into(), unit: "u".into(), key: "k".into(), status, detail: String::new() }
}

fn report(recipes: &[&str], units: Vec<UnitAudit>, findings: Vec<Finding>) -> AuditReport {
    AuditReport { units, findings, recipes: recipes.iter().map(|s| s.to_string()).collect(), known: BTreeSet::new() }
}

fn base(json: &str) -> Baseline {
    Baseline::parse(json).unwrap()
}

#[test]
fn literal_accepts_and_recomputes_count() {
    let b = base(r#"{"version":1,"recipes":{"a":{"read":["x/1","x/2"]}}}"#);
    let mut r = report(&["a"], vec![], vec![finding("a", FindingKind::Read, "x", &["x/1", "x/3"])]);
    let rat = apply(&mut r, &b);
    assert_eq!(rat.accepted, 1);
    assert_eq!(r.findings.len(), 1);
    assert_eq!((r.findings[0].count, r.findings[0].paths.clone()), (1, vec!["x/3".to_string()]));
    assert_eq!(rat.stale, vec![StaleEntry { recipe: "a".into(), kind: "read".into(), entry: "x/2".into() }]);
}

#[test]
fn finding_with_every_path_accepted_is_dropped() {
    let b = base(r#"{"version":1,"recipes":{"a":{"absent":["p"]}}}"#);
    let mut r = report(&["a"], vec![], vec![finding("a", FindingKind::Absent, "x", &["p"])]);
    assert_eq!(apply(&mut r, &b).accepted, 1);
    assert!(r.findings.is_empty());
}

#[test]
fn patterns_use_the_consumes_dialect() {
    let b = base(r#"{"version":1,"recipes":{"a":{"read":["_tmp_*","node_modules/**"],"env":["CLAUDE_*"]}}}"#);
    let mut r = report(
        &["a"],
        vec![],
        vec![
            finding("a", FindingKind::Read, "x", &["deep/dir/_tmp_1", "node_modules/a/b/c.js", "src/keep.rs"]),
            finding("a", FindingKind::Env, "CLAUDE_CODE", &[]),
            finding("a", FindingKind::Env, "HOME", &[]),
        ],
    );
    let rat = apply(&mut r, &b);
    assert_eq!(rat.accepted, 3);
    assert_eq!(r.findings.len(), 2);
    assert_eq!(r.findings[0].paths, vec!["src/keep.rs".to_string()]);
    assert_eq!(r.findings[1].root, "HOME");
    assert!(rat.stale.is_empty());
}

#[test]
fn bracket_literal_matches_only_itself() {
    let b = base(r#"{"version":1,"recipes":{"a":{"read":["src/[id].tsx"]}}}"#);
    let mut r = report(&["a"], vec![], vec![finding("a", FindingKind::Read, "x", &["src/[id].tsx", "src/i.tsx"])]);
    assert_eq!(apply(&mut r, &b).accepted, 1);
    assert_eq!(r.findings[0].paths, vec!["src/i.tsx".to_string()]);
}

#[test]
fn env_unobserved_root_is_the_item() {
    let b = base(r#"{"version":1,"recipes":{"a":{"env-unobserved":["/bin/sh"]}}}"#);
    let mut r = report(&["a"], vec![], vec![finding("a", FindingKind::EnvUnobserved, "/bin/sh", &[])]);
    assert_eq!(apply(&mut r, &b).accepted, 1);
    assert!(r.findings.is_empty());
}

#[test]
fn entries_do_not_cross_recipe_or_kind() {
    let b = base(r#"{"version":1,"recipes":{"a":{"read":["p"]}}}"#);
    let mut r = report(&["a", "b"], vec![], vec![finding("b", FindingKind::Read, "x", &["p"]), finding("a", FindingKind::Absent, "x", &["p"])]);
    assert_eq!(apply(&mut r, &b).accepted, 0);
    assert_eq!(r.findings.len(), 2);
}

#[test]
fn stale_pattern_that_matched_nothing() {
    let b = base(r#"{"version":1,"recipes":{"a":{"read":["_tmp_*","gone/**"]}}}"#);
    let mut r = report(&["a"], vec![unit("a", "traced")], vec![finding("a", FindingKind::Read, "x", &["_tmp_1"])]);
    let rat = apply(&mut r, &b);
    assert_eq!(rat.stale, vec![StaleEntry { recipe: "a".into(), kind: "read".into(), entry: "gone/**".into() }]);
}

#[test]
fn errored_recipe_is_not_stale_but_still_filters() {
    let b = base(r#"{"version":1,"recipes":{"a":{"read":["old","p"]}}}"#);
    let mut r = report(&["a"], vec![unit("a", "error")], vec![finding("a", FindingKind::Read, "x", &["p"])]);
    let rat = apply(&mut r, &b);
    assert_eq!(rat.accepted, 1);
    assert!(rat.stale.is_empty());
}

#[test]
fn recipe_outside_closure_is_never_stale() {
    let b = base(r#"{"version":1,"recipes":{"zzz":{"read":["old"]}}}"#);
    let mut r = report(&["a"], vec![], vec![]);
    assert!(apply(&mut r, &b).stale.is_empty());
}

#[test]
fn unknown_kind_is_kept_and_never_stale() {
    let b = base(r#"{"version":1,"recipes":{"a":{"future":["x"],"read":["p"]}}}"#);
    let mut r = report(&["a"], vec![], vec![finding("a", FindingKind::Read, "x", &["p"])]);
    assert!(apply(&mut r, &b).stale.is_empty());
    let new = regenerate(&b, &r, false);
    assert_eq!(new.recipes["a"]["future"], BTreeSet::from(["x".to_string()]));
}

#[test]
fn parse_refusals() {
    assert!(Baseline::parse("{").is_err());
    let e = Baseline::parse(r#"{"version":2,"recipes":{}}"#).unwrap_err();
    assert!(e.contains('2') && e.contains("understands version 1"), "{e}");
    let e = Baseline::parse(r#"{"version":1,"recipes":{},"extra":1}"#).unwrap_err();
    assert!(e.contains("extra"), "{e}");
    let e = Baseline::parse(r#"{"version":1,"recipes":{"rec":{"read":["[*"]}}}"#).unwrap_err();
    assert!(e.contains("rec") && e.contains("read") && e.contains("[*"), "{e}");
}

#[test]
fn render_is_deterministic_and_round_trips() {
    let b = base(r#"{"version":1,"recipes":{"b":{"read":["z","a"],"env":[]},"e":{},"a":{"absent":["q"]}}}"#);
    let text = b.render();
    assert_eq!(text, b.render());
    assert!(text.ends_with("}\n") && text.starts_with("{\n  \"version\": 1,"), "{text}");
    assert!(!text.contains("\"e\"") && !text.contains("env"), "{text}");
    assert!(text.find("\"a\"").unwrap() < text.find("\"b\"").unwrap());
    assert_eq!(Baseline::parse(&text).unwrap().render(), text);
    assert_eq!(Baseline::parse(&text).unwrap().recipes.len(), 2);
}

#[test]
fn load_missing_is_empty_and_other_errors_name_the_path() {
    let d = tempfile::tempdir().unwrap();
    assert_eq!(Baseline::load(&d.path().join(BASELINE_FILE)).unwrap(), Baseline::default());
    let e = Baseline::load(d.path()).unwrap_err(); // a directory: read fails, not NotFound
    assert!(e.contains(&d.path().display().to_string()), "{e}");
}

#[test]
fn regenerate_keeps_patterns_and_excludes_matched_literals() {
    let old = base(r#"{"version":1,"recipes":{"a":{"read":["_tmp_*","gone"],"env":["X"]}}}"#);
    let r = report(&["a"], vec![unit("a", "traced")], vec![finding("a", FindingKind::Read, "x", &["_tmp_1", "new"])]);
    let new = regenerate(&old, &r, true);
    assert_eq!(new.recipes["a"]["read"], BTreeSet::from(["_tmp_*".to_string(), "new".to_string()]));
    assert!(!new.recipes["a"].contains_key("env")); // judged, nothing observed
}

#[test]
fn regenerate_carries_errored_and_out_of_closure_recipes_and_drops_empties() {
    let old = base(r#"{"version":1,"recipes":{"err":{"read":["k"]},"out":{"read":["k"]},"a":{"read":["gone"]}}}"#);
    let r = report(&["err", "a"], vec![unit("err", "error")], vec![]);
    let new = regenerate(&old, &r, true);
    assert_eq!(new.recipes["err"], old.recipes["err"]);
    assert_eq!(new.recipes["out"], old.recipes["out"]);
    assert!(!new.recipes.contains_key("a"));
}

#[test]
fn regenerate_never_writes_an_observation_containing_a_star() {
    let r = report(
        &["a"],
        vec![unit("a", "traced")],
        vec![finding("a", FindingKind::Read, "x", &["x/ok", "a*b", "[*"])],
    );
    let new = regenerate(&Baseline::default(), &r, false);
    assert_eq!(new.recipes["a"]["read"], ["x/ok".to_string()].into());
    assert_eq!(Baseline::parse(&new.render()).unwrap(), new);
}

#[test]
fn persist_creates_0644_keeps_mode_and_skips_equal_bytes() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("b.json");
    assert!(persist(&p, b"one").unwrap());
    assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o644);
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert!(persist(&p, b"two").unwrap());
    assert_eq!(std::fs::read(&p).unwrap(), b"two");
    assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
    let mtime = std::fs::metadata(&p).unwrap().modified().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    assert!(!persist(&p, b"two").unwrap());
    assert_eq!(std::fs::metadata(&p).unwrap().modified().unwrap(), mtime);
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1, "no temp left behind");
}

#[test]
fn patterns_are_the_star_entries_per_recipe_and_kind() {
    let a = base(r#"{"version":1,"recipes":{"a":{"read":["x","src/*"],"absent":["y"]},"b":{"read":["z"]}}}"#);
    let want = BTreeMap::from([(("a".to_string(), "read".to_string()), BTreeSet::from(["src/*".to_string()]))]);
    assert_eq!(a.patterns(), want);
    // Literals may come and go; patterns may not.
    let literals_differ = base(r#"{"version":1,"recipes":{"a":{"read":["q","src/*"]}}}"#);
    assert_eq!(literals_differ.patterns(), want);
    let added = base(r#"{"version":1,"recipes":{"a":{"read":["src/*","**"]}}}"#);
    assert_ne!(added.patterns(), want);
    let dropped = base(r#"{"version":1,"recipes":{"a":{"read":["x"]}}}"#);
    assert_ne!(dropped.patterns(), want);
}

#[test]
fn a_handback_with_other_patterns_or_bad_bytes_is_refused() {
    let old = base(r#"{"version":1,"recipes":{"a":{"read":["src/*","x"]}}}"#);
    let ok = Baseline::from_handback(&old, br#"{"version":1,"recipes":{"a":{"read":["src/*","y"]}}}"#).unwrap();
    assert!(ok.recipes["a"]["read"].contains("y"));
    for bad in [
        &br#"{"version":1,"recipes":{"a":{"read":["**"]}}}"#[..],
        br#"{"version":1,"recipes":{"a":{"read":["x"]}}}"#,
        br#"{"version":1,"recipes":{"a":{"read":["src/*"],"absent":["*"]}}}"#,
        b"{",
        b"\xff\xfe",
    ] {
        assert!(Baseline::from_handback(&old, bad).is_err(), "{}", String::from_utf8_lossy(bad));
    }
    let none = Baseline::default();
    assert!(Baseline::from_handback(&none, br#"{"version":1,"recipes":{"lint":{"read":["**"]}}}"#).is_err());
}

fn known(mut r: AuditReport, names: &[&str]) -> AuditReport {
    r.known = names.iter().map(|s| s.to_string()).collect();
    r
}

fn lits(b: &Baseline, recipe: &str, kind: &str) -> Vec<String> {
    b.recipes[recipe][kind].iter().cloned().collect()
}

#[test]
fn regenerate_unions_old_literals_with_the_observation_and_is_a_no_op_once_held() {
    let old = base(r#"{"version":1,"recipes":{"a":{"read":["flap-a","src/*"]}}}"#);
    let r = report(&["a"], vec![unit("a", "traced")], vec![finding("a", FindingKind::Read, "x", &["flap-b", "src/x"])]);
    let new = regenerate(&old, &r, false);
    assert_eq!(lits(&new, "a", "read"), ["flap-a", "flap-b", "src/*"]);
    let again = regenerate(&new, &r, false);
    assert_eq!(again, new);
    // The other alternation, observed against the union, changes nothing either.
    let other = report(&["a"], vec![unit("a", "traced")], vec![finding("a", FindingKind::Read, "x", &["flap-a"])]);
    assert_eq!(regenerate(&new, &other, false), new);
}

#[test]
fn prune_replaces_the_judged_literals() {
    let old = base(r#"{"version":1,"recipes":{"a":{"read":["gone","src/*"]}}}"#);
    let r = report(&["a"], vec![unit("a", "traced")], vec![finding("a", FindingKind::Read, "x", &["new"])]);
    assert_eq!(lits(&regenerate(&old, &r, true), "a", "read"), ["new", "src/*"]);
    assert_eq!(lits(&regenerate(&old, &r, false), "a", "read"), ["gone", "new", "src/*"]);
}

#[test]
fn prune_drops_literals_of_unregistered_recipes_but_keeps_their_patterns() {
    let old = base(r#"{"version":1,"recipes":{"x":{"read":["l","p/*"],"future":["f"]},"y":{"read":["l"]},"z":{"read":["l"]}}}"#);
    let r = known(report(&["a"], vec![], vec![]), &["a", "z"]);
    let new = regenerate(&old, &r, true);
    assert_eq!(lits(&new, "x", "read"), ["p/*"]);
    assert!(!new.recipes["x"].contains_key("future"));
    assert!(!new.recipes.contains_key("y"));
    assert_eq!(new.recipes["z"], old.recipes["z"], "registered but outside the closure: kept");
    assert_eq!(regenerate(&old, &r, false), old, "no prune, no removal");
    for prune in [false, true] {
        let text = regenerate(&old, &r, prune).render();
        assert_eq!(Baseline::from_handback(&old, text.as_bytes()).unwrap().render(), text);
    }
}

#[test]
fn every_entry_of_an_unregistered_recipe_is_stale() {
    let b = base(r#"{"version":1,"recipes":{"x":{"read":["l","p/*"],"future":["f"]},"a":{"read":["k"]}}}"#);
    let mut r = known(report(&["a"], vec![unit("a", "traced")], vec![finding("a", FindingKind::Read, "x", &["k"])]), &["a"]);
    let stale: Vec<_> = apply(&mut r, &b).stale.into_iter().map(|s| (s.recipe, s.kind, s.entry)).collect();
    let s = |k: &str, e: &str| ("x".to_string(), k.to_string(), e.to_string());
    assert_eq!(stale, vec![s("future", "f"), s("read", "l"), s("read", "p/*")]);
}

#[test]
fn an_empty_known_set_marks_nothing_stale_and_nothing_pruned() {
    let b = base(r#"{"version":1,"recipes":{"x":{"read":["l"]}}}"#);
    let mut r = report(&["a"], vec![], vec![]);
    assert!(apply(&mut r, &b).stale.is_empty());
    assert_eq!(regenerate(&b, &r, true), b);
}

const EVERY: &str = r#"{"version":1,"recipes":{"*":{"read":["/etc/hosts","/usr/lib/**"],"env":["CI"]},"a":{"read":["own"]}}}"#;

#[test]
fn the_project_wide_recipe_accepts_literals_and_patterns_for_every_recipe() {
    let mut r = report(
        &["a", "b"],
        vec![],
        vec![
            finding("a", FindingKind::Read, "x", &["/etc/hosts", "own", "/usr/lib/x/y.so", "keep"]),
            finding("b", FindingKind::Read, "x", &["/etc/hosts", "/usr/lib/z", "own"]),
            finding("b", FindingKind::Env, "CI", &[]),
            finding("b", FindingKind::Absent, "/etc/hosts", &["/etc/hosts"]),
        ],
    );
    let rat = apply(&mut r, &base(EVERY));
    assert_eq!(rat.accepted, 6);
    let left: Vec<_> = r.findings.iter().map(|f| (f.recipe.as_str(), f.kind.as_str(), f.paths.clone())).collect();
    assert_eq!(left, [("a", "read", vec!["keep".to_string()]), ("b", "read", vec!["own".to_string()]), ("b", "absent", vec!["/etc/hosts".to_string()])]);
}

#[test]
fn regenerate_writes_nothing_the_project_wide_recipe_accepts_and_nothing_into_it() {
    let old = base(EVERY);
    let r = report(&["a", "b", "*"], vec![unit("a", "traced"), unit("b", "traced")], vec![finding("b", FindingKind::Read, "x", &["/etc/hosts", "/usr/lib/q", "/opt/n", "own"]), finding("*", FindingKind::Read, "x", &["/opt/m"])]);
    let new = regenerate(&old, &r, false);
    assert_eq!(lits(&new, "b", "read"), ["/opt/n", "own"]);
    assert_eq!(new.recipes["*"], old.recipes["*"]);
    assert_eq!(new.recipes["a"], old.recipes["a"], "own literal survives and nothing global is added");
    assert_eq!(regenerate(&new, &r, false), new);
    let pruned = regenerate(&old, &r, true);
    assert_eq!(pruned.recipes["*"], old.recipes["*"]);
    assert_eq!(lits(&pruned, "b", "read"), ["/opt/n", "own"]);
}

#[test]
fn the_project_wide_recipe_survives_prune_and_is_never_stale() {
    let old = base(EVERY);
    // Registered recipes only; "*" is not one, and it matches nothing this run saw.
    let r = known(report(&["a"], vec![unit("a", "traced")], vec![finding("a", FindingKind::Read, "x", &["own"])]), &["a"]);
    assert_eq!(regenerate(&old, &r, true), old);
    let mut r = r;
    assert!(apply(&mut r, &old).stale.is_empty());
    let mut r = known(report(&["a"], vec![], vec![]), &["a"]);
    assert!(apply(&mut r, &old).stale.iter().all(|s| s.recipe != "*"));
}

#[test]
fn a_handback_must_carry_the_project_wide_recipe_literals_included() {
    let old = base(EVERY);
    let same = old.render();
    assert!(Baseline::from_handback(&old, same.as_bytes()).is_ok());
    for bad in [
        r#"{"version":1,"recipes":{"*":{"read":["/usr/lib/**"],"env":["CI"]},"a":{"read":["own"]}}}"#,
        r#"{"version":1,"recipes":{"*":{"read":["/etc/hosts","/usr/lib/**","/etc/shadow"],"env":["CI"]}}}"#,
        r#"{"version":1,"recipes":{"a":{"read":["own"]}}}"#,
    ] {
        assert!(Baseline::from_handback(&old, bad.as_bytes()).is_err(), "{bad}");
    }
    assert!(Baseline::from_handback(&Baseline::default(), EVERY.as_bytes()).is_err(), "a run cannot invent the policy set");
}
