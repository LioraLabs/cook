//! A store is untrusted at restore time (§17.2), and that covers the paths it
//! names, not only the bytes. A forged determinant manifest put to a live
//! `CacheServer` must not make a cold fetch write outside the workspace; and
//! the restore itself refuses such a path whatever named it — a declaration
//! or a local record, with no manifest in the way.

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use cook_cache::backend::{
    ArtifactMeta, CloudKeyInputs, DeterminantManifest, LocalBackend, artifact_key, cloud_key,
    put_bytes,
};
use cook_cache::cas_backend::BackendConfig;
use cook_cache::check::{RebuildResult, needs_rebuild_cook};
use cook_cache::cloud_backend::CloudBackend;
use cook_cache::serve::CacheServer;
use cook_cache::store::{CACHE_VERSION, FileRecord, StepEntry};
use cook_cache::{CacheBackend, RestoreCtx, fetch_by_key};

const NS: &str = "proj/Cookfile::build";

/// One escape: where it is named relative to the workspace, the artifact
/// stored for it, and how a test tells that it landed outside.
struct Escape {
    path: String,
    kind: Option<&'static str>,
    target: Option<&'static str>,
    landed: fn(outer: &Path) -> bool,
}

fn exists(p: &Path) -> bool {
    std::fs::symlink_metadata(p).is_ok()
}

/// `outer/ws` is the workspace; every escape aims at `outer`.
fn escapes(outer: &Path) -> Vec<Escape> {
    vec![
        Escape { path: "../escaped.txt".into(), kind: None, target: None, landed: |o| exists(&o.join("escaped.txt")) },
        Escape {
            path: outer.join("abs.txt").to_str().unwrap().into(),
            kind: None,
            target: None,
            landed: |o| exists(&o.join("abs.txt")),
        },
        Escape { path: "../made-dir".into(), kind: Some("dir"), target: None, landed: |o| exists(&o.join("made-dir")) },
        // The target resolves inside the workspace, so only the link's own
        // place is outside: the target check alone lets it through.
        Escape { path: "../link".into(), kind: Some("symlink"), target: Some("ws/in.c"), landed: |o| exists(&o.join("link")) },
        // Names the workspace itself: a dir artifact there would chmod it.
        Escape {
            path: String::new(),
            kind: Some("dir"),
            target: None,
            landed: |o| std::fs::metadata(o.join("ws")).unwrap().permissions().mode() & 0o777 != 0o700,
        },
    ]
}

const ESCAPES: usize = 5;

fn put_artifact(b: &dyn CacheBackend, key: &[u8; 32], idx: u32, path: &str, bytes: &[u8], kind: Option<&str>, target: Option<&str>) {
    let mut meta = ArtifactMeta {
        recipe_namespace: NS.into(),
        command_hash: 1,
        env_contribution: 2,
        seal_contribution: 3,
        schema_version: CACHE_VERSION,
        size_bytes: bytes.len() as u64,
        tags: Default::default(),
        consulted_env_keys: Default::default(),
        output_index: idx,
        output_path: path.into(),
        content_hash: ArtifactMeta::zero_content_hash(),
        kind: kind.map(str::to_string),
        mode: 0o777,
        target: target.map(str::to_string),
    };
    put_bytes(b, &artifact_key(key, idx, path), bytes, &mut meta).expect("seed artifact");
}

/// A fresh `outer/ws` holding the one input, and the unit's key.
fn workspace() -> (tempfile::TempDir, std::path::PathBuf, Vec<u64>, [u8; 32]) {
    let outer = tempfile::tempdir().unwrap();
    let ws = outer.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    std::fs::set_permissions(&ws, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::write(ws.join("in.c"), b"src").unwrap();
    let hashes = vec![xxhash_rust::xxh3::xxh3_64(b"src")];
    let k = cloud_key(&CloudKeyInputs {
        schema_version: CACHE_VERSION,
        recipe_namespace: NS,
        command_hash: 1,
        env_contribution: 2,
        seal_contribution: 3,
        sorted_input_content_hashes: &hashes,
    });
    (outer, ws, hashes, k)
}

fn manifest(k: &[u8; 32], output_paths: Vec<String>, empty_dir_outputs: Vec<String>) -> DeterminantManifest {
    DeterminantManifest {
        schema_version: CACHE_VERSION,
        recipe_namespace: NS.into(),
        key: hex::encode(k),
        command_hash: 1,
        env_contribution: 2,
        seal_contribution: 3,
        inputs: Default::default(),
        output_paths,
        empty_dir_outputs,
        consulted_env: Default::default(),
        sealed_probes: Default::default(),
        observation: None,
    }
}

/// A token holder puts the declared output plus one path the recipe never
/// declared, and a manifest naming both. The manifest is read as absent: the
/// declared output restores and nothing lands outside.
#[test]
fn a_forged_remote_manifest_writes_nothing_outside_the_workspace() {
    let store = tempfile::tempdir().unwrap();
    let server = Arc::new(CacheServer::bind("127.0.0.1:0", LocalBackend::new(store.path().into()), "tok".into()).unwrap());
    let s2 = server.clone();
    let t = std::thread::spawn(move || s2.run());
    let cfg = BackendConfig {
        timeout: Duration::from_secs(5),
        max_retries: 0,
        backoff_initial: Duration::from_millis(1),
        backoff_max: Duration::from_millis(2),
        max_artifact_bytes: 1 << 20,
    };
    let client = CloudBackend::new(format!("http://{}", server.local_addr()), "tok".into(), cfg);

    // Read everything first, then stop the server, then judge: a failed
    // assertion must not leave the server thread running.
    let mut seen = Vec::new();
    for as_empty_dir in [false, true] {
        for i in 0..ESCAPES {
            let (outer, ws, hashes, k) = workspace();
            let e = escapes(outer.path()).remove(i);
            if as_empty_dir && e.kind != Some("dir") {
                continue;
            }
            put_artifact(&client, &k, 0, "out.o", b"obj", None, None);
            put_artifact(&client, &k, 1, &e.path, b"pwned", e.kind, e.target);
            let m = if as_empty_dir {
                manifest(&k, vec!["out.o".into()], vec![e.path.clone()])
            } else {
                manifest(&k, vec!["out.o".into(), e.path.clone()], vec![])
            };
            client.put_manifest(&k, &m).unwrap();

            let ctx = RestoreCtx { backend: &client, recipe_namespace: NS, declared_outputs: &["out.o", "**"] };
            // Declared as broadly as a unit can, so only the escape decides.
            let got = fetch_by_key(&ctx, 1, 2, 3, &hashes, &["out.o"], &ws, None, false);
            seen.push((
                e.path.clone(),
                as_empty_dir,
                got.map(|o| o.restored_outputs),
                std::fs::read(ws.join("out.o")).ok(),
                (e.landed)(outer.path()),
            ));
        }
    }
    server.shutdown();
    t.join().unwrap();

    assert_eq!(seen.len(), ESCAPES + 2, "every escape as an output, each dir one as an empty dir too");
    for (path, as_empty_dir, restored, out, landed) in seen {
        assert!(!landed, "{path:?} (empty dir: {as_empty_dir}) landed outside the workspace");
        assert_eq!(restored, Some(vec!["out.o".to_string()]), "{path:?}: the manifest must read as absent");
        assert_eq!(out.as_deref(), Some(&b"obj"[..]), "{path:?}: the declared output still restores");
    }
}

/// The restore refuses an escaping path a declaration names, with no manifest
/// to stop it first: a cold fetch is a miss and nothing lands outside.
#[test]
fn a_cold_restore_refuses_a_declared_path_outside_the_workspace() {
    for i in 0..ESCAPES {
        let (outer, ws, hashes, k) = workspace();
        let e = escapes(outer.path()).remove(i);
        let store = tempfile::tempdir().unwrap();
        let backend = LocalBackend::new(store.path().into());
        put_artifact(&backend, &k, 0, &e.path, b"pwned", e.kind, e.target);
        let ctx = RestoreCtx { backend: &backend, recipe_namespace: NS, declared_outputs: &[e.path.as_str()] };
        let got = fetch_by_key(&ctx, 1, 2, 3, &hashes, &[e.path.as_str()], &ws, None, false);
        assert!(got.is_none(), "{:?}: an escaping path is a miss", e.path);
        assert!(!(e.landed)(outer.path()), "{:?} landed outside the workspace", e.path);
    }
}

/// The same refusal on the warm path: a local record whose output is missing
/// asks for a restore of a path outside the workspace, and gets a rebuild.
#[test]
fn a_warm_restore_refuses_a_recorded_path_outside_the_workspace() {
    let (outer, ws, hashes, k) = workspace();
    let store = tempfile::tempdir().unwrap();
    let backend = LocalBackend::new(store.path().into());
    let path = "../escaped.txt";
    put_artifact(&backend, &k, 0, path, b"pwned", None, None);
    let entry = StepEntry {
        inputs: vec![FileRecord {
            identity: None,
            path: "in.c".into(),
            mtime: cook_cache::stat_mtime(&ws.join("in.c")).unwrap(),
            hash: hashes[0],
        }],
        outputs: vec![FileRecord {
            identity: None,
            path: path.into(),
            mtime: 0,
            hash: xxhash_rust::xxh3::xxh3_64(b"pwned"),
        }],
        command_hash: 1,
        env_contribution: 2,
        seal_contribution: 3,
        observed: None,
        module_inputs: Vec::new(),
    };
    let ctx = RestoreCtx { backend: &backend, recipe_namespace: NS, declared_outputs: &[path] };
    let (result, _) = needs_rebuild_cook(Some(&entry), &["in.c"], &[path], 1, 2, 3, &ws, Some(&ctx), None, false);
    assert!(matches!(result, RebuildResult::Rebuild(_)), "got {result:?}");
    assert!(!exists(&outer.path().join("escaped.txt")), "the record's path landed outside");
}

/// A cold fetch of `outputs` (path, kind, target), seeded in a fresh local
/// store at their index, with no manifest.
fn cold(ws: &Path, hashes: &[u64], k: &[u8; 32], outputs: &[(&str, Option<&str>, Option<&str>)]) -> Option<Vec<String>> {
    let store = tempfile::tempdir().unwrap();
    let backend = LocalBackend::new(store.path().into());
    for (i, (path, kind, target)) in outputs.iter().enumerate() {
        put_artifact(&backend, k, i as u32, path, b"pwned", *kind, *target);
    }
    let paths: Vec<&str> = outputs.iter().map(|o| o.0).collect();
    let ctx = RestoreCtx { backend: &backend, recipe_namespace: NS, declared_outputs: &paths };
    fetch_by_key(&ctx, 1, 2, 3, hashes, &paths, ws, None, false).map(|o| o.restored_outputs)
}

/// Every link stays inside on its own — `y` is `.`, `x` is `y/..` — but `x`
/// is the workspace's parent once `y` is followed, so `x/evil` is outside.
/// In either order of the two links.
#[test]
fn a_chain_of_links_does_not_carry_a_restore_outside() {
    for order in [["y", "x"], ["x", "y"]] {
        let (outer, ws, hashes, k) = workspace();
        let link = |name: &'static str| (name, Some("symlink"), Some(if name == "y" { "." } else { "y/.." }));
        let got = cold(&ws, &hashes, &k, &[link(order[0]), link(order[1]), ("x/evil", None, None)]);
        assert!(got.is_none(), "{order:?}: the chain is a miss");
        assert!(!exists(&outer.path().join("evil")), "{order:?}: x/evil landed outside the workspace");
        // Nor is a link left behind that leads out for the next writer.
        let real_ws = ws.canonicalize().unwrap();
        assert!(
            ws.join("x").canonicalize().map_or(true, |r| r.starts_with(&real_ws) && r != real_ws.parent().unwrap()),
            "{order:?}: x was left pointing out of the workspace"
        );
    }
}

/// A link the workspace already holds (a package manager's, say) points out:
/// nothing is written through it, and no dir artifact chmods its target.
#[test]
fn a_link_already_in_the_workspace_does_not_carry_a_restore_outside() {
    // `nm/../evil` folds to `evil` by name; the kernel follows `nm` first.
    for (path, kind) in [("nm/evil", None), ("nm/../evil", None), ("nm", Some("dir")), ("nm/.", Some("dir"))] {
        let (outer, ws, hashes, k) = workspace();
        let real = outer.path().join("real");
        std::fs::create_dir(&real).unwrap();
        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::os::unix::fs::symlink(&real, ws.join("nm")).unwrap();
        let got = cold(&ws, &hashes, &k, &[(path, kind, None)]);
        assert!(got.is_none(), "{path:?}: a miss");
        assert!(!exists(&real.join("evil")), "{path:?}: written through the link");
        assert!(!exists(&outer.path().join("evil")), "{path:?}: written beside the link's target");
        let mode = std::fs::metadata(&real).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "{path:?}: the link's target was chmodded");
    }
}

/// Inside the workspace but outside what the unit declares: a forged manifest
/// naming a git hook (mode 0o777 — it would run on the next commit) or the
/// Cookfile is read as absent, and the declared output restores alone. A
/// declared glob still admits what it matches.
#[test]
fn a_forged_manifest_writes_nothing_the_unit_does_not_declare() {
    let (outer, ws, hashes, k) = workspace();
    std::fs::write(ws.join("Cookfile"), b"mine").unwrap();
    let store = tempfile::tempdir().unwrap();
    let backend = LocalBackend::new(store.path().into());
    let ctx = RestoreCtx { backend: &backend, recipe_namespace: NS, declared_outputs: &["dist/*.js"] };
    put_artifact(&backend, &k, 0, "dist/app.js", b"obj", None, None);
    for (i, path) in [".git/hooks/pre-commit", "Cookfile"].into_iter().enumerate() {
        put_artifact(&backend, &k, 1 + i as u32, path, b"pwned", None, None);
        backend.put_manifest(&k, &manifest(&k, vec!["dist/app.js".into(), path.into()], vec![])).unwrap();
        let got = fetch_by_key(&ctx, 1, 2, 3, &hashes, &["dist/app.js"], &ws, None, false);
        assert_eq!(got.map(|o| o.restored_outputs), Some(vec!["dist/app.js".to_string()]), "{path:?}: the manifest must read as absent");
    }
    assert!(!exists(&ws.join(".git")), "a hook was written");
    assert_eq!(std::fs::read(ws.join("Cookfile")).unwrap(), b"mine", "the Cookfile was overwritten");
    // The glob admits a name the caller did not know: the manifest is honoured.
    put_artifact(&backend, &k, 1, "dist/chunk-9f.js", b"chunk", None, None);
    backend.put_manifest(&k, &manifest(&k, vec!["dist/app.js".into(), "dist/chunk-9f.js".into()], vec![])).unwrap();
    let got = fetch_by_key(&ctx, 1, 2, 3, &hashes, &["dist/app.js"], &ws, None, false);
    assert_eq!(got.map(|o| o.restored_outputs.len()), Some(2));
    assert_eq!(std::fs::read(ws.join("dist/chunk-9f.js")).unwrap(), b"chunk");
    drop(outer);
}

/// Inside a declared directory or wildcard, a forged manifest still cannot
/// reach the rest of the workspace: not by a `..` the pattern happens to cover,
/// not through a link it restored first (`out/l` → `..`), and not through a
/// link planted at the temp name the next file is staged under.
#[test]
fn a_forged_manifest_cannot_leave_a_declared_directory() {
    type Out = (&'static str, Option<&'static str>, Option<&'static str>);
    let cases: [(&str, Vec<Out>); 6] = [
        ("out/", vec![("out/a", None, None), ("out/../Cookfile", None, None)]),
        ("out/**", vec![("out/a", None, None), ("out/../Cookfile", None, None)]),
        ("out/*/Cookfile", vec![("out/../Cookfile", None, None)]),
        ("out/", vec![("out/l", Some("symlink"), Some("..")), ("out/l/Cookfile", None, None)]),
        ("out/", vec![("out/l", Some("symlink"), Some("..")), ("out/l/.git/hooks/pre-commit", None, None)]),
        ("out/", vec![("out/a.cook.tmp", Some("symlink"), Some("../Cookfile")), ("out/a", None, None)]),
    ];
    for (declared, outs) in cases {
        let (outer, ws, hashes, k) = workspace();
        std::fs::write(ws.join("Cookfile"), b"mine").unwrap();
        let store = tempfile::tempdir().unwrap();
        let backend = LocalBackend::new(store.path().into());
        for (i, (p, kind, t)) in outs.iter().enumerate() {
            put_artifact(&backend, &k, i as u32, p, b"pwned", *kind, *t);
        }
        backend.put_manifest(&k, &manifest(&k, outs.iter().map(|o| o.0.to_string()).collect(), vec![])).unwrap();
        let ctx = RestoreCtx { backend: &backend, recipe_namespace: NS, declared_outputs: &[declared] };
        let got = fetch_by_key(&ctx, 1, 2, 3, &hashes, &["out/a"], &ws, None, false);
        let names: Vec<&str> = outs.iter().map(|o| o.0).collect();
        assert_eq!(std::fs::read(ws.join("Cookfile")).unwrap(), b"mine", "{declared} {names:?}: the Cookfile was overwritten");
        assert!(!exists(&ws.join(".git")), "{declared} {names:?}: a hook was written");
        if let Some(o) = got {
            assert!(o.restored_outputs.iter().all(|p| !p.contains("..") && !p.starts_with("out/l/")), "{declared}: recorded {:?}", o.restored_outputs);
        }
        drop(outer);
    }
}

/// A cold fetch of `outs` under `declared`, from a store holding them and a
/// manifest naming them (`empties` as empty dirs); the workspace also holds
/// `src/main.c` (dir mode 0o755), a Cookfile and `.git/objects/pack`.
fn forge(declared: &str, outs: &[(&str, Option<&str>, Option<&str>)], empties: &[&str]) -> (tempfile::TempDir, std::path::PathBuf, Option<Vec<String>>) {
    let (outer, ws, hashes, k) = workspace();
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(ws.join("src/main.c"), b"mine").unwrap();
    std::fs::set_permissions(ws.join("src"), std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(ws.join("Cookfile"), b"mine").unwrap();
    std::fs::create_dir_all(ws.join(".git/objects")).unwrap();
    std::fs::write(ws.join(".git/objects/pack"), b"history").unwrap();
    let store = tempfile::tempdir().unwrap();
    let backend = LocalBackend::new(store.path().into());
    for (i, (p, kind, t)) in outs.iter().enumerate() {
        put_artifact(&backend, &k, i as u32, p, b"pwned", *kind, *t);
    }
    let files = outs.iter().map(|o| o.0).filter(|p| !empties.contains(p)).map(str::to_string).collect();
    backend.put_manifest(&k, &manifest(&k, files, empties.iter().map(|e| e.to_string()).collect())).unwrap();
    let ctx = RestoreCtx { backend: &backend, recipe_namespace: NS, declared_outputs: &[declared] };
    let got = fetch_by_key(&ctx, 1, 2, 3, &hashes, &[outs[0].0], &ws, None, false).map(|o| o.restored_outputs);
    (outer, ws, got)
}

fn workspace_intact(ws: &Path) -> bool {
    std::fs::read(ws.join("Cookfile")).is_ok_and(|b| b == b"mine")
        && std::fs::read(ws.join("src/main.c")).is_ok_and(|b| b == b"mine")
        && exists(&ws.join(".git/objects/pack"))
        && exists(&ws.join("in.c"))
}

/// A link in a declared directory restores only pointing at what the unit
/// declares: one at `..`, `.`'s own directory, `../src` or `../.git/hooks`
/// would hand the workspace to the next walk of `out/` (a reconcile deletes
/// through it, a record uploads through it). One pointing at a sibling output
/// restores.
#[test]
fn a_restored_link_points_only_at_a_declared_output() {
    for target in ["..", ".", "../src", "../.git/hooks", "../Cookfile"] {
        let (_outer, ws, got) = forge("out/", &[("out/a", None, None), ("out/h", Some("symlink"), Some(target))], &[]);
        assert_eq!(got, None, "out/h -> {target}: a miss");
        assert!(!exists(&ws.join("out/h")), "out/h -> {target} was restored");
        assert!(workspace_intact(&ws), "out/h -> {target}");
    }
    // Covered, but a link to a directory — its own, one above, or links that
    // reach each other in any order — loops every walk that follows links.
    type Out = (&'static str, Option<&'static str>, Option<&'static str>);
    let l = |p, t| (p, Some("symlink"), Some(t));
    let loops: [Vec<Out>; 6] = [
        vec![("out/k/f", None, None), l("out/k/y", ".")],
        vec![("out/k/f", None, None), l("out/k/y", "..")],
        vec![("out/sub/f", None, None), l("out/l", "sub")],
        vec![("out/a/f", None, None), ("out/b/g", None, None), l("out/a/x", "../b"), l("out/b/y", "../a")],
        vec![l("out/a/x", "../b"), l("out/b/y", "../a"), ("out/a/f", None, None), ("out/b/g", None, None)],
        vec![("out/a/f", None, None), l("out/a/x", "../l"), l("out/l", "a")],
    ];
    for outs in loops {
        let (_outer, ws, got) = forge("out/", &outs, &[]);
        assert_eq!(got, None, "{outs:?}: a miss");
        for (p, kind, _) in &outs {
            assert!(kind.is_none() || !exists(&ws.join(p)), "{outs:?}: {p} was left behind");
        }
        let walked = cook_cache::resolve_glob(&ws, "out/**/*").len();
        assert!(walked <= outs.len(), "{outs:?}: a link-following walk saw {walked} entries");
    }
    let (_outer, ws, got) = forge("out/", &[("out/a", None, None), ("out/l", Some("symlink"), Some("a"))], &[]);
    assert_eq!(got, Some(vec!["out/a".to_string(), "out/l".to_string()]));
    assert_eq!(std::fs::read_link(ws.join("out/l")).unwrap(), Path::new("a"));
}

/// The warm path asks the store for the artifact's kind and target, so a
/// store that turns a recorded file into a link to `..` is held to the same
/// rule: a rebuild, no link.
#[test]
fn a_warm_restore_refuses_a_link_the_store_swapped_in() {
    let (_outer, ws, hashes, k) = workspace();
    let store = tempfile::tempdir().unwrap();
    let backend = LocalBackend::new(store.path().into());
    put_artifact(&backend, &k, 0, "out/a", b"", Some("symlink"), Some(".."));
    let entry = StepEntry {
        inputs: vec![FileRecord { identity: None, path: "in.c".into(), mtime: cook_cache::stat_mtime(&ws.join("in.c")).unwrap(), hash: hashes[0] }],
        outputs: vec![FileRecord { identity: None, path: "out/a".into(), mtime: 0, hash: xxhash_rust::xxh3::xxh3_64(b"obj") }],
        command_hash: 1,
        env_contribution: 2,
        seal_contribution: 3,
        observed: None,
        module_inputs: Vec::new(),
    };
    let ctx = RestoreCtx { backend: &backend, recipe_namespace: NS, declared_outputs: &["out/"] };
    let (result, _) = needs_rebuild_cook(Some(&entry), &["in.c"], &["out/a"], 1, 2, 3, &ws, Some(&ctx), None, false);
    assert!(matches!(result, RebuildResult::Rebuild(_)), "got {result:?}");
    assert!(!exists(&ws.join("out/a")), "the swapped-in link was restored");
}

/// Names a declaration covers only by accident are refused: coverage is
/// case-sensitive as the output walk is (`OUT/x` under `out/`); a `.` segment
/// (`src/.` is `src` itself, which a dir artifact would chmod; `out/./Cookfile`)
/// and a `.git` segment under a root `*` or `**` read the manifest as absent.
#[test]
fn a_forged_manifest_names_nothing_a_declaration_covers_by_accident() {
    type Out = (&'static str, Option<&'static str>, Option<&'static str>);
    let cases: [(&str, Vec<Out>, Vec<&str>); 6] = [
        ("out/", vec![("out/a", None, None), ("OUT/x", None, None)], vec![]),
        ("src/*", vec![("src/gen.c", None, None), ("src/.", Some("dir"), None)], vec!["src/."]),
        ("out/**", vec![("out/a", None, None), ("out/./Cookfile", None, None)], vec![]),
        ("*", vec![("a.o", None, None), (".git", Some("dir"), None)], vec![".git"]),
        ("*", vec![("a.o", None, None), (".GIT", Some("dir"), None)], vec![".GIT"]),
        ("**", vec![("a.o", None, None), (".git/hooks/pre-commit", None, None)], vec![]),
    ];
    for (declared, outs, empties) in cases {
        let (_outer, ws, got) = forge(declared, &outs, &empties);
        assert_eq!(got, Some(vec![outs[0].0.to_string()]), "{declared} {outs:?}: the manifest must read as absent");
        assert!(!exists(&ws.join("OUT")), "{declared}: OUT/x was written");
        assert!(!exists(&ws.join(".git/hooks")) && !exists(&ws.join(".GIT")), "{declared}: .git was written");
        let mode = std::fs::metadata(ws.join("src")).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o755, "{declared}: src was chmodded");
        assert!(workspace_intact(&ws), "{declared}");
    }
}

/// A link already in a declared directory — planted, or an honest build's
/// `out/h -> ..` — is an entry of the directory, not a way into the rest of
/// the workspace: reconciling `out/` removes the link and nothing it reaches,
/// and the output walk lists the link and nothing under it.
#[test]
fn reconciling_a_directory_output_never_goes_through_a_link() {
    for target in ["..", "../src"] {
        let (_outer, ws, got) = forge("out/", &[("out/a", None, None)], &[]);
        assert_eq!(got, Some(vec!["out/a".to_string()]));
        std::os::unix::fs::symlink(target, ws.join("out/h")).unwrap();
        assert_eq!(
            cook_cache::resolve_output_glob(&ws, "out/**/*"),
            ["out/a", "out/h"].map(String::from).into(),
            "out/h -> {target}"
        );
        cook_cache::reconcile_dir_output(&ws, "out", &["out/a".to_string()].into());
        assert!(workspace_intact(&ws), "out/h -> {target}: removed through the link");
        assert!(!exists(&ws.join("out/h")), "out/h -> {target}: the stray link stays");
        assert!(exists(&ws.join("out/a")));
    }
}
