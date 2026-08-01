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
