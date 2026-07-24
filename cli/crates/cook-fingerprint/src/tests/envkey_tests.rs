use super::*;

#[test]
fn baseline_excludes_HOME() {
    let d = EnvDenylist::baseline();
    assert!(d.is_ignored("HOME"));
}

#[test]
fn baseline_excludes_PATH() {
    let d = EnvDenylist::baseline();
    assert!(d.is_ignored("PATH"));
}

#[test]
fn baseline_excludes_XDG_glob() {
    let d = EnvDenylist::baseline();
    assert!(d.is_ignored("XDG_RUNTIME_DIR"));
    assert!(d.is_ignored("XDG_CONFIG_HOME"));
}

#[test]
fn baseline_excludes_GITHUB_glob() {
    let d = EnvDenylist::baseline();
    assert!(d.is_ignored("GITHUB_TOKEN"));
    assert!(d.is_ignored("GITHUB_ACTIONS"));
}

#[test]
fn baseline_does_not_exclude_CFLAGS() {
    let d = EnvDenylist::baseline();
    assert!(!d.is_ignored("CFLAGS"));
    assert!(!d.is_ignored("CXXFLAGS"));
    assert!(!d.is_ignored("CPATH"));
}

#[test]
fn baseline_does_not_exclude_LANG_or_LC() {
    let d = EnvDenylist::baseline();
    assert!(!d.is_ignored("LANG"));
    assert!(!d.is_ignored("LC_ALL"));
    assert!(!d.is_ignored("LC_CTYPE"));
    assert!(!d.is_ignored("TZ"));
    assert!(!d.is_ignored("SOURCE_DATE_EPOCH"));
}

#[test]
fn extend_with_adds_user_names() {
    let mut d = EnvDenylist::baseline();
    d.extend_with(&["MY_API_TOKEN".to_string(), "MY_SECRET".to_string()]);
    assert!(d.is_ignored("MY_API_TOKEN"));
    assert!(d.is_ignored("MY_SECRET"));
    assert!(d.is_ignored("HOME"), "baseline still applies");
}

#[test]
fn extend_with_overlap_is_idempotent() {
    let mut d = EnvDenylist::baseline();
    d.extend_with(&["HOME".to_string()]);
    assert!(d.is_ignored("HOME"));
}

#[test]
fn env_contribution_empty_consulted_is_constant() {
    let d = EnvDenylist::baseline();
    let consulted = BTreeMap::new();
    let h1 = env_contribution(&consulted, &d);
    let h2 = env_contribution(&consulted, &d);
    assert_eq!(h1, h2);
}

#[test]
fn env_contribution_filtered_keys_excluded() {
    let d = EnvDenylist::baseline();
    let mut a = BTreeMap::new();
    a.insert("CFLAGS".to_string(), "-O2".to_string());
    let mut b = a.clone();
    b.insert("HOME".to_string(), "/home/alice".to_string());
    let h_a = env_contribution(&a, &d);
    let h_b = env_contribution(&b, &d);
    assert_eq!(h_a, h_b, "denylisted HOME must not contribute");
}

#[test]
fn env_contribution_kept_keys_included() {
    let d = EnvDenylist::baseline();
    let mut a = BTreeMap::new();
    a.insert("CFLAGS".to_string(), "-O2".to_string());
    let mut b = BTreeMap::new();
    b.insert("CFLAGS".to_string(), "-O3".to_string());
    let h_a = env_contribution(&a, &d);
    let h_b = env_contribution(&b, &d);
    assert_ne!(h_a, h_b, "CFLAGS value change must change hash");
}

#[test]
fn env_contribution_value_change_changes_hash() {
    let d = EnvDenylist::baseline();
    let mut a = BTreeMap::new();
    a.insert("MYVAR".to_string(), "v1".to_string());
    let mut b = BTreeMap::new();
    b.insert("MYVAR".to_string(), "v2".to_string());
    assert_ne!(env_contribution(&a, &d), env_contribution(&b, &d));
}

#[test]
fn env_contribution_iteration_order_independent() {
    let d = EnvDenylist::baseline();
    let mut a = BTreeMap::new();
    a.insert("Z".to_string(), "1".to_string());
    a.insert("A".to_string(), "2".to_string());
    let mut b = BTreeMap::new();
    b.insert("A".to_string(), "2".to_string());
    b.insert("Z".to_string(), "1".to_string());
    assert_eq!(env_contribution(&a, &d), env_contribution(&b, &d));
}
