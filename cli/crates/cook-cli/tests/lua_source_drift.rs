//! Drift guard: cli/vendored/lua-5.4.7/ must match lua-src 547.0.0's source tree
//! exactly. mlua statically links lua-src; if the vendored copy diverges, C rocks
//! compiled against the bundled headers will be ABI-incompatible with cook.
//!
//! The drift guard skips files cook vendors that aren't in lua-src (the lua-src
//! crate strips the standalone-interpreter `lua.c` and `luac` driver `luac.c`).
//! Cook fetches those from the upstream Lua 5.4.7 release tarball so the
//! bundled `bin/lua` and `bin/luac` artifacts can be built. They're version-
//! pinned to Lua 5.4.7 — same release lua-src embeds — so ABI alignment holds.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use cargo_metadata::MetadataCommand;
use sha2::{Digest, Sha256};
use walkdir::WalkDir;

const LUA_SRC_NAME: &str = "lua-src";
const LUA_SRC_VERSION: &str = "547.0.0";
const VENDORED_REL: &str = "../../vendored/lua-5.4.7";
// Sub-directory of the lua-src crate root containing the Lua 5.4.7 sources.
// (The crate also ships 5.1/5.2/5.3 trees; we only mirror 5.4.7.)
const LUA_SRC_SUBDIR: &str = "lua-5.4.7";

fn skipped_files() -> BTreeSet<&'static str> {
    // README.md is cook-authored provenance, not upstream source.
    // lua.c / luac.c are sourced from the upstream Lua 5.4.7 tarball because
    // lua-src strips them (the crate only embeds the library, not the
    // standalone interpreter / compiler drivers).
    let mut s = BTreeSet::new();
    s.insert("README.md");
    s.insert("lua.c");
    s.insert("luac.c");
    s
}

fn manifest(root: &Path) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let skip = skipped_files();
    for entry in WalkDir::new(root).into_iter().filter_map(|e| e.ok()) {
        if !entry.file_type().is_file() {
            continue;
        }
        let rel = entry
            .path()
            .strip_prefix(root)
            .expect("walkdir entry under root")
            .to_string_lossy()
            .replace('\\', "/");
        if skip.contains(rel.as_str()) {
            continue;
        }
        let bytes = std::fs::read(entry.path())
            .unwrap_or_else(|e| panic!("read {}: {}", entry.path().display(), e));
        let mut h = Sha256::new();
        h.update(&bytes);
        out.insert(rel, format!("{:x}", h.finalize()));
    }
    out
}

fn lua_src_lua_dir() -> PathBuf {
    let meta = MetadataCommand::new()
        .manifest_path("Cargo.toml")
        .exec()
        .expect("cargo metadata failed");
    let pkg = meta
        .packages
        .iter()
        .find(|p| p.name.as_str() == LUA_SRC_NAME && p.version.to_string() == LUA_SRC_VERSION)
        .unwrap_or_else(|| {
            panic!(
                "lua-src {} not in dependency graph; cook expects mlua to pin it",
                LUA_SRC_VERSION
            )
        });
    pkg.manifest_path
        .parent()
        .expect("manifest has parent")
        .join(LUA_SRC_SUBDIR)
        .into_std_path_buf()
}

#[test]
fn vendored_lua_matches_lua_src_crate() {
    let vendored = Path::new(env!("CARGO_MANIFEST_DIR")).join(VENDORED_REL);
    assert!(
        vendored.is_dir(),
        "vendored Lua dir missing at {} — copy from lua-src {}",
        vendored.display(),
        LUA_SRC_VERSION
    );
    let upstream = lua_src_lua_dir();
    assert!(
        upstream.is_dir(),
        "lua-src crate source dir missing at {} — cargo cache may be stale",
        upstream.display()
    );

    let v_man = manifest(&vendored);
    let u_man = manifest(&upstream);

    let mut diffs: Vec<String> = Vec::new();
    for (k, vh) in &v_man {
        match u_man.get(k) {
            None => diffs.push(format!("only in vendored: {}", k)),
            Some(uh) if uh != vh => diffs.push(format!("hash differs: {}", k)),
            _ => {}
        }
    }
    for k in u_man.keys() {
        if !v_man.contains_key(k) {
            diffs.push(format!("only in lua-src: {}", k));
        }
    }
    assert!(
        diffs.is_empty(),
        "drift detected vs lua-src {}:\n  {}",
        LUA_SRC_VERSION,
        diffs.join("\n  ")
    );
}
