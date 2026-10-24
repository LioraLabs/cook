//! The manifest codec `CloudBackend` and `CacheServer` share: one route, one
//! size cap, one decoder.

use std::io::Read;

use crate::backend::{CloudKey, DeterminantManifest};

pub(crate) const MANIFESTS_PATH: &str = "/v1/manifests/";
/// ~200 bytes per input; the largest of 6,614 real manifests was 86.5 KB, so
/// 4 MiB holds ~20,000 inputs. It bounds one PUT, not the store: keys are
/// unbounded and manifest sidecars are not yet evicted (gc owns that).
pub(crate) const MAX_MANIFEST_BYTES: u64 = 4 * 1024 * 1024;

/// Why a manifest body was refused: the server answers 413 or 400.
pub(crate) enum ManifestRefusal {
    TooLarge,
    Malformed(String),
    /// The transport failed mid-body: not the manifest's fault, so a client retries.
    Io(String),
}

impl std::fmt::Display for ManifestRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooLarge => write!(f, "manifest over {MAX_MANIFEST_BYTES} bytes"),
            Self::Malformed(why) | Self::Io(why) => f.write_str(why),
        }
    }
}

/// Read a manifest of at most `MAX_MANIFEST_BYTES` and require that it names `key`.
pub(crate) fn decode_manifest(
    key: &CloudKey,
    reader: impl Read,
) -> Result<DeterminantManifest, ManifestRefusal> {
    let mut bytes = Vec::new();
    reader
        .take(MAX_MANIFEST_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| ManifestRefusal::Io(format!("read manifest: {e}")))?;
    if bytes.len() as u64 > MAX_MANIFEST_BYTES {
        return Err(ManifestRefusal::TooLarge);
    }
    let manifest: DeterminantManifest = serde_json::from_slice(&bytes)
        .map_err(|e| ManifestRefusal::Malformed(format!("malformed manifest: {e}")))?;
    let want = hex::encode(key);
    if manifest.key != want {
        return Err(ManifestRefusal::Malformed(format!(
            "manifest names key {} but was addressed to {want}",
            manifest.key
        )));
    }
    Ok(manifest)
}
