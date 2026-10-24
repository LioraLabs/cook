use super::*;
use super::covers::{anchor_pattern, escape_glob};

fn acc(t: &str) -> Vec<(String, bool, bool)> {
    parse_trace(t, Path::new("/start"))
        .into_iter()
        .map(|a| (a.path.to_string_lossy().into_owned(), a.absent, a.write))
        .collect()
}
fn one(p: &str, absent: bool, write: bool) -> Vec<(String, bool, bool)> {
    vec![(p.to_string(), absent, write)]
}

#[test]
fn absolute_path_read() {
    assert_eq!(acc("1 stat(\"/etc/x\", {st_mode=S_IFREG}) = 0"), one("/etc/x", false, false));
}

#[test]
fn unfinished_resumed_merge_per_pid() {
    let t = "7 openat(AT_FDCWD</d>, \"x\", O_RDONLY <unfinished ...>\n\
             8 stat(\"/other\", {}) = 0\n\
             7 <... openat resumed>) = 3</d/x>";
    assert_eq!(
        acc(t),
        vec![("/other".into(), false, false), ("/d/x".into(), false, false)]
    );
}

#[test]
fn at_fdcwd_annotation_resolves_and_refreshes_cwd() {
    let t = "1 openat(AT_FDCWD</d>, \"rel/x\", O_RDONLY|O_CLOEXEC) = 3</d/rel/x>\n\
             1 access(\"y\", R_OK) = 0";
    assert_eq!(acc(t), vec![("/d/rel/x".into(), false, false), ("/d/y".into(), false, false)]);
}

#[test]
fn dirfd_annotation_resolves_relative_path() {
    assert_eq!(
        acc("1 newfstatat(3</d/sub>, \"f\", {st_mode=S_IFREG}, 0) = 0"),
        one("/d/sub/f", false, false)
    );
}

#[test]
fn chdir_then_relative_access() {
    let t = "1 chdir(\"sub\") = 0\n1 access(\"f\", F_OK) = 0";
    assert_eq!(acc(t), one("/start/sub/f", false, false));
}

#[test]
fn fchdir_sets_cwd() {
    let t = "1 fchdir(4</elsewhere>) = 0\n1 access(\"f\", F_OK) = 0";
    assert_eq!(acc(t), one("/elsewhere/f", false, false));
}

#[test]
fn clone_child_inherits_cwd() {
    let t = "1 chdir(\"sub\") = 0\n\
             1 clone(child_stack=NULL, flags=CLONE_VM|SIGCHLD) = 2\n\
             2 access(\"f\", F_OK) = 0";
    assert_eq!(acc(t), one("/start/sub/f", false, false));
}

#[test]
fn enoent_is_absent_and_other_errors_skipped() {
    let t = "1 access(\"/a\", F_OK) = -1 ENOENT (No such file or directory)\n\
             1 access(\"/b\", F_OK) = -1 EACCES (Permission denied)\n\
             1 stat(\"/c/d\", 0x1) = -1 ENOTDIR (Not a directory)";
    assert_eq!(acc(t), vec![("/a".into(), true, false), ("/c/d".into(), true, false)]);
}

#[test]
fn write_flags_make_a_write() {
    for f in ["O_WRONLY|O_TRUNC", "O_RDONLY|O_TRUNC", "O_WRONLY|O_CREAT|O_EXCL"] {
        let t = format!("1 openat(AT_FDCWD</d>, \"o\", {f}, 0644) = 3</d/o>");
        assert_eq!(acc(&t), one("/d/o", false, true), "{f}");
    }
    // A write that keeps existing bytes reads them first.
    for f in ["O_WRONLY", "O_RDWR", "O_RDONLY|O_CREAT", "O_WRONLY|O_CREAT|O_APPEND"] {
        let t = format!("1 openat(AT_FDCWD</d>, \"o\", {f}, 0644) = 3</d/o>");
        assert_eq!(acc(&t), vec![("/d/o".into(), false, false), ("/d/o".into(), false, true)], "{f}");
    }
    assert_eq!(acc("1 openat(AT_FDCWD</d>, \"o\", O_RDONLY) = 3</d/o>"), one("/d/o", false, false));
}

#[test]
fn rename_paths_are_writes() {
    assert_eq!(
        acc("1 rename(\"a\", \"b\") = 0"),
        vec![("/start/a".into(), false, true), ("/start/b".into(), false, true)]
    );
}

#[test]
fn hex_and_quote_escapes_decode() {
    assert_eq!(
        acc("1 stat(\"/d/caf\\xc3\\xa9 \\\"q\\\"\", {}) = 0"),
        one("/d/caf\u{e9} \"q\"", false, false)
    );
}

#[test]
fn empty_path_skipped() {
    let t = "1 newfstatat(3</d>, \"\", {}, AT_EMPTY_PATH) = 0\n1 stat(\"/sib\", {}) = 0";
    assert_eq!(acc(t), one("/sib", false, false));
}

#[test]
fn getcwd_is_not_an_access() {
    assert!(acc("1 getcwd(\"/d\", 4096) = 3").is_empty());
}

#[test]
fn xattr_and_statfs_calls_read_only_their_first_path() {
    let t = "1 llistxattr(\".\", \"\", 152) = 0\n\
             1 getxattr(\"/proc/self/fd/4\", \"user.x\", 0x55, 103) = -1 ENODATA (No data available)\n\
             1 getxattr(\"/p\", \"user.x\", \"61184-65519\", 103) = 11\n\
             1 statfs(\".\", {f_type=EXT2_SUPER_MAGIC, f_namelen=255}) = 0";
    // ENODATA is neither ENOENT nor ENOTDIR: that call is skipped.
    assert_eq!(
        acc(t),
        vec![("/start".into(), false, false), ("/p".into(), false, false), ("/start".into(), false, false)]
    );
}

#[test]
fn symlinkat_writes_only_the_new_path() {
    assert_eq!(
        acc("1 symlinkat(\"/etc/hostname\", AT_FDCWD</d>, \"lnk\") = 0"),
        one("/d/lnk", false, true)
    );
    assert_eq!(acc("1 symlink(\"/etc/hostname\", \"lnk\") = 0"), one("/start/lnk", false, true));
}

#[test]
fn renameat2_with_two_dirfds_writes_both_paths() {
    let t = "1 renameat2(AT_FDCWD</d>, \"a\", AT_FDCWD</d>, \"b\", RENAME_NOREPLACE) = 0";
    assert_eq!(acc(t), vec![("/d/a".into(), false, true), ("/d/b".into(), false, true)]);
}

#[test]
fn linkat_writes_only_the_new_path() {
    let t = "1 linkat(AT_FDCWD</d>, \"b\", AT_FDCWD</d>, \"hl\", 0) = 0";
    assert_eq!(acc(t), one("/d/hl", false, true));
}

#[test]
fn explicit_write_table() {
    for call in [
        "mkdir(\"x\", 0777)", "unlink(\"x\")", "unlinkat(AT_FDCWD</d>, \"x\", 0)", "rmdir(\"x\")",
        "mknod(\"x\", S_IFIFO|0600, 0)",
        "creat(\"x\", 0644)",
    ] {
        let got = acc(&format!("1 {call} = 0"));
        assert_eq!(got.len(), 1, "{call}");
        assert!(got[0].2 && got[0].0.ends_with("/x"), "{call}: {got:?}");
    }
    // truncate keeps a prefix of the bytes: a read.
    assert_eq!(acc("1 truncate(\"/x\", 8) = 0"), one("/x", false, false));
}

#[test]
fn unknown_file_calls_are_reads_of_the_first_path() {
    assert_eq!(acc("1 inotify_add_watch(4, \"/w\", IN_ALL) = 1"), one("/w", false, false));
}

#[test]
fn paths_are_lexically_normalised() {
    assert_eq!(acc("1 stat(\"/a/./b/../c\", {}) = 0"), one("/a/c", false, false));
}

#[test]
fn execve_reads_only_the_program() {
    assert_eq!(
        acc("1 execve(\"/bin/cat\", [\"cat\", \"x\"], 0x7ff /* 3 vars */) = 0"),
        one("/bin/cat", false, false)
    );
}

fn uncovered_in(t: &str, declared: &[&str]) -> Vec<PathBuf> {
    let coverage = Coverage { exact: declared.iter().map(PathBuf::from).collect(), ..Coverage::default() };
    uncovered_reads(&parse_trace(t, Path::new("/wd")), &coverage, Path::new("/wd"), Path::new("/wd"), &[])
        .into_iter()
        .map(|(p, _)| p)
        .collect()
}

#[test]
fn unit_writes_cover_exactly_their_own_path() {
    let t = "1 openat(AT_FDCWD</wd>, \"out/o\", O_WRONLY|O_CREAT|O_TRUNC, 0644) = 3</wd/out/o>\n\
             1 openat(AT_FDCWD</wd>, \"out/o\", O_RDONLY) = 3</wd/out/o>\n\
             1 openat(AT_FDCWD</wd>, \"out/other\", O_RDONLY) = 3</wd/out/other>";
    assert_eq!(uncovered_in(t, &[]), vec![PathBuf::from("/wd/out/other")]);
}

#[test]
fn getcwd_statfs_llistxattr_on_the_working_dir_cover_nothing_below_it() {
    let t = "1 getcwd(\"/wd\", 4096) = 4\n\
             1 statfs(\".\", {f_type=1}) = 0\n\
             1 llistxattr(\".\", \"\", 152) = 0\n\
             1 openat(AT_FDCWD</wd>, \"sub/a\", O_RDONLY) = 3</wd/sub/a>";
    assert_eq!(uncovered_in(t, &[]), vec![PathBuf::from("/wd/sub/a")]);
}

#[test]
fn a_declared_path_does_not_cover_its_descendants() {
    let t = "1 openat(AT_FDCWD</wd>, \"d/x\", O_RDONLY) = 3</wd/d/x>";
    assert_eq!(uncovered_in(t, &["/wd/d"]), vec![PathBuf::from("/wd/d/x")]);
}

#[test]
fn read_then_rewrite_stays_an_uncovered_read() {
    let t = "1 openat(AT_FDCWD</wd>, \"f\", O_RDONLY) = 3</wd/f>\n\
             1 openat(AT_FDCWD</wd>, \"f\", O_WRONLY|O_TRUNC) = 4</wd/f>\n\
             1 openat(AT_FDCWD</wd>, \"f\", O_RDONLY) = 3</wd/f>";
    assert_eq!(uncovered_in(t, &[]), vec![PathBuf::from("/wd/f"), PathBuf::from("/wd/f")]);
}

#[test]
fn absent_lookup_then_create_is_covered() {
    let t = "1 stat(\"g\", 0x1) = -1 ENOENT (No such file or directory)\n\
             1 openat(AT_FDCWD</wd>, \"g\", O_WRONLY|O_CREAT, 0644) = 3</wd/g>\n\
             1 openat(AT_FDCWD</wd>, \"g\", O_RDONLY) = 3</wd/g>";
    assert!(uncovered_in(t, &[]).is_empty());
}

#[test]
fn audit_refuses_outside_the_isolated_view() {
    let dir = tempfile::tempdir().unwrap();
    let ws = RegisteredWorkspace {
        warnings: vec![],
        names: vec![],
        units_by_recipe: BTreeMap::new(),
        probes: BTreeMap::new(),
        resolved_probe_keys: Default::default(),
        working_dir_by_prefix: BTreeMap::new(),
        alias_dirs_by_prefix: BTreeMap::new(),
        terminal_outputs: BTreeMap::new(),
        materializations: vec![],
    };
    let store = tempfile::tempdir().unwrap();
    let ctx = scratch_cache_ctx(dir.path(), store.path()).unwrap();
    let e = audit(dir.path(), &ws, &BTreeMap::new(), &BTreeSet::new(), 1, ctx).unwrap_err();
    assert_eq!(e, "audit must run inside the isolated project view");
}

fn h(p: &str, k: FindingKind) -> (PathBuf, FindingKind) {
    (PathBuf::from(p), k)
}

#[test]
fn collapse_roots_and_counts() {
    let hits = [
        h("/proj/.lintrc", FindingKind::Read),
        h("/proj/node_modules/a/x.js", FindingKind::Read),
        h("/proj/node_modules/b/y.js", FindingKind::Read),
        h("/home/u/.cache/z", FindingKind::Read),
        h("/home/u/.cache/z", FindingKind::Read),
        h("/usr/lib/libc.so", FindingKind::Read),
        h("/usr/lib/x/libm.so", FindingKind::Read),
        h("/etc", FindingKind::Read),
        h("/proj/missing.cfg", FindingKind::Absent),
    ];
    let f = collapse("r", &hits, Path::new("/proj"), Some(Path::new("/home/u")));
    let got: Vec<(FindingKind, &str, usize)> = f.iter().map(|f| (f.kind, f.root.as_str(), f.count)).collect();
    assert_eq!(
        got,
        vec![
            (FindingKind::Read, ".lintrc", 1),
            (FindingKind::Read, "/etc", 1),
            (FindingKind::Read, "/usr/lib", 2),
            (FindingKind::Read, "node_modules", 2),
            (FindingKind::Read, "~/.cache", 1),
            (FindingKind::Absent, "missing.cfg", 1),
        ]
    );
    let nm = f.iter().find(|f| f.root == "node_modules").unwrap();
    assert_eq!(nm.paths, vec!["node_modules/a/x.js", "node_modules/b/y.js"]);
    assert_eq!(serde_json::to_value(&f[0]).unwrap()["kind"], "read");
}

// --- end to end ------------------------------------------------------------

mod e2e {
    use super::super::*;
    use cook_contracts::probe::{ProbeInputs, ProbeUnit};
    use cook_contracts::probe_key::{qualified_key, LocalProbeKey};
    use cook_contracts::{CacheMeta, CapturedUnit, DepKind, RecipeUnits};

    /// Env is process-wide and set once, before anything reads the cache root.
    fn isolate_cache() {
        static DIR: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
        DIR.get_or_init(|| {
            let d = tempfile::tempdir().unwrap();
            std::env::set_var("XDG_CACHE_HOME", d.path());
            d
        });
    }

    fn workspace(root: &Path, seals: &[(&str, &str)]) -> RegisteredWorkspace {
        let mut probes = BTreeMap::new();
        let mut units_probes = Vec::new();
        for (key, file) in seals {
            let p = ProbeUnit {
                covers: Vec::new(),
                key: LocalProbeKey::new(*key),
                produce_source: cook_contracts::probe_value::FILES_MANIFEST_PRODUCE.to_string(),
                produce_line: 1,
                inputs: ProbeInputs { files: vec![file.to_string()], ..Default::default() },
            };
            probes.insert(qualified_key("", &p.key), p.clone());
            units_probes.push(p);
        }
        let meta = CacheMeta {
            recipe_name: "lint".into(),
            project_id: String::new(),
            cookfile_path: String::new(),
            cache_key: "k".into(),
            inputs: vec![],
            consumes: vec![],
            member_keyed: false,
            output_paths: vec![],
            command_hash: 0,
            env_contribution: 0,
            consulted_env: BTreeMap::new(),
            discovered_inputs: None,
            seal_keys: seals.iter().map(|(k, _)| LocalProbeKey::new(*k)).collect(),
            sharing: Default::default(),
            record: false,
        };
        let unit = CapturedUnit {
            payload: WorkPayload::Shell {
                cmd: "cat .lintrc src/a.ts >/dev/null; test -f missing.cfg || true".into(),
                line: 1,
            },
            cache_meta: Some(meta),
            dep_kind: DepKind::Sequential,
            probes: seals.iter().map(|(k, _)| k.to_string()).collect(),
            unit_env_vars: Default::default(),
            member: None,
            output_paths: vec![],
            after: vec![],
            test_name: None,
        };
        let mut units_by_recipe = BTreeMap::new();
        units_by_recipe.insert(
            "lint".to_string(),
            RecipeUnits {
                recipe_name: "lint".into(),
                deps: vec![],
                units: vec![unit],
                step_groups: vec![],
                working_dir: root.to_path_buf(),
                env_vars: BTreeMap::new(),
                terminal_outputs: vec![],
                dep_edges: vec![],
                probes: units_probes,
            },
        );
        let mut wd = BTreeMap::new();
        wd.insert(String::new(), root.to_path_buf());
        RegisteredWorkspace {
            warnings: vec![],
            names: vec![],
            units_by_recipe,
            probes,
            resolved_probe_keys: Default::default(),
            working_dir_by_prefix: wd,
            alias_dirs_by_prefix: BTreeMap::new(),
            terminal_outputs: BTreeMap::new(),
            materializations: vec![],
        }
    }

    fn run_audit(root: &Path, seals: &[(&str, &str)]) -> AuditReport {
        let ws = workspace(root, seals);
        let edges = BTreeMap::from([("lint".to_string(), vec![])]);
        let reachable = BTreeSet::from(["lint".to_string()]);
        let store = tempfile::tempdir().unwrap();
        let ctx = scratch_cache_ctx(root, store.path()).unwrap();
        audit_in_place(root, &ws, &edges, &reachable, 1, ctx).expect("audit")
    }

    #[test]
    fn reports_undeclared_reads_and_absent_probes() {
        if cook_cache::resolve_tool_path("strace").is_none() {
            eprintln!("skip: strace not installed");
            return;
        }
        isolate_cache();
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join(".lintrc"), "x").unwrap();
        std::fs::write(root.join("src/a.ts"), "y").unwrap();

        let r = run_audit(&root, &[("srcs", "src/a.ts")]);
        assert_eq!(r.errors(), 0, "{:?}", r.units);
        let find = |r: &AuditReport, kind, root: &str| r.findings.iter().any(|f| f.kind == kind && f.root == root);
        assert!(find(&r, FindingKind::Read, ".lintrc"), "{:?}", r.findings);
        assert!(find(&r, FindingKind::Absent, "missing.cfg"), "{:?}", r.findings);
        assert!(!r.findings.iter().any(|f| f.paths.iter().any(|p| p.contains("src/a.ts"))), "{:?}", r.findings);
        assert_eq!(r.exit_code(false), 1);

        let r = run_audit(&root, &[("srcs", "src/a.ts"), ("cfg", ".lintrc")]);
        assert_eq!(r.errors(), 0, "{:?}", r.units);
        assert!(!find(&r, FindingKind::Read, ".lintrc"), "{:?}", r.findings);
        assert!(find(&r, FindingKind::Absent, "missing.cfg"));
    }
}

#[test]
fn touch_then_read_and_append_keep_the_existing_file_an_input() {
    // Real strace 7.2: `touch f; cat f` and `echo x >> db; cat db`.
    let t = "1 openat(AT_FDCWD</wd>, \"f\", O_WRONLY|O_CREAT|O_NOCTTY|O_NONBLOCK, 0666) = 3</wd/f>\n\
             1 utimensat(3</wd/f>, NULL, NULL, 0) = 0\n\
             1 openat(AT_FDCWD</wd>, \"f\", O_RDONLY) = 3</wd/f>\n\
             1 openat(AT_FDCWD</wd>, \"db\", O_WRONLY|O_CREAT|O_APPEND, 0666) = 3</wd/db>";
    assert_eq!(
        uncovered_in(t, &[]),
        vec![PathBuf::from("/wd/f"), PathBuf::from("/wd/f"), PathBuf::from("/wd/db")]
    );
}

#[test]
fn metadata_calls_are_not_accesses() {
    let t = "1 chmod(\"/x\", 0644) = 0\n1 utimensat(AT_FDCWD</d>, \"y\", NULL, 0) = 0\n1 stat(\"/s\", 0x1) = 0";
    assert_eq!(acc(t), one("/s", false, false));
}

#[test]
fn a_non_path_fd_annotation_is_not_a_base() {
    let t = "1 inotify_add_watch(4<anon_inode:inotify>, \"rel\", IN_MODIFY) = 1";
    assert_eq!(acc(t), one("/start/rel", false, false));
}

#[test]
fn embedded_project_and_home_paths_are_elided() {
    let (root, home) = (Path::new("/home/alex/x/proj"), Some(Path::new("/home/alex")));
    let sp = |p: &str| spelling(Path::new(p), root, home);
    assert_eq!(sp("/home/alex/x/proj/public/home/alex/x/proj/src/a.svg"), "public/<project>/src/a.svg");
    assert_eq!(sp("/home/alex/x/proj/a/home/alex/.cfg"), "a/~/.cfg");
    assert_eq!(sp("/home/alex/x/proj/a/home/alex/x/project2/b"), "a/~/x/project2/b");
    assert_eq!(sp("/home/alex/.cfg"), "~/.cfg");
    // Only a relative remainder is elided: an absolute path outside both stays as it is.
    assert_eq!(sp("/srv/home/alex/x"), "/srv/home/alex/x");
    assert_eq!(sp("/srv/home/alex/x/proj/y"), "/srv/home/alex/x/proj/y");
}

#[test]
fn a_covered_subtree_covers_everything_under_it_and_its_ancestors() {
    let t = "1 openat(AT_FDCWD</wd>, \"/opt/tc/lib/x.so\", O_RDONLY) = 3</opt/tc/lib/x.so>\n\
             1 newfstatat(AT_FDCWD</wd>, \"/opt\", {st_mode=S_IFDIR|0755, ...}, 0) = 0\n\
             1 openat(AT_FDCWD</wd>, \"/opt/other\", O_RDONLY) = 3</opt/other>\n";
    let coverage = Coverage { subtrees: BTreeSet::from([PathBuf::from("/opt/tc")]), ..Coverage::default() };
    let hits: Vec<PathBuf> =
        uncovered_reads(&parse_trace(t, Path::new("/wd")), &coverage, Path::new("/wd"), Path::new("/wd"), &[])
            .into_iter()
            .map(|(p, _)| p)
            .collect();
    assert_eq!(hits, vec![PathBuf::from("/opt/other")]);
}

mod env_names {
    use super::*;
    use cook_cache::envkey::EnvDenylist;
    use cook_contracts::{CacheMeta, LocalProbeKey, ProbeUnit, RecipeUnits};

    fn set(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    fn unit_with(consulted: &[&str], sealed_probe: Option<&str>) -> (CacheMeta, RecipeUnits) {
        let key = LocalProbeKey::new("p");
        let meta = CacheMeta {
            consulted_env: consulted.iter().map(|k| (k.to_string(), "v".to_string())).collect(),
            seal_keys: sealed_probe.iter().map(|_| key.clone()).collect(),
            ..Default::default()
        };
        let probes = sealed_probe
            .map(|src| ProbeUnit {
                key: key.clone(),
                produce_source: src.to_string(),
                produce_line: 1,
                inputs: Default::default(),
                covers: Vec::new(),
            })
            .into_iter()
            .collect();
        let units = RecipeUnits {
            recipe_name: "r".into(),
            deps: vec![],
            units: vec![],
            step_groups: vec![],
            working_dir: PathBuf::from("/wd"),
            env_vars: BTreeMap::new(),
            terminal_outputs: vec![],
            dep_edges: vec![],
            probes,
        };
        (meta, units)
    }

    #[test]
    fn a_consulted_var_keys_no_environment_name() {
        let (meta, units) = unit_with(&["NODE_ENV", "mode"], None);
        assert!(keyed_env(&meta, &units).is_empty());
    }

    #[test]
    fn a_sealed_probes_literal_getenv_keys_the_name() {
        let (meta, units) = unit_with(&[], Some("local v = os.getenv(\"NODE_ENV\")\nreturn getenv(dyn)"));
        let keyed = keyed_env(&meta, &units);
        assert_eq!(keyed, set(&["NODE_ENV"]));
    }

    #[test]
    fn an_unsealed_probe_keys_nothing() {
        let (mut meta, units) = unit_with(&[], Some("os.getenv('NODE_ENV')"));
        meta.seal_keys.clear();
        assert!(keyed_env(&meta, &units).is_empty());
    }

    #[test]
    fn the_key_denylist_the_audit_ignores_and_the_recorders_own_variables_are_never_reported() {
        let mut deny = EnvDenylist::baseline();
        deny.extend_with(&["MY_NOISE_*".to_string()]);
        let mut queried = set(&[
            "HOME", "XDG_DATA_DIRS", "MY_NOISE_X", "LANG", "LANGUAGE", "LC_ALL", "LC_CTYPE", "LD_LIBRARY_PATH",
            cook_envtrace::OUT_VAR, "NODE_ENV", "LANGUAGES", "LCX",
        ]);
        queried.extend(cook_envtrace::REFERENCES.map(|(name, _)| name.to_string()));
        assert_eq!(unkeyed_env(&queried, &BTreeSet::new(), &deny), set(&["LANGUAGES", "LCX", "NODE_ENV"]));
    }

    #[test]
    fn read_queried_dedupes_and_a_missing_file_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("n");
        std::fs::write(&f, "1 1 - 0 3 A\n1 1 - 0 3 B\n2 2 - 0 3 A\n\ngarbage\n").unwrap();
        assert_eq!(read_queried(&f), set(&["A", "B"]));
        assert!(!f.exists());
        assert!(read_queried(&f).is_empty());
    }

    /// `lines` as one thread's lookups of a three-entry environment X, Y, NODE_ENV:
    /// `index count NAME`, the index prefixed `L` for a long-valued name.
    fn queried(lines: &[&str]) -> BTreeSet<String> {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("n");
        let text: String = lines
            .iter()
            .map(|l| match l.strip_prefix('L') {
                Some(rest) => rest.split_once(' ').map(|(i, r)| format!("7 7 {i} 1 {r}\n")).unwrap(),
                None => l.split_once(' ').map(|(i, r)| format!("7 7 {i} 0 {r}\n")).unwrap(),
            })
            .collect();
        std::fs::write(&f, text).unwrap();
        read_queried(&f)
    }

    #[test]
    fn a_walk_of_the_whole_environment_is_a_copy_not_a_read() {
        // node spawning a child: every name in environ order, each looked up thrice.
        let walk = ["0 3 X", "0 3 X", "0 3 X", "1 3 Y", "1 3 Y", "1 3 Y", "2 3 NODE_ENV", "2 3 NODE_ENV", "2 3 NODE_ENV"];
        assert_eq!(queried(&walk), BTreeSet::new());
        // Unset names between steps are kept and do not break it.
        assert_eq!(queried(&["0 3 X", "- 3 UNSET", "1 3 Y", "2 3 NODE_ENV"]), set(&["UNSET"]));
    }

    #[test]
    fn point_reads_around_a_walk_are_kept() {
        let walk = ["0 3 X", "1 3 Y", "2 3 NODE_ENV"];
        let mut before_and_after = vec!["2 3 NODE_ENV", "- 3 UNSET"];
        before_and_after.extend(walk);
        before_and_after.extend(["- 3 UNSET", "2 3 NODE_ENV"]);
        assert_eq!(queried(&before_and_after), set(&["NODE_ENV", "UNSET"]));
        // A read of the first name, later followed by a walk, is not absorbed into it.
        let mut first = vec!["0 3 X", "- 3 UNSET"];
        first.extend(walk);
        assert_eq!(queried(&first), set(&["UNSET", "X"]));
    }

    #[test]
    fn a_first_or_last_name_looked_up_beyond_the_copys_repeat_count_is_a_read() {
        // A point read of the first name straight before the copy.
        assert_eq!(queried(&["0 3 X", "0 3 X", "1 3 Y", "2 3 NODE_ENV"]), set(&["X"]));
        // The same with node's three lookups per name.
        let mut thrice: Vec<&str> = vec!["0 3 X"; 4];
        thrice.extend(["1 3 Y"; 3]);
        thrice.extend(["2 3 NODE_ENV"; 3]);
        assert_eq!(queried(&thrice), set(&["X"]));
        // A read of the last name straight after it.
        assert_eq!(queried(&["0 3 X", "1 3 Y", "2 3 NODE_ENV", "2 3 NODE_ENV"]), set(&["NODE_ENV"]));
    }

    #[test]
    fn a_name_read_inside_a_loop_over_the_copy_is_a_read() {
        // vite's loadEnv: for-in over process.env (two passes) reading each VITE_* name in the body.
        let mut walk = vec!["0 3 X", "1 3 VITE_X", "2 3 NODE_ENV"];
        walk.extend(["0 3 X", "1 3 VITE_X", "1 3 VITE_X", "2 3 NODE_ENV"]);
        assert_eq!(queried(&walk), set(&["VITE_X"]));
        // A uniform copy, three lookups a name, stays dropped.
        let mut thrice: Vec<&str> = vec!["0 3 X"; 3];
        thrice.extend(["1 3 VITE_X"; 3]);
        thrice.extend(["2 3 NODE_ENV"; 3]);
        assert_eq!(queried(&thrice), BTreeSet::new());
        // A loop reading every name equally often cannot be told from a copy.
        assert_eq!(queried(&["0 3 X", "0 3 X", "1 3 VITE_X", "1 3 VITE_X", "2 3 NODE_ENV", "2 3 NODE_ENV"]), BTreeSet::new());
    }

    #[test]
    fn a_long_valued_name_in_a_copy_is_measured_against_its_own_kind() {
        // node looks a value of 256 bytes or more up five times where short ones get three.
        let mut copy: Vec<&str> = vec!["0 3 X"; 3];
        copy.extend(["L1 3 LS_COLORS"; 5]);
        copy.extend(["2 3 NODE_ENV"; 3]);
        assert_eq!(queried(&copy), BTreeSet::new());
        // A long name looked up more than its kind's others is still a read.
        let mut more: Vec<&str> = vec!["L0 3 X"; 5];
        more.extend(["L1 3 Y"; 7]);
        more.extend(["2 3 NODE_ENV"; 3]);
        assert_eq!(queried(&more), set(&["Y"]));
        // JSON.stringify: one lookup a short name, two a long one.
        assert_eq!(queried(&["0 3 X", "L1 3 LS_COLORS", "L1 3 LS_COLORS", "2 3 NODE_ENV"]), BTreeSet::new());
    }

    #[test]
    fn a_lone_long_valued_read_survives_next_to_the_reference_long_run() {
        // The recorder's reference long name is only copied (5); NODE_ENV, the one other long name, is read once more (6).
        let mut block: Vec<&str> = vec!["L0 4 __cook_envtrace_long"; 5];
        block.extend(["L1 4 NODE_ENV"; 6]);
        block.extend(["2 4 X"; 3]);
        block.extend(["3 4 __cook_envtrace_short"; 3]);
        assert_eq!(queried(&block), set(&["NODE_ENV"]));
    }

    #[test]
    fn a_pick_in_a_loop_over_the_copy_is_a_read_whatever_its_value_length() {
        // for-in reading the picks: a pass of single lookups, then one reading the picked name.
        let mut long_pick = vec!["0 3 X", "L1 3 LS_COLORS", "L2 3 VITE_LONG"];
        long_pick.extend(["0 3 X", "L1 3 LS_COLORS", "L2 3 VITE_LONG", "L2 3 VITE_LONG", "L2 3 VITE_LONG"]);
        assert_eq!(queried(&long_pick), set(&["VITE_LONG"]));
        let mut short_pick = vec!["0 3 X", "1 3 NODE_ENV", "L2 3 LS_COLORS"];
        short_pick.extend(["0 3 X", "0 3 X", "1 3 NODE_ENV", "L2 3 LS_COLORS"]);
        assert_eq!(queried(&short_pick), set(&["X"]));
    }

    #[test]
    fn a_partial_or_out_of_order_run_is_reads() {
        assert_eq!(queried(&["0 3 X", "1 3 Y"]), set(&["X", "Y"]));
        assert_eq!(queried(&["0 3 X", "2 3 NODE_ENV", "1 3 Y"]), set(&["NODE_ENV", "X", "Y"]));
        // Two threads each reading part of the environment do not make one walk.
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("n");
        std::fs::write(&f, "7 7 0 0 3 X\n7 8 1 0 3 Y\n7 7 2 0 3 NODE_ENV\n").unwrap();
        assert_eq!(read_queried(&f), set(&["NODE_ENV", "X", "Y"]));
        // A one-entry environment: the walk and the read look alike, so it is a read.
        assert_eq!(queried(&["0 1 NODE_ENV"]), set(&["NODE_ENV"]));
    }
}

// --- programs that never loaded the recorder ---

const SO: &str = "/scratch/libcook_envtrace.so";

fn unobserved(t: &str) -> Vec<PathBuf> {
    unobserved_programs(t, Path::new("/start"), Path::new(SO)).into_iter().collect()
}

fn loads(pid: u32) -> String {
    format!("{pid} openat(AT_FDCWD, \"{SO}\", O_RDONLY|O_CLOEXEC) = 3</scratch/libcook_envtrace.so>\n")
}

#[test]
fn a_segment_that_opened_the_recorder_is_observed() {
    let t = format!("5 execve(\"/bin/sh\", [\"sh\"], 0x7ff /* 3 vars */) = 0\n{}", loads(5));
    assert_eq!(unobserved(&t), Vec::<PathBuf>::new());
}

#[test]
fn a_re_exec_in_the_same_pid_without_a_new_open_is_named() {
    let t = format!(
        "5 execve(\"/bin/env\", [\"env\"], 0x7ff /* 3 vars */) = 0\n{}\
         5 execve(\"/usr/bin/node\", [\"node\"], 0x7ff /* 0 vars */) = 0\n",
        loads(5)
    );
    assert_eq!(unobserved(&t), vec![PathBuf::from("/usr/bin/node")]);
}

#[test]
fn a_failed_open_does_not_count() {
    let t = format!(
        "5 execve(\"/bin/a\", [\"a\"], 0x7ff /* 3 vars */) = 0\n\
         5 openat(AT_FDCWD, \"{SO}\", O_RDONLY|O_CLOEXEC) = -1 ENOENT (No such file or directory)\n"
    );
    assert_eq!(unobserved(&t), vec![PathBuf::from("/bin/a")]);
}

#[test]
fn a_failed_execve_starts_no_segment() {
    let t = format!(
        "5 execve(\"/bin/a\", [\"a\"], 0x7ff /* 3 vars */) = 0\n{}\
         5 execve(\"/nope/b\", [\"b\"], 0x7ff /* 0 vars */) = -1 ENOENT (No such file or directory)\n",
        loads(5)
    );
    assert_eq!(unobserved(&t), Vec::<PathBuf>::new());
}

#[test]
fn split_execve_and_open_lines_are_joined() {
    let t = format!(
        "7 execve(\"tools/stat\", [\"stat\"], 0x7ff /* 3 vars */ <unfinished ...>\n\
         8 stat(\"/other\", {{}}) = 0\n\
         7 <... execve resumed>) = 0\n\
         7 openat(AT_FDCWD, \"{SO}\", O_RDONLY <unfinished ...>\n\
         7 <... openat resumed>) = 3</scratch/libcook_envtrace.so>\n\
         9 execve(\"tools/other\", [\"o\"], 0x7ff /* 0 vars */ <unfinished ...>\n\
         9 <... execve resumed>) = 0\n"
    );
    assert_eq!(unobserved(&t), vec![PathBuf::from("/start/tools/other")]);
}

fn uncovered_with(paths: &[(&str, bool)], coverage: &Coverage) -> Vec<String> {
    let t: String = paths
        .iter()
        .map(|(p, absent)| {
            if *absent {
                format!("1 newfstatat(AT_FDCWD</wd>, \"{p}\", {{st_mode=S_IFREG}}, 0) = -1 ENOENT (No such file or directory)\n")
            } else {
                format!("1 openat(AT_FDCWD</wd>, \"{p}\", O_RDONLY) = 3<{p}>\n")
            }
        })
        .collect();
    uncovered_reads(&parse_trace(&t, Path::new("/wd")), coverage, Path::new("/wd"), Path::new("/wd"), &[])
        .into_iter()
        .map(|(p, _)| p.to_string_lossy().into_owned())
        .collect()
}

#[test]
fn a_literal_root_covers_reads_and_absent_lookups_below_it() {
    let c = Coverage { subtrees: BTreeSet::from([PathBuf::from("/wd/vendor")]), ..Coverage::default() };
    let hits = uncovered_with(&[("/wd/vendor/a/b", false), ("/wd/vendor/x/y", true), ("/wd/src/a", false)], &c);
    assert_eq!(hits, vec!["/wd/src/a"]);
}

#[test]
fn a_glob_root_covers_any_depth_and_only_the_named_directories() {
    let c = Coverage { patterns: vec!["/wd/**/node_modules".into()], ..Coverage::default() };
    let hits = uncovered_with(
        &[
            ("/wd/node_modules/a/b", false),
            ("/wd/pkg/node_modules/x", false),
            ("/wd/src/deep/node_modules", true),
            ("/wd/src/a.ts", false),
            ("/wd/node_modules_not/a", false),
        ],
        &c,
    );
    assert_eq!(hits, vec!["/wd/src/a.ts", "/wd/node_modules_not/a"]);
}

#[test]
fn a_single_star_does_not_cross_a_separator() {
    let c = Coverage { patterns: vec!["/wd/*/lib".into()], ..Coverage::default() };
    let hits = uncovered_with(&[("/wd/a/lib/x", false), ("/wd/a/b/lib/x", false)], &c);
    assert_eq!(hits, vec!["/wd/a/b/lib/x"]);
}

#[test]
fn a_pattern_anchor_with_metacharacters_matches_literally() {
    let anchor = escape_glob(Path::new("/wd/we[ir]d{a}*?/x"));
    let c = Coverage { patterns: vec![format!("{anchor}/**/nm")], ..Coverage::default() };
    let hits = uncovered_with(
        &[("/wd/we[ir]d{a}*?/x/p/nm/f", false), ("/wd/weid{a}*?/x/p/nm/f", false), ("/wd/we[ir]d{a}*?/x/p/q", false)],
        &c,
    );
    assert_eq!(hits, vec!["/wd/weid{a}*?/x/p/nm/f", "/wd/we[ir]d{a}*?/x/p/q"]);
}

#[test]
fn an_invalid_pattern_is_an_error_not_dropped() {
    assert!(ConsumesFilter::compile(&["/wd/[".to_string()]).is_err());
}

#[test]
fn covers_glob_literal_prefix_resolves_like_a_path() {
    assert_eq!(anchor_pattern(Path::new("/wd"), "./**/node_modules"), "/wd/**/node_modules");
    assert_eq!(anchor_pattern(Path::new("/wd"), "**/node_modules"), "/wd/**/node_modules");
    assert_eq!(anchor_pattern(Path::new("/wd/sub"), "../a/**"), "/wd/a/**");
    assert_eq!(anchor_pattern(Path::new("/wd"), "/opt/x/**"), "/opt/x/**");
    assert_eq!(anchor_pattern(Path::new("/w[d]"), "**/n"), "/w[[]d[]]/**/n");
}

#[test]
fn covers_entry_resolving_to_a_too_broad_root_is_refused() {
    let deep = Path::new("/a/b/c/d/e/f/g");
    let home = Path::new("/home/me");
    for e in ["../../../../../../..", "../../../../../../../**", "../../../../../../../**/x", "~/..", "~/../**", "~/../me", "~/[.a-z]*", "/usr/.", "/usr/../usr", "/", "/h*", "/home/me", "/home/me/**"] {
        let r = covers::resolve(e, deep, Some(home));
        assert!(r.as_ref().is_err_and(|m| m.contains(e) && m.contains("too broad")), "{e}: {:?}", r.map(|c| c.is_some()));
    }
    for e in ["../../x", "**/node_modules", "/opt/x/**", "~/.nvm/*/bin", "~/.rustup/toolchains"] {
        assert!(matches!(covers::resolve(e, deep, Some(home)), Ok(Some(_))), "{e}");
    }
    // A project at a top-level directory still covers its own node_modules.
    assert!(matches!(covers::resolve("**/node_modules", Path::new("/app"), Some(home)), Ok(Some(_))));
}

#[test]
fn a_member_lexically_inside_but_symlinked_out_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap().join("proj");
    let elsewhere = std::fs::canonicalize(tmp.path()).unwrap().join("elsewhere");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&elsewhere).unwrap();
    std::os::unix::fs::symlink(&elsewhere, root.join("lib")).unwrap();
    std::fs::create_dir(root.join("real")).unwrap();
    let inside = root.join("real");
    let linked = root.join("lib");
    assert!(refuse_members_outside(&root, [("ok", inside.as_path())]).is_ok());
    let err = refuse_members_outside(&root, [("lib", linked.as_path())]).unwrap_err();
    assert!(err.contains("workspace member `lib`") && err.contains("outside the project"), "{err}");
}

#[test]
fn finding_kind_spellings_match_serde_and_all_is_exhaustive() {
    for k in FindingKind::ALL {
        // No wildcard: a new variant stops compiling here until it is in ALL.
        match k {
            FindingKind::Read | FindingKind::Absent | FindingKind::Env | FindingKind::EnvUnobserved => {}
        }
        assert_eq!(serde_json::to_value(k).unwrap(), k.as_str());
    }
    let distinct: std::collections::BTreeSet<_> = FindingKind::ALL.iter().collect();
    assert_eq!(distinct.len(), 4);
}

#[test]
fn exit_code_is_the_one_rule() {
    let finding = Finding { recipe: "a".into(), kind: FindingKind::Read, root: "x".into(), count: 1, paths: vec!["x".into()] };
    let unit = |status| UnitAudit { recipe: "a".into(), unit: "u".into(), key: "k".into(), status, detail: String::new() };
    let mut r = AuditReport::default();
    assert_eq!((r.exit_code(false), r.exit_code(true)), (0, 0));
    r.findings.push(finding);
    assert_eq!((r.exit_code(false), r.exit_code(true)), (1, 0));
    r.units.push(unit("error"));
    assert_eq!((r.exit_code(false), r.exit_code(true)), (1, 1));
}
