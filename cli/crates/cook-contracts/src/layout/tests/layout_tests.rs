use super::*;

/// CS-0207: two candidates in the installed tree, and NO hand-vendored top
/// level. The removed pair is what §{cat.bootstrap.vendor}'s shadowing
/// workflow rested on; with it gone there is no precedence rule left to test,
/// which is the point.
#[test]
fn candidates_are_the_installed_tree_only() {
    let wd = Path::new("/proj");
    let c = module_candidates(wd, "cook_cc");
    assert_eq!(c.len(), 2);
    assert_eq!(c[0], Path::new("/proj/.cook/modules/share/lua/5.4/cook_cc.lua"));
    assert_eq!(c[1], Path::new("/proj/.cook/modules/share/lua/5.4/cook_cc/init.lua"));
    for p in &c {
        assert!(
            !p.to_string_lossy().contains(LEGACY_MODULES_DIR),
            "the retired root must not be probed: {}",
            p.display()
        );
    }
}

/// The composed search paths MUST mirror `module_candidates`. A name that
/// resolves through one and not the other is the "works at register, missing
/// at execute" failure this module exists to prevent.
#[test]
fn search_paths_mirror_the_candidates_and_keep_the_original_suffix() {
    let p = compose_lua_search_paths(Path::new("/proj"), "ORIG_PATH", "ORIG_CPATH");
    let ext = native_lua_ext();
    assert_eq!(
        p.path,
        "/proj/.cook/modules/share/lua/5.4/?.lua;\
         /proj/.cook/modules/share/lua/5.4/?/init.lua;\
         ORIG_PATH"
    );
    assert_eq!(
        p.cpath,
        format!("/proj/.cook/modules/lib/lua/5.4/?.{ext};ORIG_CPATH")
    );
    assert!(!p.path.contains(LEGACY_MODULES_DIR));
    assert!(!p.cpath.contains(LEGACY_MODULES_DIR));
}

/// The rocks tree lives under the SAME `.cook/` root as the caches, and that
/// is what keeps CS-0204 sound across the move: an installed module still
/// resolves inside the unit's working directory, so `relative_module_paths`
/// keeps it in the recorded determinant set instead of dropping it.
#[test]
fn the_installed_tree_stays_under_the_working_directory() {
    let wd = Path::new("/proj");
    let resolved = module_candidates(wd, "cook_cc")[0].clone();
    assert!(resolved.starts_with(wd));
    assert_eq!(
        relative_module_paths(wd, &[resolved]),
        vec![".cook/modules/share/lua/5.4/cook_cc.lua".to_string()]
    );
}

#[test]
fn index_basename_round_trips() {
    for name in ["build", "@cap/env:build", "50%/done", "a%2Fb"] {
        assert_eq!(decode_index_basename(&encode_index_basename(name)), name);
    }
    assert_eq!(encode_index_basename("@cap/env:build"), "@cap%2Fenv:build");
}

#[test]
fn dot_cook_tree() {
    let b = Path::new("/p");
    assert_eq!(cache_dir(b), Path::new("/p/.cook/cache"));
    assert_eq!(probes_dir(b), Path::new("/p/.cook/probes"));
    assert_eq!(logs_dir(b), Path::new("/p/.cook/logs"));
}

// ---------------------------------------------------------------------------
// Path-addressed modules (§12.2.1, CS-0206)
// ---------------------------------------------------------------------------

mod use_paths {
    use crate::layout::*;
    use std::path::Path;

    #[test]
    fn a_tree_relative_path_normalises_to_one_spelling() {
        // §12.3.2: two spellings of one file must be ONE module instance.
        for raw in [
            "build/helpers.lua",
            "./build/helpers.lua",
            "build/./helpers.lua",
            "build//helpers.lua",
        ] {
            assert_eq!(normalise_use_path(raw), Ok("build/helpers.lua".to_string()), "{raw}");
        }
    }

    #[test]
    fn the_sigil_is_refused_as_a_sigil_not_as_an_absolute_path() {
        // `//x` is also `/x`-shaped. Reporting it as "absolute" would tell an
        // author to make it relative when the real answer is that `use` does
        // not admit `import`'s workspace-root reach at all.
        assert_eq!(
            normalise_use_path("//build/helpers.lua"),
            Err(UsePathRejection::Sigil)
        );
        let msg = use_path_rejected_message("//build/helpers.lua", UsePathRejection::Sigil);
        assert!(msg.contains("//build/helpers.lua"));
        assert!(msg.contains("§12.2.1"));
    }

    #[test]
    fn absolute_and_dotdot_are_refused() {
        assert_eq!(
            normalise_use_path("/opt/cook/helpers.lua"),
            Err(UsePathRejection::Absolute)
        );
        assert_eq!(
            normalise_use_path("../shared/helpers.lua"),
            Err(UsePathRejection::DotDotSegment)
        );
        // A `..` anywhere, not only in front.
        assert_eq!(
            normalise_use_path("build/../../helpers.lua"),
            Err(UsePathRejection::DotDotSegment)
        );
    }

    #[test]
    fn a_dotdot_inside_a_segment_is_not_a_dotdot_segment() {
        // `..foo.lua` is a legal file name, not an escape.
        assert_eq!(
            normalise_use_path("build/..foo.lua"),
            Ok("build/..foo.lua".to_string())
        );
    }

    #[test]
    fn a_path_that_elides_to_nothing_is_refused() {
        assert_eq!(normalise_use_path("./"), Err(UsePathRejection::Empty));
        assert_eq!(normalise_use_path(""), Err(UsePathRejection::Empty));
    }

    #[test]
    fn the_candidate_is_the_normalised_path_under_the_working_dir() {
        assert_eq!(
            module_path_candidate(Path::new("/proj"), "build/helpers.lua"),
            Path::new("/proj/build/helpers.lua")
        );
    }

    #[test]
    fn containment_follows_symlinks_and_refuses_what_it_cannot_resolve() {
        let tmp = std::env::temp_dir().join(format!(
            "cook-431-containment-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let inside = tmp.join("proj");
        let outside = tmp.join("elsewhere");
        std::fs::create_dir_all(inside.join("build")).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(inside.join("build/real.lua"), "return {}").unwrap();
        std::fs::write(outside.join("escaped.lua"), "return {}").unwrap();

        assert!(contains_resolved_module_path(
            &inside,
            &inside.join("build/real.lua")
        ));

        // A symlink inside the tree pointing out of it: `normalise_use_path`
        // sees a clean relative path and cannot know.
        #[cfg(unix)]
        {
            let link = inside.join("build/link.lua");
            std::os::unix::fs::symlink(outside.join("escaped.lua"), &link).unwrap();
            assert!(!contains_resolved_module_path(&inside, &link));
        }

        // Nothing there at all reports NOT contained: the permissive answer on
        // a rule whose failure mode is a silent wrong cache is the wrong one.
        assert!(!contains_resolved_module_path(
            &inside,
            &inside.join("build/absent.lua")
        ));

        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn the_escape_diagnostic_explains_the_cache_reason() {
        let msg = module_path_escaped_message(
            Path::new("/proj"),
            "build/link.lua",
            Path::new("/elsewhere/x.lua"),
        );
        assert!(msg.contains("build/link.lua"));
        assert!(msg.contains("/elsewhere/x.lua"));
        assert!(msg.contains("determinant"));
    }

    #[test]
    fn the_path_not_found_diagnostic_reports_one_file_not_a_candidate_list() {
        let msg = module_path_not_found_message(
            Path::new("/proj"),
            "./build/helpers.lua",
            Path::new("/proj/build/helpers.lua"),
        );
        assert_eq!(
            msg,
            "cook.load_module: module file './build/helpers.lua' not found at \
             /proj/build/helpers.lua (relative to /proj)"
        );
        assert!(!msg.contains("tried"));
    }
}

// ---------------------------------------------------------------------------
// Module-load laws
// ---------------------------------------------------------------------------

mod module_load_laws {
    use crate::layout::*;
    use std::path::Path;

    #[test]
    fn memo_key_is_cwd_and_name() {
        assert_eq!(
            module_memo_key(Path::new("/proj/sub"), "cook_cc"),
            "/proj/sub::cook_cc"
        );
    }

    /// §12.3.3: prefix, ` -> ` joining, re-entered name appended.
    #[test]
    fn cycle_message_renders_the_path() {
        let stack = vec!["a".to_string(), "b".to_string()];
        assert_eq!(
            module_cycle_message(&stack, "a"),
            "module cycle detected: a -> b -> a"
        );
    }

    #[test]
    fn cycle_message_self_cycle() {
        assert_eq!(
            module_cycle_message(&["solo".to_string()], "solo"),
            "module cycle detected: solo -> solo"
        );
    }

    /// §24.2: the diagnostic identifies the name and the searched paths.
    #[test]
    fn not_found_message_names_module_and_paths() {
        let msg = module_not_found_message(Path::new("/proj"), "foo");
        assert!(msg.starts_with(
            "cook.load_module: module 'foo' not found under /proj/.cook/modules \
             (tried share/lua/5.4/foo.lua, share/lua/5.4/foo/init.lua)"
        ), "{msg}");
    }

    /// CS-0207: the hard cut is affordable because the diagnostic does the one
    /// job a deprecation window would have done — telling the user what moved.
    /// A window would have had to keep the retired root in the search path for
    /// its whole life, which is the conflation the cut exists to delete.
    #[test]
    fn a_stale_package_directory_is_recognised_and_the_migration_named() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(LEGACY_MODULES_DIR)).unwrap();

        let msg = module_not_found_message(tmp.path(), "foo");
        assert!(msg.contains("still exists"), "{msg}");
        assert!(msg.contains("cook modules install"), "{msg}");
        // Both halves of the migration: reinstall the rocks, and move a module
        // you WROTE somewhere else and reach it by path.
        assert!(msg.contains("use ./path/to/it.lua"), "{msg}");
    }

    /// The other conditional clause, and the cost CS-0207 accepted by putting
    /// the tree under the build-output root: `rm -rf .cook` is an established
    /// habit and it now takes installed rocks with it.
    #[test]
    fn a_wiped_dot_cook_is_named_as_such() {
        let tmp = tempfile::tempdir().unwrap();
        let msg = module_not_found_message(tmp.path(), "foo");
        assert!(msg.contains("no module tree here"), "{msg}");
        assert!(msg.contains("cook modules install"), "{msg}");
        assert!(!msg.contains("still exists"), "{msg}");
    }

    #[test]
    fn read_failed_message_names_module_path_and_cause() {
        let msg =
            module_read_failed_message("foo", Path::new("/p/foo.lua"), "permission denied");
        assert_eq!(
            msg,
            "cook.load_module: failed to read module 'foo' at /p/foo.lua: permission denied"
        );
    }

    #[test]
    fn chunk_name_is_at_path() {
        assert_eq!(
            module_chunk_name(Path::new("/p/.cook/modules/share/lua/5.4/foo.lua")),
            "@/p/.cook/modules/share/lua/5.4/foo.lua"
        );
    }
}
