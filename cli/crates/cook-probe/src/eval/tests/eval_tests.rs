//! One test per rule that used to live in only one of the two copies
//! (COOK-359). Each rule now has exactly one implementation, so it needs
//! exactly one test — which is the point of the extraction.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use cook_contracts::{ProbeInputs, ProbeUnit};

use crate::eval::{
    evaluate, probe_artifact_meta, CacheAccess, EvalCtx, Evaluated, ProbeError, ProduceRunner,
};

/// Counts executions so a test can assert that `produce` did NOT run, which is
/// the only way to tell a cache hit from a re-produce that happened to return
/// the same bytes.
struct CountingRunner {
    value: Vec<u8>,
    runs: RefCell<usize>,
}

impl CountingRunner {
    fn new(value: &str) -> Self {
        Self { value: value.as_bytes().to_vec(), runs: RefCell::new(0) }
    }
    fn runs(&self) -> usize {
        *self.runs.borrow()
    }
}

impl ProduceRunner for CountingRunner {
    fn run(&self, _key: &str, _source: &str) -> Result<Vec<u8>, String> {
        *self.runs.borrow_mut() += 1;
        Ok(self.value.clone())
    }
}

struct FailingRunner;

impl ProduceRunner for FailingRunner {
    fn run(&self, _key: &str, _source: &str) -> Result<Vec<u8>, String> {
        Err("boom".to_string())
    }
}

/// A runner that must never be called. Reaching it is the failure.
struct PoisonRunner;

impl ProduceRunner for PoisonRunner {
    fn run(&self, key: &str, _source: &str) -> Result<Vec<u8>, String> {
        panic!("produce ran for '{key}' when the value should have been served from cache");
    }
}

fn probe(key: &str, inputs: ProbeInputs) -> ProbeUnit {
    ProbeUnit {
        key: key.to_string(),
        produce_source: "return { 1 }".to_string(),
        produce_line: 1,
        inputs,
    }
}

fn declares_nothing(key: &str) -> ProbeUnit {
    probe(key, ProbeInputs::default())
}

fn declares_file(key: &str, path: &str) -> ProbeUnit {
    probe(key, ProbeInputs { files: vec![path.to_string()], ..Default::default() })
}

fn backend(root: &Path) -> cook_cache::backend::LocalBackend {
    cook_cache::backend::LocalBackend::new(root.to_path_buf())
}

fn no_env(_: &str) -> Option<String> {
    None
}

/// Evaluate with a wired backend.
fn eval_cached(
    unit: &ProbeUnit,
    wd: &Path,
    be: &cook_cache::backend::LocalBackend,
    runner: &dyn ProduceRunner,
    keyless_upstreams: &BTreeSet<String>,
    upstream_fps: &BTreeMap<String, [u8; 32]>,
    publish: bool,
) -> Result<Evaluated, ProbeError> {
    let ctx = EvalCtx {
        working_dir: wd,
        cache: Some(CacheAccess { backend: be, project_root: wd, publish_enabled: publish }),
    };
    evaluate(unit, &ctx, runner, &no_env, upstream_fps, keyless_upstreams)
}

#[test]
fn a_keyed_probe_is_served_from_cache_on_the_second_evaluation() {
    let tmp = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("dep.txt"), "content").unwrap();
    let be = backend(store.path());
    let unit = declares_file("ns:keyed", "dep.txt");

    let first = eval_cached(
        &unit, tmp.path(), &be, &CountingRunner::new("[1]"),
        &BTreeSet::new(), &BTreeMap::new(), true,
    )
    .unwrap();
    assert!(!first.cache_hit);

    // A runner that panics if reached: the second evaluation must not produce.
    let second = eval_cached(
        &unit, tmp.path(), &be, &PoisonRunner,
        &BTreeSet::new(), &BTreeMap::new(), true,
    )
    .unwrap();
    assert!(second.cache_hit);
    assert_eq!(first.bytes, second.bytes);
    assert_eq!(first.fingerprint, second.fingerprint);
}

#[test]
fn cs0178_a_probe_declaring_nothing_is_keyless_and_always_reproduces() {
    let tmp = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    let be = backend(store.path());
    let unit = declares_nothing("ns:keyless");
    let runner = CountingRunner::new("[1]");

    for _ in 0..3 {
        let out = eval_cached(
            &unit, tmp.path(), &be, &runner,
            &BTreeSet::new(), &BTreeMap::new(), true,
        )
        .unwrap();
        assert!(out.keyless);
        assert!(!out.cache_hit);
    }
    assert_eq!(runner.runs(), 3, "a keyless probe must re-produce every time");
}

#[test]
fn cs0178_a_keyless_probe_publishes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    let be = backend(store.path());

    eval_cached(
        &declares_nothing("ns:keyless"), tmp.path(), &be, &CountingRunner::new("[1]"),
        &BTreeSet::new(), &BTreeMap::new(), true,
    )
    .unwrap();

    // Skipping only the GET would leave a stable fingerprint addressing a
    // stored value that another reader of a shared store could still be served.
    assert_eq!(
        file_count(store.path()), 0,
        "a keyless probe wrote to the shared store",
    );
}

#[test]
fn cs0178_keylessness_propagates_along_requires() {
    let tmp = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    let be = backend(store.path());

    // Declares a `requires` — so not keyless by its own declaration — but the
    // upstream it names is.
    let unit = probe(
        "ns:downstream",
        ProbeInputs { requires: vec!["ns:keyless".to_string()], ..Default::default() },
    );
    let mut upstream_fps = BTreeMap::new();
    upstream_fps.insert("ns:keyless".to_string(), [7u8; 32]);
    let mut keyless_upstreams = BTreeSet::new();
    keyless_upstreams.insert("ns:keyless".to_string());

    let runner = CountingRunner::new("[1]");
    for _ in 0..2 {
        let out = eval_cached(
            &unit, tmp.path(), &be, &runner, &keyless_upstreams, &upstream_fps, true,
        )
        .unwrap();
        assert!(
            out.keyless,
            "a probe requiring a keyless probe folds a constant upstream \
             fingerprint, so it would be served across the change it exists to notice",
        );
    }
    assert_eq!(runner.runs(), 2);
}

#[test]
fn cook168_publish_off_suppresses_the_upload_but_not_the_value() {
    let tmp = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("dep.txt"), "content").unwrap();
    let be = backend(store.path());

    let out = eval_cached(
        &declares_file("ns:keyed", "dep.txt"), tmp.path(), &be,
        &CountingRunner::new("[1]"), &BTreeSet::new(), &BTreeMap::new(),
        /*publish*/ false,
    )
    .unwrap();

    assert_eq!(out.bytes, b"[1]");
    assert_eq!(file_count(store.path()), 0, "publish-off still uploaded");
}

#[test]
fn cs0148_a_files_producer_is_synthesised_and_never_reaches_a_vm() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("a.txt"), "alpha").unwrap();

    let mut unit = declares_file("ns:manifest", "a.txt");
    unit.produce_source = cook_contracts::probe_value::FILES_MANIFEST_PRODUCE.to_string();

    let ctx = EvalCtx { working_dir: tmp.path(), cache: None };
    // COOK-353: the sentinel is deliberately not valid Lua, so a path that
    // tried to run it would die on a bare `@`. PoisonRunner proves no path does.
    let out = evaluate(&unit, &ctx, &PoisonRunner, &no_env, &BTreeMap::new(), &BTreeSet::new())
        .unwrap();

    let value = cook_contracts::probe_value::decode_json(&out.bytes).unwrap();
    assert!(
        value.get("a.txt").is_some(),
        "files manifest should map the declared path, got {value}",
    );
}

#[test]
fn cs0102_unparseable_cached_bytes_are_evicted_not_merely_ignored() {
    let tmp = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("dep.txt"), "content").unwrap();
    let be = backend(store.path());
    let unit = declares_file("ns:keyed", "dep.txt");

    // Learn the fingerprint, then poison that exact key.
    let first = eval_cached(
        &unit, tmp.path(), &be, &CountingRunner::new("[1]"),
        &BTreeSet::new(), &BTreeMap::new(), true,
    )
    .unwrap();
    // CS-0055 conflict detection refuses to overwrite a key with differing
    // bytes, so poisoning means replacing the entry, not writing over it.
    let poison = b"not json at all";
    cook_fingerprint::backend::CacheBackend::delete(&be, &first.fingerprint).unwrap();
    let mut meta = probe_artifact_meta("ns:keyed", poison.len());
    cook_cache::backend::put_bytes(&be, &first.fingerprint, poison, &mut meta).unwrap();

    let runner = CountingRunner::new("[1]");
    let out = eval_cached(
        &unit, tmp.path(), &be, &runner, &BTreeSet::new(), &BTreeMap::new(), true,
    )
    .unwrap();

    assert!(!out.cache_hit, "unparseable bytes must read as a miss");
    assert_eq!(runner.runs(), 1, "the miss must re-produce");
    assert!(
        out.warnings.iter().any(|w| w.contains("not probe-value JSON")),
        "the condition must be reported, got {:?}", out.warnings,
    );
    // Self-healed: the poisoned key now addresses valid bytes again, so the
    // next reader of the shared store is not served the same garbage forever.
    let served = eval_cached(
        &unit, tmp.path(), &be, &PoisonRunner, &BTreeSet::new(), &BTreeMap::new(), true,
    )
    .unwrap();
    assert!(served.cache_hit);
    assert_eq!(served.bytes, b"[1]");
}

#[test]
fn cs0102_the_canonical_local_copy_is_written_with_the_value_bytes() {
    let tmp = tempfile::tempdir().unwrap();
    let ctx = EvalCtx { working_dir: tmp.path(), cache: None };
    let out = evaluate(
        &declares_nothing("ns:local"), &ctx, &CountingRunner::new("[1]"),
        &no_env, &BTreeMap::new(), &BTreeSet::new(),
    )
    .unwrap();

    let path = tmp.path().join(".cook/probes").join(
        cook_contracts::probe::value::probe_file_name("ns:local"),
    );
    assert_eq!(std::fs::read(&path).unwrap(), out.bytes);
}

#[test]
fn a_produce_failure_names_the_probe() {
    let tmp = tempfile::tempdir().unwrap();
    let ctx = EvalCtx { working_dir: tmp.path(), cache: None };
    let err = evaluate(
        &declares_nothing("ns:bad"), &ctx, &FailingRunner,
        &no_env, &BTreeMap::new(), &BTreeSet::new(),
    )
    .unwrap_err();

    assert_eq!(err.key(), "ns:bad");
    assert!(err.to_string().contains("boom"));
}

#[test]
fn a_missing_upstream_fingerprint_is_a_resolve_error() {
    let tmp = tempfile::tempdir().unwrap();
    let unit = probe(
        "ns:downstream",
        ProbeInputs { requires: vec!["ns:absent".to_string()], ..Default::default() },
    );
    let ctx = EvalCtx { working_dir: tmp.path(), cache: None };
    let err = evaluate(
        &unit, &ctx, &PoisonRunner, &no_env, &BTreeMap::new(), &BTreeSet::new(),
    )
    .unwrap_err();

    assert!(matches!(err, ProbeError::ResolveInputs { .. }));
    assert_eq!(err.key(), "ns:downstream");
}

fn file_count(dir: &Path) -> usize {
    let mut count = 0;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(p) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&p) else { continue };
        for entry in rd.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                count += 1;
            }
        }
    }
    count
}
