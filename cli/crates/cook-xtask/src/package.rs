//! `cargo xtask package` — assembles a release tarball for the cook binary.
//!
//! Tarball layout (Phase 1):
//!     ./VERSION        — version string with trailing newline
//!     ./bin/cook       — the cook executable, mode 0755

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use clap::Args;
use flate2::write::GzEncoder;
use flate2::Compression;
use sha2::{Digest, Sha256};
use tar::Builder;

/// Normalized OS identifier used in tarball filenames.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Os {
    Linux,
    Darwin,
}

/// Normalized architecture identifier used in tarball filenames.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Arch {
    Amd64,
    Arm64,
}

/// Parsed and validated components derived from a rustc target triple.
#[derive(Debug)]
pub struct TargetParts {
    pub os: Os,
    pub arch: Arch,
}

impl Os {
    fn as_str(&self) -> &'static str {
        match self {
            Os::Linux => "linux",
            Os::Darwin => "darwin",
        }
    }
}

impl Arch {
    fn as_str(&self) -> &'static str {
        match self {
            Arch::Amd64 => "amd64",
            Arch::Arm64 => "arm64",
        }
    }
}

/// Parses a rustc target triple into (OS, ARCH) components.
///
/// Returns an error for any OS or arch not supported in Phase 1.
pub fn parse_target(triple: &str) -> Result<TargetParts> {
    let os = if triple.contains("-linux-") || triple.ends_with("-linux") {
        Os::Linux
    } else if triple.contains("-apple-darwin") {
        Os::Darwin
    } else {
        bail!("Phase 1 supports only linux and darwin.")
    };

    let arch = if triple.starts_with("x86_64-") {
        Arch::Amd64
    } else if triple.starts_with("aarch64-") {
        Arch::Arm64
    } else {
        bail!("Phase 1 supports only amd64 and arm64.")
    };

    Ok(TargetParts { os, arch })
}

/// Returns the tarball filename for the given version and target.
pub fn tarball_name(version: &str, target: &TargetParts) -> String {
    format!(
        "cook-{}-{}-{}.tar.gz",
        version,
        target.os.as_str(),
        target.arch.as_str()
    )
}

#[derive(Args, Debug)]
pub struct PackageArgs {
    /// Path to the pre-built cook binary.
    #[arg(long)]
    pub binary: PathBuf,

    /// Version string (e.g. v0.0.1-dev); written verbatim into ./VERSION.
    #[arg(long)]
    pub version: String,

    /// Rustc target triple (e.g. x86_64-unknown-linux-gnu).
    #[arg(long)]
    pub target: String,
}

/// Builds the tarball and returns the output path and hex-encoded SHA-256.
pub fn run(args: &PackageArgs, workspace_root: &Path) -> Result<(PathBuf, String)> {
    let target_parts = parse_target(&args.target)?;
    let filename = tarball_name(&args.version, &target_parts);

    let dist_dir = workspace_root.join("target").join("dist");
    fs::create_dir_all(&dist_dir)
        .with_context(|| format!("failed to create dist dir: {}", dist_dir.display()))?;

    let out_path = dist_dir.join(&filename);
    let out_file = fs::File::create(&out_path)
        .with_context(|| format!("cannot create {}", out_path.display()))?;

    let gz = GzEncoder::new(out_file, Compression::default());
    let mut ar = Builder::new(gz);

    // Write ./VERSION — version string + newline
    let version_bytes = format!("{}\n", args.version).into_bytes();
    let mut version_header = tar::Header::new_gnu();
    version_header.set_size(version_bytes.len() as u64);
    version_header.set_mode(0o644);
    version_header.set_cksum();
    ar.append_data(&mut version_header, "./VERSION", version_bytes.as_slice())
        .context("failed to append VERSION to tarball")?;

    // Write ./bin/cook — the cook binary, executable
    let binary_path = &args.binary;
    let binary_data = fs::read(binary_path)
        .with_context(|| format!("cannot read binary: {}", binary_path.display()))?;

    let mut bin_header = tar::Header::new_gnu();
    bin_header.set_size(binary_data.len() as u64);
    bin_header.set_mode(0o755);
    bin_header.set_cksum();
    ar.append_data(&mut bin_header, "./bin/cook", binary_data.as_slice())
        .context("failed to append bin/cook to tarball")?;

    // Flush and finish the gzip stream
    let gz_inner = ar.into_inner().context("failed to finish tar archive")?;
    gz_inner.finish().context("failed to finish gzip stream")?;

    // SHA-256 over the written file
    let tarball_bytes =
        fs::read(&out_path).with_context(|| format!("cannot re-read {}", out_path.display()))?;
    let digest = Sha256::digest(&tarball_bytes);
    let hex_digest = hex::encode(digest);

    // Append checksum line to the release-wide checksums file.
    // Callers SHOULD clear cli/target/dist/ before a fresh release run so
    // append-mode produces a clean file (one line per target).
    let checksums_path = dist_dir.join(format!("cook-{}-checksums.txt", args.version));
    let checksum_line = format!("{}  {}\n", hex_digest, filename);
    let mut checksums_file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&checksums_path)
        .with_context(|| format!("cannot open checksums file: {}", checksums_path.display()))?;
    checksums_file
        .write_all(checksum_line.as_bytes())
        .context("failed to write checksum line")?;

    Ok((out_path, hex_digest))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_linux_amd64() {
        let t = parse_target("x86_64-unknown-linux-gnu").unwrap();
        assert_eq!(t.os, Os::Linux);
        assert_eq!(t.arch, Arch::Amd64);
    }

    #[test]
    fn parse_linux_arm64() {
        let t = parse_target("aarch64-unknown-linux-gnu").unwrap();
        assert_eq!(t.os, Os::Linux);
        assert_eq!(t.arch, Arch::Arm64);
    }

    #[test]
    fn parse_darwin_amd64() {
        let t = parse_target("x86_64-apple-darwin").unwrap();
        assert_eq!(t.os, Os::Darwin);
        assert_eq!(t.arch, Arch::Amd64);
    }

    #[test]
    fn parse_darwin_arm64() {
        let t = parse_target("aarch64-apple-darwin").unwrap();
        assert_eq!(t.os, Os::Darwin);
        assert_eq!(t.arch, Arch::Arm64);
    }

    #[test]
    fn parse_musl_linux_is_linux() {
        let t = parse_target("x86_64-unknown-linux-musl").unwrap();
        assert_eq!(t.os, Os::Linux);
    }

    #[test]
    fn parse_unsupported_os_errors() {
        let err = parse_target("x86_64-pc-windows-msvc").unwrap_err();
        assert!(
            err.to_string().contains("linux and darwin"),
            "unexpected message: {err}"
        );
    }

    #[test]
    fn parse_unsupported_arch_errors() {
        let err = parse_target("riscv64gc-unknown-linux-gnu").unwrap_err();
        assert!(
            err.to_string().contains("amd64 and arm64"),
            "unexpected message: {err}"
        );
    }

    #[test]
    fn tarball_name_format() {
        let t = parse_target("x86_64-unknown-linux-gnu").unwrap();
        assert_eq!(
            tarball_name("v0.0.1-dev", &t),
            "cook-v0.0.1-dev-linux-amd64.tar.gz"
        );
    }

    #[test]
    fn tarball_name_darwin_arm64() {
        let t = parse_target("aarch64-apple-darwin").unwrap();
        assert_eq!(
            tarball_name("v1.2.3", &t),
            "cook-v1.2.3-darwin-arm64.tar.gz"
        );
    }
}
