//! Resolve a ProbeUnit's declared inputs into ProbeFingerprintInputs by
//! consulting the current env, PATH, filesystem, and upstream probe map.

use std::collections::BTreeMap;
use std::path::Path;

use cook_contracts::ProbeUnit;
use sha2::{Digest, Sha256};

use crate::context::ProbeFingerprintInputs;

/// Resolve a `ProbeUnit`'s declared inputs into `ProbeFingerprintInputs` by
/// walking env/PATH/filesystem/upstream-fp-map.
pub fn resolve_probe_inputs(
    probe: &ProbeUnit,
    working_dir: &Path,
    env_lookup: &dyn Fn(&str) -> Option<String>,
    upstream_fingerprints: &BTreeMap<String, [u8; 32]>,
) -> Result<ProbeFingerprintInputs, String> {
    let env: Vec<(String, Option<String>)> = probe
        .inputs
        .env
        .iter()
        .map(|name| (name.clone(), env_lookup(name)))
        .collect();

    let tools: Vec<(String, [u8; 32])> = probe
        .inputs
        .tools
        .iter()
        .map(|name| (name.clone(), resolve_tool_hash(name)))
        .collect();

    let files: Vec<(String, [u8; 32])> = probe
        .inputs
        .files
        .iter()
        .map(|path| (path.clone(), hash_file(&working_dir.join(path))))
        .collect();

    let upstream_probes: Vec<(String, [u8; 32])> = probe
        .inputs
        .requires
        .iter()
        .map(|k| {
            let fp = upstream_fingerprints.get(k).copied().ok_or_else(|| {
                format!(
                    "probe '{}' requires upstream '{}' which has no fingerprint",
                    probe.key, k,
                )
            })?;
            Ok((k.clone(), fp))
        })
        .collect::<Result<_, String>>()?;

    Ok(ProbeFingerprintInputs {
        key: probe.key.clone(),
        produce_source: probe.produce_source.clone(),
        env,
        tools,
        files,
        upstream_probes,
    })
}

fn resolve_tool_hash(name: &str) -> [u8; 32] {
    let Ok(path) = which::which(name) else {
        return [0u8; 32];
    };
    hash_file(&path)
}

fn hash_file(path: &Path) -> [u8; 32] {
    let Ok(bytes) = std::fs::read(path) else {
        return [0u8; 32];
    };
    let result = Sha256::digest(&bytes);
    let mut out = [0u8; 32];
    out.copy_from_slice(&result);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn resolve_probe_inputs_with_no_inputs_succeeds() {
        let probe = ProbeUnit {
            key: "cc:x".into(),
            produce_source: "return 1".into(),
            produce_line: 1,
            inputs: cook_contracts::ProbeInputs::default(),
        };
        let r = resolve_probe_inputs(&probe, &PathBuf::from("."), &|_| None, &BTreeMap::new());
        assert!(r.is_ok());
    }

    #[test]
    fn missing_upstream_fingerprint_errors() {
        let mut probe = ProbeUnit {
            key: "cc:x".into(),
            produce_source: "return 1".into(),
            produce_line: 1,
            inputs: cook_contracts::ProbeInputs::default(),
        };
        probe.inputs.requires = vec!["cc:missing".into()];
        let r = resolve_probe_inputs(&probe, &PathBuf::from("."), &|_| None, &BTreeMap::new());
        let err = r.unwrap_err();
        assert!(err.contains("cc:missing"), "got: {}", err);
        assert!(err.contains("cc:x"), "got: {}", err);
    }

    #[test]
    fn env_lookup_propagates_to_fingerprint_inputs() {
        let mut probe = ProbeUnit {
            key: "k".into(),
            produce_source: "".into(),
            produce_line: 1,
            inputs: cook_contracts::ProbeInputs::default(),
        };
        probe.inputs.env = vec!["MY_VAR".into()];
        let lookup = |name: &str| match name {
            "MY_VAR" => Some("value".into()),
            _ => None,
        };
        let r =
            resolve_probe_inputs(&probe, &PathBuf::from("."), &lookup, &BTreeMap::new()).unwrap();
        assert_eq!(r.env, vec![("MY_VAR".into(), Some("value".into()))]);
    }

    #[test]
    fn missing_env_value_becomes_none() {
        let mut probe = ProbeUnit {
            key: "k".into(),
            produce_source: "".into(),
            produce_line: 1,
            inputs: cook_contracts::ProbeInputs::default(),
        };
        probe.inputs.env = vec!["UNSET_VAR".into()];
        let r =
            resolve_probe_inputs(&probe, &PathBuf::from("."), &|_| None, &BTreeMap::new()).unwrap();
        assert_eq!(r.env, vec![("UNSET_VAR".into(), None)]);
    }
}
