//! Parse a streamed `.tar.gz` registry archive into an [`ArchivePlan`].
//!
//! Forge tarballs (Gitea, GitHub) prefix every entry with a single top-level
//! `<repo>-<sha>/` directory. We strip that prefix transparently. The remaining
//! entries we care about are rooted under `modules/<name>/`; everything else is
//! ignored.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use flate2::read::GzDecoder;
use tar::Archive;

use super::errors::PullError;

/// One file slated for installation under `cook_modules/<name>/`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveEntry {
    /// Path relative to the module root, e.g. `init.lua` or `helpers/foo.lua`.
    pub rel_path: PathBuf,
    /// File contents, fully buffered. Module files are small (KB).
    pub contents: Vec<u8>,
}

/// All modules found under `modules/` in the archive, keyed by module name.
/// `BTreeMap` for deterministic iteration per project convention.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ArchivePlan {
    pub modules: BTreeMap<String, Vec<ArchiveEntry>>,
}

impl ArchivePlan {
    pub fn module_names(&self) -> Vec<String> {
        self.modules.keys().cloned().collect()
    }
}

/// Parse a streamed `.tar.gz` into an `ArchivePlan`.
pub fn parse_archive<R: Read>(reader: R) -> Result<ArchivePlan, PullError> {
    let gz = GzDecoder::new(reader);
    let mut tar = Archive::new(gz);
    let mut plan = ArchivePlan::default();

    let entries = tar.entries().map_err(|e| PullError::BadArchive {
        reason: format!("cannot read tar entries: {e}"),
    })?;

    for entry in entries {
        let mut entry = entry.map_err(|e| PullError::BadArchive {
            reason: format!("cannot read entry: {e}"),
        })?;

        // Reject anything that isn't a regular file or directory outright.
        let etype = entry.header().entry_type();
        if etype.is_symlink() || etype.is_hard_link() {
            let path = entry
                .path()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| "<unknown>".to_string());
            return Err(PullError::BadArchive {
                reason: format!("archive contains link entry: {path}"),
            });
        }
        // `is_file()` is strict (Regular only). `is_contiguous()` covers POSIX
        // typeflag 7 entries that some tools (e.g. `git archive`) emit and which
        // are semantically regular files.
        if !(etype.is_file() || etype.is_contiguous()) {
            // Directories etc. — skip silently. Directories are recreated implicitly.
            continue;
        }

        let path_in_tar = entry
            .path()
            .map_err(|e| PullError::BadArchive {
                reason: format!("entry has non-utf8 path: {e}"),
            })?
            .into_owned();

        // Reject path traversal and absolute paths on the raw tar path. Doing this
        // before stripping the forge prefix means a path like `../etc/passwd` is
        // rejected directly rather than relying on downstream filtering.
        for comp in path_in_tar.components() {
            match comp {
                Component::Normal(_) => {}
                _ => {
                    return Err(PullError::BadArchive {
                        reason: format!(
                            "rejected non-normal path component in '{}'",
                            path_in_tar.display()
                        ),
                    });
                }
            }
        }

        // Strip any single leading <repo>-<sha>/ prefix that forge tarballs add.
        let stripped = strip_forge_prefix(&path_in_tar);

        // Match modules/<name>/<rest...>.
        let (name, rel_path) = match split_module_path(&stripped) {
            Some(parts) => parts,
            None => continue, // top-level files (README, LICENSE) and other dirs
        };

        let mut contents = Vec::new();
        entry
            .read_to_end(&mut contents)
            .map_err(|e| PullError::BadArchive {
                reason: format!("cannot read entry body for {}: {e}", stripped.display()),
            })?;

        plan.modules
            .entry(name)
            .or_default()
            .push(ArchiveEntry { rel_path, contents });
    }

    // Empty modules (a directory with no files) won't appear in `modules` because
    // we only insert on a file entry. That matches the spec's "empty modules =
    // not present" rule.

    Ok(plan)
}

/// Strip the single forge `<repo>-<sha>/` prefix if present.
///
/// Forge tarballs always wrap their content in one top-level dir. If the path
/// has no `/`, leave it alone. Otherwise drop the first component.
fn strip_forge_prefix(path: &Path) -> PathBuf {
    let mut comps = path.components();
    let first = comps.next();
    if first.is_none() {
        return PathBuf::new();
    }
    let rest: PathBuf = comps.collect();
    if rest.as_os_str().is_empty() {
        // Single-component path like "registry-abc/"; nothing left after strip.
        PathBuf::new()
    } else {
        rest
    }
}

/// If `path` starts with `modules/<name>/<rest...>`, return (name, rest).
/// Otherwise None.
fn split_module_path(path: &Path) -> Option<(String, PathBuf)> {
    let mut comps = path.components();
    match comps.next()? {
        Component::Normal(s) if s == "modules" => {}
        _ => return None,
    }
    let name = match comps.next()? {
        Component::Normal(s) => s.to_str()?.to_string(),
        _ => return None,
    };
    let rest: PathBuf = comps.collect();
    if rest.as_os_str().is_empty() {
        return None; // bare modules/<name>/ directory; no file
    }
    Some((name, rest))
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use std::io::Write;
    use tar::{Builder, Header};

    /// Test helper: build an in-memory `.tar.gz` from (path, contents) pairs.
    fn make_archive(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let buf = Vec::new();
        let gz = GzEncoder::new(buf, Compression::default());
        let mut tar = Builder::new(gz);
        for (path, body) in entries {
            let mut header = Header::new_gnu();
            header.set_path(path).unwrap();
            header.set_size(body.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            tar.append(&header, *body).unwrap();
        }
        tar.into_inner().unwrap().finish().unwrap()
    }

    /// Test helper: build a tar.gz that includes a symlink entry.
    fn make_archive_with_symlink(target: &str, link_name: &str) -> Vec<u8> {
        let buf = Vec::new();
        let gz = GzEncoder::new(buf, Compression::default());
        let mut tar = Builder::new(gz);
        let mut header = Header::new_gnu();
        header.set_entry_type(tar::EntryType::Symlink);
        header.set_path(link_name).unwrap();
        header.set_link_name(target).unwrap();
        header.set_size(0);
        header.set_mode(0o777);
        header.set_cksum();
        tar.append(&header, std::io::empty()).unwrap();
        tar.into_inner().unwrap().finish().unwrap()
    }

    #[test]
    fn extracts_two_modules() {
        let archive = make_archive(&[
            ("registry-abc/modules/cpp/init.lua", b"-- cpp"),
            ("registry-abc/modules/cpp/helpers.lua", b"-- helpers"),
            ("registry-abc/modules/rust/init.lua", b"-- rust"),
        ]);
        let plan = parse_archive(&archive[..]).unwrap();
        assert_eq!(plan.module_names(), vec!["cpp", "rust"]);
        assert_eq!(plan.modules["cpp"].len(), 2);
        assert_eq!(plan.modules["rust"].len(), 1);

        let cpp_init = plan.modules["cpp"]
            .iter()
            .find(|e| e.rel_path == PathBuf::from("init.lua"))
            .unwrap();
        assert_eq!(cpp_init.contents, b"-- cpp");
    }

    #[test]
    fn skips_top_level_non_modules_paths() {
        let archive = make_archive(&[
            ("registry-abc/README.md", b"# registry"),
            ("registry-abc/LICENSE", b"MIT"),
            ("registry-abc/modules/cpp/init.lua", b"-- cpp"),
        ]);
        let plan = parse_archive(&archive[..]).unwrap();
        assert_eq!(plan.module_names(), vec!["cpp"]);
    }

    #[test]
    fn rejects_symlink_entry() {
        let archive = make_archive_with_symlink("/etc/passwd", "registry-abc/modules/evil/link");
        let err = parse_archive(&archive[..]).unwrap_err();
        match err {
            PullError::BadArchive { reason } => {
                assert!(reason.contains("link entry"), "got: {reason}");
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn rejects_path_traversal() {
        // The tar crate's set_path rejects `..` components on most platforms, so
        // we craft the 512-byte POSIX header by hand, writing the traversal path
        // directly into the name field, then gzip the result.  This tests our
        // parser's defense-in-depth independently of the tar builder's own checks.
        let path = b"registry-abc/modules/evil/../escape.lua";
        let mut header = [0u8; 512];
        header[..path.len()].copy_from_slice(path);
        // mode "0000644\0"
        header[100..108].copy_from_slice(b"0000644\x00");
        // size "00000000000\0"
        header[124..136].copy_from_slice(b"00000000000\x00");
        // mtime
        header[136..148].copy_from_slice(b"00000000000\x00");
        // typeflag '0' = regular file
        header[156] = b'0';
        // GNU magic
        header[257..265].copy_from_slice(b"ustar  \x00");
        // checksum placeholder
        header[148..156].copy_from_slice(b"        ");
        let cksum: u32 = header.iter().map(|&b| b as u32).sum();
        let cksum_str = format!("{:06o}\x00 ", cksum);
        header[148..156].copy_from_slice(cksum_str.as_bytes());

        // end-of-archive = two zero blocks
        let mut tar_bytes = Vec::new();
        tar_bytes.extend_from_slice(&header);
        tar_bytes.extend_from_slice(&[0u8; 1024]);

        let mut gz_buf = Vec::new();
        {
            let mut gz = GzEncoder::new(&mut gz_buf, Compression::default());
            gz.write_all(&tar_bytes).unwrap();
        }

        let err = parse_archive(&gz_buf[..]).unwrap_err();
        match err {
            PullError::BadArchive { reason } => {
                assert!(
                    reason.contains("non-normal path component"),
                    "expected component-rejection reason, got: {reason}"
                );
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn empty_archive_yields_empty_plan() {
        let archive = make_archive(&[]);
        let plan = parse_archive(&archive[..]).unwrap();
        assert!(plan.modules.is_empty());
    }

    #[test]
    fn nested_paths_preserved_under_module() {
        let archive = make_archive(&[
            ("registry-abc/modules/cpp/init.lua", b"-- cpp"),
            ("registry-abc/modules/cpp/lib/inner.lua", b"-- inner"),
            (
                "registry-abc/modules/cpp/lib/sub/deep.lua",
                b"-- deep",
            ),
        ]);
        let plan = parse_archive(&archive[..]).unwrap();
        let cpp = &plan.modules["cpp"];
        assert_eq!(cpp.len(), 3);
        let paths: Vec<&Path> = cpp.iter().map(|e| e.rel_path.as_path()).collect();
        assert!(paths.contains(&Path::new("init.lua")));
        assert!(paths.contains(&Path::new("lib/inner.lua")));
        assert!(paths.contains(&Path::new("lib/sub/deep.lua")));
    }

    #[test]
    fn malformed_gzip_is_bad_archive() {
        let not_gzip = b"this is plain text, not gzip";
        let err = parse_archive(&not_gzip[..]).unwrap_err();
        assert!(matches!(err, PullError::BadArchive { .. }));
    }
}
