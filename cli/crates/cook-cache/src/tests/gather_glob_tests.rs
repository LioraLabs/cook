use super::*;

#[test]
fn normalize_glob_directory_forms() {
    assert_eq!(normalize_glob_pattern("dir/**").as_ref(), "dir/**/*");
    assert_eq!(normalize_glob_pattern("dir/").as_ref(), "dir/**/*");
}
#[test]
fn resolves_nested_and_workspace_anchored_files() {
    let t = tempfile::tempdir().unwrap();
    let m = t.path().join("member");
        std::fs::create_dir_all(m.join("dir/nested")).unwrap();
    std::fs::write(m.join("dir/nested/file"), "").unwrap();
    std::fs::write(t.path().join("root.txt"), "").unwrap();
    assert_eq!(
        resolve_gather_glob(&m, t.path(), "dir/**").unwrap(),
        BTreeSet::from(["dir/nested/file".into()])
    );
    let rooted = resolve_gather_glob(&m, t.path(), "//root.txt").unwrap();
    assert_eq!(rooted, BTreeSet::from(["../root.txt".into()]));
    assert!(m.join(rooted.first().unwrap()).is_file());
}
#[test]
fn malformed_anchor_errors() {
    let t = tempfile::tempdir().unwrap();
    for pattern in [
        "/x",
        "//",
        "//..",
        "//../x",
        "///x",
        "//dir/../../outside",
        "//./../outside",
    ] {
        let error = resolve_gather_glob(t.path(), t.path(), pattern).unwrap_err();
        assert!(
            error.contains("malformed workspace anchor"),
                "{pattern}: {error}"
        );
    }
}

#[test]
fn anchored_curdir_component_stays_within_workspace() {
    let t = tempfile::tempdir().unwrap();
    std::fs::create_dir(t.path().join("dir")).unwrap();
        std::fs::write(t.path().join("dir/file"), "").unwrap();
    assert_eq!(
        resolve_gather_glob(t.path(), t.path(), "//dir/./file").unwrap(),
        BTreeSet::from(["dir/file".into()])
    );
}

#[test]
fn member_relative_patterns_cannot_escape_member_root() {
    let t = tempfile::tempdir().unwrap();
    for pattern in ["../outside", "dir/../../outside"] {
        let error = resolve_gather_glob(t.path(), t.path(), pattern).unwrap_err();
        assert!(error.contains("escapes member root"), "{pattern}: {error}");
    }
}

#[test]
fn contained_member_parent_component_is_allowed() {
    let t = tempfile::tempdir().unwrap();
    std::fs::create_dir(t.path().join("dir")).unwrap();
        std::fs::write(t.path().join("file"), "").unwrap();
    assert_eq!(
        resolve_gather_glob(t.path(), t.path(), "dir/../file").unwrap(),
        BTreeSet::from(["file".into()])
    );
}

#[test]
fn lexical_aliases_resolve_to_one_path_identity() {
    let t = tempfile::tempdir().unwrap();
    std::fs::create_dir(t.path().join("dir")).unwrap();
    std::fs::write(t.path().join("file"), "").unwrap();
    let mut resolved = resolve_gather_glob(t.path(), t.path(), "file").unwrap();
    resolved.extend(resolve_gather_glob(t.path(), t.path(), "dir/../file").unwrap());
    assert_eq!(resolved, BTreeSet::from(["file".into()]));
}

#[test]
fn exclude_matching_preserves_expansion_results_and_diagnostics() {
    let t = tempfile::tempdir().unwrap();
    let member = t.path().join("member");
    for path in ["member/dir/deep/a.txt", "member/dir/.hidden", "member/root.txt", "member/.dot", "member/bin/out", "shared/root.txt"] {
        let path = t.path().join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "").unwrap();
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink("dir", member.join("alias")).unwrap();
        std::os::unix::fs::symlink("missing", member.join("dangling")).unwrap();
    }
    let candidates = resolve_gather_glob(&member, t.path(), "//**").unwrap();
    for pattern in ["**", "*", "*.txt", "dir/**", "./dir/", "//shared/**", "//member/dir/./**", "dir/../root.txt", "missing/../root.txt", "*/../root.txt", "**/.hidden", "**/bin/**", "[r]*.txt", "alias/**", "back\\slash.txt"] {
        let old = resolve_gather_glob(&member, t.path(), pattern).unwrap();
        let matcher = GatherExcludes::new(&member, t.path(), [pattern]).unwrap();
        let actual: BTreeSet<_> = candidates.iter().filter(|p| matcher.contains(p)).cloned().collect();
        let expected = candidates.intersection(&old).cloned().collect::<BTreeSet<_>>();
        assert_eq!(actual, expected, "exclude {pattern:?}");
    }
    for pattern in ["[", "a**b", "/bad", "//", "//dir/../file", "../outside"] {
        let old = resolve_gather_glob(&member, t.path(), pattern).unwrap_err();
        let new = GatherExcludes::new(&member, t.path(), [pattern]).err().unwrap();
        assert_eq!(new, old, "exclude {pattern:?}");
    }
}
