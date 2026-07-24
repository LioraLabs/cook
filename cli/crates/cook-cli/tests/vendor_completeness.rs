//! Vendor-completeness guard: every file inside the parts of the vendored
//! luarocks tree that `chore "bundle-luarocks"` actually copies into the
//! cook stage must be tracked by git.
//!
//! Why this exists: the project-wide .gitignore has `build/` and `bin/`
//! rules (intended for Cargo build dirs). These also match
//! `cli/vendored/luarocks-3.11.0/src/luarocks/build/` and
//! `cli/vendored/luarocks-3.11.0/src/bin/`, which silently dropped six
//! load-bearing files from the M2.3 vendor commit. The Linux gate
//! passed only because the implementer's local copy retained the files;
//! cloning to a fresh machine produced a broken bundle. SHI-176 Phase 2
//! M2.4 caught this on darwin-amd64.
//!
//! The fix added negation rules to .gitignore. This test ensures the
//! negation rules stay correct over time: any future `build/` or `bin/`
//! tightening that re-drops vendored files will fail this test before
//! the bundle reaches a fresh runner.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use walkdir::WalkDir;

const VENDORED_LUAROCKS_REL: &str = "../../vendored/luarocks-3.11.0";
// Subtrees that bundle-luarocks copies into the stage. Anything missing
// here breaks `~/.cook/share/luarocks/...` resolution at runtime.
const REQUIRED_SUBTREES: &[&str] = &["src/luarocks", "src/bin"];

fn vendor_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(VENDORED_LUAROCKS_REL)
        .canonicalize()
        .expect("vendor root canonicalize")
}

fn repo_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is .../cli/crates/cook-cli; repo root is three up.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("cli/crates/cook-cli has three ancestors to repo root")
        .canonicalize()
        .expect("repo root canonicalize")
}

fn git_tracked_files(repo: &Path, scope: &Path) -> BTreeSet<PathBuf> {
    let out = Command::new("git")
        .args(["ls-files", "-z"])
        .arg(scope)
        .current_dir(repo)
        .output()
        .expect("git ls-files");
    assert!(
        out.status.success(),
        "git ls-files failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    out.stdout
        .split(|b| *b == 0)
        .filter(|s| !s.is_empty())
        .map(|s| repo.join(std::str::from_utf8(s).expect("ls-files utf8")))
        .map(|p| p.canonicalize().unwrap_or(p))
        .collect()
}

#[test]
fn vendored_luarocks_runtime_subtrees_are_fully_tracked() {
    let vendor = vendor_root();
    let repo = repo_root();

    let mut missing: Vec<PathBuf> = Vec::new();
    for subtree in REQUIRED_SUBTREES {
        let scope = vendor.join(subtree);
        assert!(
            scope.is_dir(),
            "expected vendored subtree at {} — vendor refresh incomplete",
            scope.display()
        );

        let tracked = git_tracked_files(&repo, &scope);
        for entry in WalkDir::new(&scope).into_iter().filter_map(|e| e.ok()) {
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path().canonicalize().unwrap_or_else(|_| entry.into_path());
            if !tracked.contains(&path) {
                missing.push(path);
            }
        }
    }

    assert!(
        missing.is_empty(),
        "vendored luarocks files exist on disk but are not tracked by git \
         (probably because a project-wide .gitignore rule swallowed them — \
         add a negation rule under cli/vendored/luarocks-*/):\n  {}",
        missing
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join("\n  ")
    );
}
