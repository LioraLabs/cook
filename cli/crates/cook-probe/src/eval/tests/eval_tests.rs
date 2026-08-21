//! One test per rule that used to live in only one of the two copies
//! (COOK-359). Each rule now has exactly one implementation, so it needs
//! exactly one test — which is the point of the extraction.
//!
//! CS-0243 deleted the probe-value cache. Tests that used to pin cache-hit
//! behaviour are inverted or removed; see the individual doc comments below.

use std::cell::RefCell;
use std::path::Path;

use cook_contracts::{LocalProbeKey, ProbeInputs, ProbeUnit};

use crate::eval::{evaluate, EvalCtx, Produced, ProduceRunner};

/// Counts executions so a test can assert how many times `produce` ran.
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
    fn run(&self, _key: &str, _source: &str) -> Result<Produced, String> {
        *self.runs.borrow_mut() += 1;
        Ok(Produced { bytes: self.value.clone() })
    }
}

struct FailingRunner;

impl ProduceRunner for FailingRunner {
    fn run(&self, _key: &str, _source: &str) -> Result<Produced, String> {
        Err("boom".to_string())
    }
}

/// A runner that must never be called. Reaching it is the failure.
struct PoisonRunner;

impl ProduceRunner for PoisonRunner {
    fn run(&self, key: &str, _source: &str) -> Result<Produced, String> {
        panic!("produce ran for '{key}' when no VM should have been reached");
    }
}

fn probe(key: &str, inputs: ProbeInputs) -> ProbeUnit {
    ProbeUnit {
        key: LocalProbeKey::new(key),
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

fn declares_tools(key: &str, tool: &str) -> ProbeUnit {
    probe(key, ProbeInputs { tools: vec![tool.to_string()], ..Default::default() })
}

/// CS-0243: an evaluation with no project root wired, matching a workspace
/// that has no resolved cache context.
fn ctx(wd: &Path) -> EvalCtx<'_> {
    EvalCtx { working_dir: wd, project_root: None, declaring_prefix: "" }
}

/// CS-0243: a reached probe always observes. An unchanged declared input
/// does not serve the first evaluation's bytes on the second — there is no
/// store to serve them from. Inverts the pre-CS-0243
/// `a_keyed_probe_is_served_from_cache_on_the_second_evaluation`.
#[test]
fn a_keyed_probe_re_executes_on_the_second_evaluation() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("dep.txt"), "content").unwrap();
    let unit = declares_file("ns:keyed", "dep.txt");
    let eval_ctx = ctx(tmp.path());
    let runner = CountingRunner::new("[1]");

    let first =
        evaluate(&unit, &eval_ctx, &runner).unwrap();
    let second =
        evaluate(&unit, &eval_ctx, &runner).unwrap();

    assert_eq!(
        runner.runs(), 2,
        "a reached probe must run produce on every invocation, even with an \
         unchanged declared input",
    );
    assert_eq!(first.bytes, second.bytes);
}

/// The same rule for a probe that declares nothing at all. Kept as its own
/// test because CS-0244 retired the distinction the two used to sit on either
/// side of (CS-0178 keylessness), and a rule that now holds for every probe
/// alike is worth pinning at both of the old poles.
#[test]
fn a_probe_declaring_nothing_re_executes_on_every_evaluation() {
    let tmp = tempfile::tempdir().unwrap();
    let unit = declares_nothing("ns:bare");
    let runner = CountingRunner::new("[1]");
    let eval_ctx = ctx(tmp.path());

    for _ in 0..3 {
        evaluate(&unit, &eval_ctx, &runner).unwrap();
    }
    assert_eq!(runner.runs(), 3, "a reached probe must re-produce every time");
}

/// CS-0243: no probe value is ever written anywhere but the local forensic
/// record, `.cook/probes/<key>.json`. Replaces the pre-CS-0243
/// `cs0178_a_keyless_probe_publishes_nothing`, which pinned that only
/// keylessness suppressed publishing; there is no publishing left to
/// suppress, and since CS-0244 no keylessness to suppress it with.
#[test]
fn cs0243_no_probe_value_is_ever_written_outside_the_local_record() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("dep.txt"), "content").unwrap();
    // A probe with a declared input: the case CS-0178 used to exempt from
    // nothing, so it is the one that would still be publishing if anything
    // were.
    let unit = declares_file("ns:keyed", "dep.txt");
    let eval_ctx = ctx(tmp.path());

    evaluate(&unit, &eval_ctx, &CountingRunner::new("[1]"))
        .unwrap();

    let probes_dir = tmp.path().join(".cook").join("probes");
    assert_eq!(
        file_count(tmp.path()),
        file_count(&probes_dir) + 1, // + the pre-existing dep.txt
        "a probe wrote somewhere other than .cook/probes",
    );
}

/// CS-0244: `requires` is an ordering edge and nothing else. Evaluating a
/// probe that names an upstream neither consults it nor fails for want of it —
/// the ordering is the DAG's (`unit_graph::plan`) and the register resolver's
/// recursion, not this sequence's. Replaces
/// `cs0178_keylessness_propagates_along_requires` and
/// `a_missing_upstream_fingerprint_is_a_resolve_error`, which both pinned the
/// fingerprint chain this entry deleted.
#[test]
fn a_declared_requires_neither_keys_nor_blocks_the_evaluation() {
    let tmp = tempfile::tempdir().unwrap();
    let unit = probe(
        "ns:downstream",
        ProbeInputs { requires: vec!["ns:never-evaluated".to_string()], ..Default::default() },
    );

    let runner = CountingRunner::new("[1]");
    let eval_ctx = ctx(tmp.path());
    for _ in 0..2 {
        evaluate(&unit, &eval_ctx, &runner)
            .expect("an unresolved upstream is the scheduler's business, not this sequence's");
    }
    assert_eq!(runner.runs(), 2);
}

#[test]
fn cs0148_a_files_producer_is_synthesised_and_never_reaches_a_vm() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("a.txt"), "alpha").unwrap();

    let mut unit = declares_file("ns:manifest", "a.txt");
    unit.produce_source = cook_contracts::probe_value::FILES_MANIFEST_PRODUCE.to_string();

    let eval_ctx = ctx(tmp.path());
    // COOK-353: the sentinel is deliberately not valid Lua, so a path that
    // tried to run it would die on a bare `@`. PoisonRunner proves no path does.
    let out = evaluate(&unit, &eval_ctx, &PoisonRunner)
        .unwrap();

    let value = cook_contracts::probe_value::decode_json(&out.bytes).unwrap();
    assert!(
        value.get("a.txt").is_some(),
        "files manifest should map the declared path, got {value}",
    );
}

#[test]
fn cs0214_a_tools_producer_is_synthesised_from_the_digests_its_declaration_resolved() {
    let tmp = tempfile::tempdir().unwrap();
    let mut unit = declares_tools("ns:tc", "sh");
    unit.produce_source = cook_contracts::probe_value::TOOLS_IDENTITY_PRODUCE.to_string();

    let eval_ctx = ctx(tmp.path());
    // Before CS-0214 this producer was a Lua program shelling out to
    // `command -v` and `sha256sum`. PoisonRunner proves no VM is reached now,
    // which is also what makes the producer work on a host with no coreutils.
    let out = evaluate(&unit, &eval_ctx, &PoisonRunner)
        .unwrap();

    let value = cook_contracts::probe_value::decode_json(&out.bytes).unwrap();
    let hash = value
        .get("sh")
        .and_then(|e| e.get("hash"))
        .and_then(|h| h.as_str())
        .unwrap_or_else(|| panic!("expected {{ sh = {{ hash }} }}, got {value}"));

    // The rule under test is not "there is a hash" but "it is THE hash": the
    // same content digest the `tools` declaration just resolved. Two
    // computations that agree are what CS-0214 retired.
    let resolved = cook_cache::resolve_tool_path("sh").expect("sh resolves on any unix host");
    assert_eq!(
        hash,
        cook_contracts::render::lower_hex(&cook_cache::probe::hash_file_sha256(Path::new(
            &resolved
        ))),
    );
    assert!(
        !out.bytes.windows(4).any(|w| w == b"path"),
        "CS-0157: location must never enter the value bytes",
    );
}

#[test]
fn cs0214_a_tools_probe_naming_an_unresolvable_tool_fails_by_name() {
    let tmp = tempfile::tempdir().unwrap();
    let mut unit = declares_tools("ns:tc", "cook-no-such-tool-COOK-416");
    unit.produce_source = cook_contracts::probe_value::TOOLS_IDENTITY_PRODUCE.to_string();

    // §22.5.2 requires the failure ahead of any value resolution. Before
    // CS-0243 the interesting case was a cache hit that this check had to
    // outrun; with no cache left the ordering rule is simpler to state — the
    // check runs before `lookup`'s resolved/produce fork, full stop —  and
    // PoisonRunner proves no VM is reached either way.
    let eval_ctx = ctx(tmp.path());
    let err = evaluate(&unit, &eval_ctx, &PoisonRunner)
        .unwrap_err();

    assert!(
        err.message.contains("cook-no-such-tool-COOK-416"),
        "the diagnostic must name the tool; got: {err}",
    );
    assert!(err.message.contains("not found on PATH"), "got: {err}");
}

#[test]
#[cfg(unix)]
fn cs0214_a_tools_probe_whose_binary_cannot_be_read_fails_rather_than_recording_zeros() {
    // `which` selects on X_OK, not R_OK, so a name can RESOLVE to a binary
    // whose bytes cannot be read — and the probe-side file hash answers the
    // all-zero digest for anything it cannot read. Rendering that into the
    // value would put the same 64 zeros in every such value, so two hosts each
    // failing to read a DIFFERENT toolchain would compose identical value bytes
    // and one could be served the other's sealed artifact. The deleted Lua producer could not reach this state: `sha256sum`
    // exited non-zero and failed the probe.
    // The declared name is an absolute path rather than a bare one, so `which`
    // resolves it directly and the test never touches the process PATH — a
    // process-global mutation that would race every other test in this binary.
    // A Cookfile can only spell a bare name, but `ProbeInputs.tools` at this
    // seam is a list of strings and the resolver is the same one either way.
    use std::os::unix::fs::PermissionsExt;

    let tmp = tempfile::tempdir().unwrap();
    let tool = tmp.path().join("cook-unreadable-416");
    std::fs::write(&tool, b"#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o111)).unwrap();

    let mut unit = declares_tools("ns:tc", &tool.to_string_lossy());
    unit.produce_source = cook_contracts::probe_value::TOOLS_IDENTITY_PRODUCE.to_string();
    let eval_ctx = ctx(tmp.path());
    let result =
        evaluate(&unit, &eval_ctx, &PoisonRunner);

    // Root can read a mode-0111 file, so the unreadable state is not
    // constructible when the suite runs as root. The rule still holds there:
    // whichever way this lands, an all-zero digest must not reach the value.
    match result {
        Err(err) => {
            assert!(err.message.contains("cook-unreadable-416"), "got: {err}");
            assert!(err.message.contains("could not be read"), "got: {err}");
        }
        Ok(out) => {
            let text = String::from_utf8_lossy(&out.bytes).into_owned();
            assert!(
                !text.contains(&"0".repeat(64)),
                "an all-zero digest must never stand for an identity: {text}",
            );
        }
    }
}

#[test]
#[cfg(unix)]
fn cook510_a_files_probe_whose_matched_path_cannot_be_read_fails_rather_than_recording_missing() {
    // Mirrors cs0214_a_tools_probe_whose_binary_cannot_be_read_fails_rather_than_recording_zeros
    // for the `files` side. `hash_file_sha256` answers the identical all-zero
    // digest for "does not exist" and "exists but unreadable", and
    // §{cat.probes.decl} folds the FORMER as the placeholder `"<missing>"`
    // deliberately — a glob matching nothing is ordinary. Folding the LATTER
    // the same way is not: the placeholder is indistinguishable from a path
    // that was never there, so the synthesised manifest — and every unit
    // sealing it — freezes at a value that can never again observe an edit to
    // that file's content. This is the false-hit COOK-510 exists to close.
    use std::os::unix::fs::PermissionsExt;

    let tmp = tempfile::tempdir().unwrap();
    let unreadable = tmp.path().join("secret.txt");
    std::fs::write(&unreadable, b"content nobody may read").unwrap();
    std::fs::set_permissions(&unreadable, std::fs::Permissions::from_mode(0o000)).unwrap();

    let mut unit = declares_file("ns:manifest", "secret.txt");
    unit.produce_source = cook_contracts::probe_value::FILES_MANIFEST_PRODUCE.to_string();
    let eval_ctx = ctx(tmp.path());
    let result =
        evaluate(&unit, &eval_ctx, &PoisonRunner);

    // Root can read a mode-0000 file, so the unreadable state is not
    // constructible when the suite runs as root — same caveat CS-0214's
    // mirror test carries. Whichever way this lands, "<missing>" must never
    // stand in for a file that is actually there.
    match result {
        Err(err) => {
            assert!(err.message.contains("secret.txt"), "got: {err}");
            assert!(err.message.contains("could not be read"), "got: {err}");
        }
        Ok(out) => {
            let text = String::from_utf8_lossy(&out.bytes).into_owned();
            assert!(
                !text.contains("<missing>"),
                "a present-but-unreadable file must never fold as missing: {text}",
            );
        }
    }
}

#[test]
fn cook510_a_files_probe_whose_matched_path_is_genuinely_absent_still_folds_as_missing() {
    // The regression guard for the test above: COOK-510 narrows the guard to
    // "exists but unreadable", not "any unhashable path". A glob's compile-time
    // resolution can still name a path that is gone by the time the probe
    // evaluates (a file deleted between register and execute), and that stays
    // the pre-existing, spec'd `"<missing>"` fold — no error.
    let tmp = tempfile::tempdir().unwrap();
    // Deliberately never created.
    let mut unit = declares_file("ns:manifest", "gone.txt");
    unit.produce_source = cook_contracts::probe_value::FILES_MANIFEST_PRODUCE.to_string();
    let eval_ctx = ctx(tmp.path());
    let out = evaluate(&unit, &eval_ctx, &PoisonRunner)
        .expect("a genuinely absent match must not fail the probe");

    let value = cook_contracts::probe_value::decode_json(&out.bytes).unwrap();
    assert_eq!(
        value.get("gone.txt").and_then(|v| v.as_str()),
        Some("<missing>"),
        "an absent match keeps folding as the spec'd placeholder, got {value}",
    );
}

#[test]
fn cs0102_the_canonical_local_copy_is_written_with_the_value_bytes() {
    let tmp = tempfile::tempdir().unwrap();
    let eval_ctx = ctx(tmp.path());
    let out = evaluate(
        &declares_nothing("ns:local"), &eval_ctx, &CountingRunner::new("[1]"),
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
    let eval_ctx = ctx(tmp.path());
    let err = evaluate(
        &declares_nothing("ns:bad"), &eval_ctx, &FailingRunner,
    )
    .unwrap_err();

    assert_eq!(err.key, "ns:bad");
    assert!(err.to_string().contains("boom"));
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
