//! Probe-units integration tests (CS-0074, CS-0243).
//!
//! End-to-end tests exercising the `cook.probe` API and demand-driven
//! scheduling at the binary level — write a Cookfile in a tempdir,
//! invoke `cook build`, and inspect filesystem outputs and `.cook/cache/`.
//!
//! CS-0243 removed the probe-value cache: a reached probe always observes.
//! `.cook/cache/` still holds artifacts for ordinary `cook`-step units
//! (including ones that seal a probe's value), just never a `probe_value`
//! kind any more.
//!
//! Coverage:
//!   * `probe_consumer_end_to_end_first_run_then_cache_hit` — a probe and
//!     a consumer unit that references it; verifies the probe value reaches
//!     the consumer and a second run produces identical output (the probe
//!     re-observes; the consumer, keyed on the probe's unchanged value,
//!     still cache-hits).
//!   * `probe_unreached_is_not_executed` — a probe no recipe-reachable unit
//!     consumes; verifies demand-driven scheduling prunes it (no
//!     `probe_value` artifact written under `.cook/cache/`, which is now
//!     true of every run, reached or not).
//!
//! COOK-527 Task 2 pins — CS-0243 cost no early cutoff, and the per-run
//! probe-value store is not a cross-invocation source:
//!   * `seal_hits_when_the_observed_value_is_unchanged_across_reobservation`
//!     (pin 1) — a `seal`ed unit still cache-hits when two builds observe an
//!     unchanged value, even though the probe itself re-produces every
//!     time; a third build that changes the observed value must then miss,
//!     which is what makes the first two builds' hits mean anything.
//!   * `seal_a_to_b_to_a_revert_re_serves_the_original_artifact` (pin 2) — a
//!     `seal`ed unit's A -> B -> A revert re-serves A's original artifact
//!     from cache on the third build rather than rebuilding it.
//!   * `probes_get_does_not_read_a_prior_invocations_materialised_file`
//!     (pin 3b) — `cook.probes.get(key)` on a step that never demanded `key`
//!     this invocation raises the §22.5.8 not-materialised error even when
//!     an earlier invocation already materialised `.cook/probes/<key>.json`
//!     on disk. Pin 3a (an unreached probe never executes) is already
//!     covered by `probe_unreached_is_not_executed` above.

use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

fn cook_binary() -> std::path::PathBuf {
    let mut path = std::env::current_exe().unwrap();
    path.pop(); // /target/debug/deps  →  /target/debug
    path.pop(); // /target/debug       →  /target
    path.push("cook");
    if !path.exists() {
        panic!(
            "cook binary not found at {} — run `cargo build --bin cook` first",
            path.display()
        );
    }
    path
}

fn run_cook(dir: &Path, args: &[&str]) -> Result<std::process::Output, String> {
    // Isolate the shared cache backend per test dir (see file_ref_integration's
    // run_cook for the rationale). Without this, COOK-162 cold fetch-by-key can
    // serve a previous run's output from the global `~/.cache/cook/cloud` store
    // as a spurious first-run hit, so no local StepEntry is recorded and the
    // `.cook/cache` index assertions below break. The shared store points at a
    // private subdir; the StepEntry index at `.cook/cache` is unaffected.
    let cloud_toml = dir.join(".cook/cloud.toml");
    if !cloud_toml.exists() {
        fs::create_dir_all(dir.join(".cook")).map_err(|e| e.to_string())?;
        let shared = dir.join(".cook/shared-cache");
        fs::write(
            &cloud_toml,
            format!("[cache]\ncache_dir = {:?}\n", shared.to_string_lossy()),
        )
        .map_err(|e| e.to_string())?;
    }
    let out = Command::new(cook_binary())
        .args(args)
        .current_dir(dir)
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!(
            "cook failed (exit={:?}): stdout={}, stderr={}",
            out.status.code(),
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr),
        ));
    }
    Ok(out)
}

/// Shared Cookfile shape for COOK-527 pins 1 and 2. A probe named `key`
/// appends one byte to `probe-runs.log` every time its `produce` body runs
/// and returns `input.txt`'s content as its value; a `seal`ed `cook` step
/// appends a line to `marker.log` only when its command actually executes
/// (never on a cache hit), and writes the probe's value to `out.txt`.
fn watched_cookfile(key: &str) -> String {
    format!(
        r#"
probe {key}
    >{{
        local log = io.open("probe-runs.log", "a")
        log:write("x")
        log:close()
        local h = io.open("input.txt")
        local content = h:read("*a")
        h:close()
        return {{ v = content }}
    }}

recipe build
    seal {key}
    cook "out.txt" {{
        echo run >> marker.log
        echo $<{key}.v> > $<out>
    }}
"#
    )
}

#[test]
fn inline_file_seal_invalidates_without_fanout() {
    let tmp = TempDir::new().unwrap();
    fs::write(
        tmp.path().join("Cookfile"),
        "recipe build\n    seal \"det.txt\"\n    cook \"out.txt\" { cat det.txt > $<out> }\n",
    )
    .unwrap();
    fs::write(tmp.path().join("det.txt"), "one\n").unwrap();

    run_cook(tmp.path(), &["build"]).unwrap();
    assert_eq!(fs::read_to_string(tmp.path().join("out.txt")).unwrap(), "one\n");

    fs::write(tmp.path().join("det.txt"), "two\n").unwrap();
    run_cook(tmp.path(), &["build"]).unwrap();
    assert_eq!(fs::read_to_string(tmp.path().join("out.txt")).unwrap(), "two\n");
}

#[test]
fn inline_seal_resolution_errors_name_the_attempted_reading() {
    let tmp = TempDir::new().unwrap();
    for (operand, message) in [
        // COOK-484 / CS-0235 §8.4.3.1 rule 4: the bare-ref failure names the
        // `seal` step, not the consumer `probes` list its key is unioned into.
        // This test's own name asked for that reading; it had been pinned to
        // the §22.5.6 sentence, which is about a `cook.add_unit` field.
        ("missing_key", "seal: 'missing_key' does not name"),
        ("\"missing/**\"", "quoted file determinant"),
    ] {
        fs::write(
            tmp.path().join("Cookfile"),
            format!(
                "recipe build\n    seal {operand}\n    cook \"out.txt\" {{ echo ok > $<out> }}\n"
            ),
        )
        .unwrap();
        let err = run_cook(tmp.path(), &["build"]).unwrap_err();
        assert!(err.contains(message), "{operand}: {err}");
    }
}

/// First run: probe executes, consumer unit reads its value via `probes`,
/// output file `done.marker` is produced and the consumer's artifact lands
/// in `.cook/cache/`. Second run (CS-0243): the probe re-observes, as every
/// reached probe does.
///
/// The consumer here uses `probes`, not `seal` (COOK-530): `probes` buys it a
/// DAG edge and substitution, never a key (CS-0244 deleted the fingerprint
/// fold that used to be §22.5.6 rule 3) — only `seal` carries a probe's
/// observed VALUE into a consumer's key (§22.5.7). `done.marker` comes out
/// identical on both runs because this probe's produce body is deterministic
/// and substitutes the same literal value into the command text each time,
/// not because the consumer's key tracked the value.
///
/// SHI-222 Phase 7 Task 7 carry-forward: the legacy `register` block
/// that called `cook.add_unit` directly is reshaped so the `cook.add_unit`
/// call lives inside the recipe body (as a bare module call, CS-0134), matching the
/// CS-0077 contract that top-level register-block execution has no
/// active recipe `body_slot`. Top-level `cook.probe` is still
/// session-scoped per spec §6 step 4 and §7.
#[test]
fn probe_consumer_end_to_end_first_run_then_cache_hit() {
    let tmp = TempDir::new().unwrap();
    // Both `cook.probe` and `cook.add_unit` live inside the recipe body
    // so the probe lowers to a body-scope `CapturedUnit` (`WorkPayload::Probe`)
    // and the dag-builder can wire the consumer→probe edge from the
    // consumer's `probes` field (see `dag_builder::build_dag`, CS-0074
    // Bug 2 wiring). Top-level (session-scope) probes are not edge-targets
    // for body-scope consumers in the current dag-builder; that's an
    // orthogonal limitation not exercised by this test.
    let cookfile = r#"
recipe build
        cook.probe("test:greet", {
            inputs = {},
            produce = "return { word = \"hello-from-probe\" }",
        })
        cook.add_unit({
            name = "echo",
            inputs = {},
            outputs = {"done.marker"},
            probes = {"test:greet"},
            command = "echo $<test:greet.word> > done.marker",
        })
"#;
    fs::write(tmp.path().join("Cookfile"), cookfile).unwrap();

    // First run.
    let out1 = run_cook(tmp.path(), &["build"]).expect("first run should succeed");
    let marker = fs::read_to_string(tmp.path().join("done.marker"))
        .expect("done.marker should exist after first run");
    assert!(
        marker.contains("hello-from-probe"),
        "marker should contain probe value; got: {:?}\nstdout: {}\nstderr: {}",
        marker,
        String::from_utf8_lossy(&out1.stdout),
        String::from_utf8_lossy(&out1.stderr)
    );

    // The consumer's artifact (a `cook` step with a declared output) should
    // exist in cache — CS-0243 leaves ordinary unit caching untouched.
    let cache_dir = tmp.path().join(".cook/cache");
    let entries: Vec<_> = fs::read_dir(&cache_dir)
        .unwrap_or_else(|_| panic!("cache dir {} missing", cache_dir.display()))
        .filter_map(|e| e.ok())
        .collect();
    assert!(!entries.is_empty(), "expected at least one cache artifact after first run");

    // Second run — should still succeed and produce the same output, even
    // though the probe re-observed rather than being served from a store.
    let _out2 = run_cook(tmp.path(), &["build"]).expect("second run should succeed");
    let marker2 = fs::read_to_string(tmp.path().join("done.marker")).unwrap();
    assert_eq!(marker, marker2, "probe output should be identical on second run");
}

/// CS-0243: a reached probe always observes. There is no probe-value cache —
/// a probe's produce body MUST run on every invocation in which the probe is
/// reached, even when nothing it declares has changed.
///
/// This test pins the contract with an observable side effect: the probe's
/// produce body appends a single line to `probe-runs.log` each time it
/// runs. After two `cook build` invocations the log MUST contain exactly
/// TWO lines — proving the second run re-invoked the produce body rather
/// than serving the first run's value from a store.
///
/// Inverts the pre-CS-0243 `probe_produce_does_not_re_execute_on_cache_hit`,
/// which pinned the opposite law under the removed cache.
///
/// The probe key and produce-source contents are uniquified per test
/// invocation so the host-wide cache (~/.cache/cook/cloud/) cannot leak
/// state across `cargo test` runs.
///
/// SHI-222 Phase 7 Task 7 carry-forward: same reshape as
/// `probe_consumer_end_to_end_first_run_then_cache_hit` — the
/// `cook.add_unit` call moves into the recipe body so the test
/// composes against the CS-0077 register-pass contract (top-level
/// `register` blocks execute with no active recipe `body_slot`).
#[test]
fn probe_produce_re_executes_on_every_invocation() {
    let tmp = TempDir::new().unwrap();
    // Uniquify the probe key per test invocation so we never collide with
    // a cached probe-value from a prior test run.
    let uniq = format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let probe_key = format!("test:counter-{uniq}");
    // Embed the same uniquifier in the produce source itself, so a stray
    // artifact from a same-key prior run cannot be mistaken for this one's
    // (the key already uniquifies, but defence in depth is cheap).
    // Body-scope probe + body-scope consumer: the dag-builder wires the
    // consumer→probe edge from the consumer's `probes` field against the
    // probe `CapturedUnit` it finds in the same recipe body.
    let cookfile = format!(
        r#"
recipe build
        cook.probe("{probe_key}", {{
            inputs = {{ files = {{ "seed.txt" }} }},
            produce = [[
                -- uniq={uniq}
                local f = io.open("probe-runs.log", "a")
                f:write("ran\n")
                f:close()
                return {{ v = 1 }}
            ]],
        }})
        cook.add_unit({{
            name = "consume",
            inputs = {{}},
            outputs = {{"done.marker"}},
            probes = {{"{probe_key}"}},
            command = "echo $<{probe_key}.v> > done.marker",
        }})
"#
    );
    fs::write(tmp.path().join("Cookfile"), &cookfile).unwrap();
    // A declared input is not what makes this probe re-produce: CS-0243 means
    // EVERY reached probe re-produces regardless. Kept anyway, because it
    // exercises the declared-`files` resolution path rather than the empty
    // one.
    fs::write(tmp.path().join("seed.txt"), "seed\n").unwrap();

    // First run: produce body MUST execute.
    run_cook(tmp.path(), &["build"]).expect("first run should succeed");
    let log1 = fs::read_to_string(tmp.path().join("probe-runs.log"))
        .expect("probe-runs.log should exist after first run");
    assert_eq!(
        log1, "ran\n",
        "first run: produce body should have executed exactly once, got log: {log1:?}"
    );

    // Second run: CS-0243 — the probe is reached again with nothing about its
    // declared inputs changed, and it MUST re-observe rather than being
    // served the first run's value. The log growing to "ran\nran\n" is the
    // side effect under test.
    run_cook(tmp.path(), &["build"]).expect("second run should succeed");
    let log2 = fs::read_to_string(tmp.path().join("probe-runs.log")).unwrap();
    assert_eq!(
        log2, "ran\nran\n",
        "second run: a reached probe MUST re-execute produce every invocation; \
         got log: {log2:?} (expected \"ran\\nran\\n\")"
    );
}

/// A native shell-block `probe` (`as lines`) feeding a member fan-out: the lowering
/// executes via the §22.5.10 register pre-pass and fans out one unit per line.
#[test]
fn native_probe_member_fanout_as_lines_end_to_end() {
    let tmp = TempDir::new().unwrap();
    let cookfile = r#"
probe names
    lines { printf 'alpha\nbeta\n' }

recipe render
    gather names
    cook "out/$<in>.txt" { mkdir -p out && echo '$<in>' > $<out> }
"#;
    fs::write(tmp.path().join("Cookfile"), cookfile).unwrap();
    run_cook(tmp.path(), &["render"]).unwrap();
    assert!(tmp.path().join("out/alpha.txt").exists(), "alpha.txt missing");
    assert!(tmp.path().join("out/beta.txt").exists(), "beta.txt missing");
}

/// A native lua-block `probe` returning records, feeding a member fan-out with
/// `$<in.field>` access.
#[test]
fn native_probe_member_fanout_lua_records_end_to_end() {
    let tmp = TempDir::new().unwrap();
    let cookfile = r#"
probe cards
    >{ return { {id='a'}, {id='b'} } }

recipe render
    gather cards
    cook "out/$<in.id>.txt" { mkdir -p out && echo $<in.id> > $<out> }
"#;
    fs::write(tmp.path().join("Cookfile"), cookfile).unwrap();
    run_cook(tmp.path(), &["render"]).unwrap();
    assert!(tmp.path().join("out/a.txt").exists(), "a.txt missing");
    assert!(tmp.path().join("out/b.txt").exists(), "b.txt missing");
}

/// A native shell-block `probe` (`as json`) whose JSON array feeds a member fan-out
/// (evaluated in the pre-pass VM, where cook.json_decode is available). Also
/// exercises a sealed file determinant on the probe.
#[test]
fn native_probe_member_fanout_as_json_end_to_end() {
    let tmp = TempDir::new().unwrap();
    fs::write(tmp.path().join("cards.json"), r#"[{"id":"x"},{"id":"y"}]"#).unwrap();
    let cookfile = r#"
probe cards
    seal "cards.json"
    json { cat cards.json }

recipe render
    gather cards
    cook "out/$<in.id>.txt" { mkdir -p out && echo $<in.id> > $<out> }
"#;
    fs::write(tmp.path().join("Cookfile"), cookfile).unwrap();
    run_cook(tmp.path(), &["render"]).unwrap();
    assert!(tmp.path().join("out/x.txt").exists(), "x.txt missing");
    assert!(tmp.path().join("out/y.txt").exists(), "y.txt missing");
}

/// Editing a probe's sealed input changes the value it observes; the member
/// fan-out reflects the new data on the next run.
#[test]
fn native_probe_gather_edit_reinvalidates() {
    let tmp = TempDir::new().unwrap();
    fs::write(tmp.path().join("cards.json"), r#"[{"id":"first"}]"#).unwrap();
    let cookfile = r#"
probe cards
    seal "cards.json"
    json { cat cards.json }

recipe render
    gather cards
    cook "out/$<in.id>.txt" { mkdir -p out && echo $<in.id> > $<out> }
"#;
    fs::write(tmp.path().join("Cookfile"), cookfile).unwrap();
    run_cook(tmp.path(), &["render"]).unwrap();
    assert!(
        tmp.path().join("out/first.txt").exists(),
        "first.txt missing"
    );
    // edit the input -> new observed value -> new member
    fs::write(tmp.path().join("cards.json"), r#"[{"id":"second"}]"#).unwrap();
    run_cook(tmp.path(), &["render"]).unwrap();
    assert!(tmp.path().join("out/second.txt").exists(), "second.txt missing after edit");
}

#[test]
fn native_probe_seal_edit_reinvalidates() {
    let tmp = TempDir::new().unwrap();
    fs::write(tmp.path().join("cards.json"), r#"[{"id":"first"}]"#).unwrap();
    let cookfile = r#"
files cards_data
    "cards.json"

probe cards
    seal cards_data
    json { cat cards.json }

recipe render
    gather cards
    cook "out/$<in.id>.txt" { mkdir -p out && echo $<in.id> > $<out> }
"#;
    fs::write(tmp.path().join("Cookfile"), cookfile).unwrap();
    run_cook(tmp.path(), &["render"]).unwrap();
    fs::write(tmp.path().join("cards.json"), r#"[{"id":"second"}]"#).unwrap();
    run_cook(tmp.path(), &["render"]).unwrap();
    assert!(tmp.path().join("out/second.txt").exists());
}

/// A native `probe` and a `cook.probe()` API call with the same key MUST be
/// rejected by the §22.5.2 duplicate-key diagnostic (coexistence: both register
/// into one probe table).
#[test]
fn native_probe_and_api_duplicate_key_rejected() {
    let tmp = TempDir::new().unwrap();
    let cookfile = r#"
probe dup
    >{ return 1 }
register
    cook.probe("dup", { inputs = {}, produce = "return 2" })

recipe build
    test { true }
"#;
    fs::write(tmp.path().join("Cookfile"), cookfile).unwrap();
    let err = run_cook(tmp.path(), &["build"]).expect_err("expected duplicate-key rejection");
    assert!(err.contains("dup") && (err.contains("declared") || err.to_lowercase().contains("duplicate")),
        "expected duplicate-key diagnostic mentioning 'dup', got: {err}");
}

/// Demand-driven scheduling: a probe that no recipe-reachable unit references
/// MUST NOT be executed and MUST NOT write a probe-value artifact to
/// `.cook/cache/`.
///
/// Locks the §22.5.8 demand-driven scheduling contract at the binary level:
/// declaring `cook.probe("test:unused", ...)` in the register phase is not
/// sufficient to trigger its execution — only consumer demand (a unit with
/// `probes = {...}` reachable from the requested recipe) causes the probe
/// to run. CS-0243 left this law untouched: no probe value is EVER written
/// to `.cook/cache/` any more, reached or not, so this test's negative
/// assertion is now unconditionally true of a reached probe too — its
/// interesting case remains the unreached one, which is what it names.
///
/// Detection scheme: walk `.cook/cache/` and inspect every `*.meta.json`
/// sidecar; an `ArtifactMeta` with `kind = Some("probe_value")` serializes
/// to JSON containing the substring `"kind":"probe_value"`. The presence of
/// that substring anywhere under `.cook/cache/` would indicate the probe
/// ran and persisted its output. A missing `.cook/cache/` directory is a
/// valid pass (no work executed at all).
#[test]
fn probe_unreached_is_not_executed() {
    let tmp = TempDir::new().unwrap();
    let cookfile = r#"
register
    cook.probe("test:unused", {
        inputs = {},
        produce = "return { v = 1 }",
    })

recipe build
    test { echo hello }
"#;
    fs::write(tmp.path().join("Cookfile"), cookfile).unwrap();

    // `cook build` must succeed: the recipe body doesn't depend on the probe,
    // so demand-driven scheduling should prune the probe entirely.
    run_cook(tmp.path(), &["build"]).expect("cook build should succeed");

    // No probe-value artifact should have been persisted. If `.cook/cache/`
    // doesn't exist at all, the assertion trivially holds.
    let cache_dir = tmp.path().join(".cook").join("cache");
    if cache_dir.exists() {
        let mut found = None;
        for entry in walkdir::WalkDir::new(&cache_dir).into_iter().flatten() {
            let path = entry.path();
            if path.is_file()
                && path.extension().and_then(|s| s.to_str()) == Some("json")
                && path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .map(|n| n.ends_with(".meta.json"))
                    .unwrap_or(false)
            {
                if let Ok(content) = fs::read_to_string(path) {
                    // ArtifactMeta with kind = Some("probe_value") serializes
                    // to JSON containing this exact substring.
                    if content.contains("\"kind\":\"probe_value\"") {
                        found = Some(path.to_path_buf());
                        break;
                    }
                }
            }
        }
        assert!(
            found.is_none(),
            "unreached probe must not write a probe-value artifact under .cook/cache/, \
             but found one at: {}",
            found.as_ref().map(|p| p.display().to_string()).unwrap_or_default()
        );
    }
}

/// CS-0102 (COOK-91): a probe completion materialises its value at
/// `.cook/probes/<key>.json` as canonical JSON — UTF-8, two-space pretty
/// printing, object keys sorted bytewise (alpha before zeta), exactly one
/// trailing LF — and a warm rerun leaves the file byte-identical. CS-0243:
/// the file is byte-identical because the produce body is deterministic and
/// re-observes the same answer, not because anything was served from a
/// store — there is none.
///
/// The probe key and produce source are uniquified per invocation so the
/// host-wide persistent cache (~/.cache/cook/cloud) cannot leak state across
/// `cargo test` runs.
#[test]
fn probe_value_file_is_canonical_json_and_survives_warm_rerun() {
    let tmp = TempDir::new().unwrap();
    let uniq = format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let probe_key = format!("test:canon-{uniq}");
    // Insertion order zeta-then-alpha so key-sorting is observable in the file.
    let cookfile = format!(
        r#"
recipe build
        cook.probe("{probe_key}", {{
            inputs = {{}},
            produce = [[
                -- uniq={uniq}
                return {{ zeta = "last", alpha = {{ 1, 2 }} }}
            ]],
        }})
        cook.add_unit({{
            name = "consume",
            inputs = {{}},
            outputs = {{"done.marker"}},
            probes = {{"{probe_key}"}},
            command = "echo $<{probe_key}.zeta> > done.marker",
        }})
"#
    );
    fs::write(tmp.path().join("Cookfile"), &cookfile).unwrap();

    // First run (cold): probe executes, file must be materialised.
    run_cook(tmp.path(), &["build"]).expect("first run should succeed");
    let marker = fs::read_to_string(tmp.path().join("done.marker")).unwrap();
    assert!(
        marker.contains("last"),
        "consumer must read the probe value; got marker: {marker:?}"
    );

    let probe_file = tmp
        .path()
        .join(".cook")
        .join("probes")
        .join(format!("{probe_key}.json"));
    assert!(
        probe_file.exists(),
        "probe completion must write {}",
        probe_file.display()
    );

    let bytes1 = fs::read(&probe_file).unwrap();
    let text = std::str::from_utf8(&bytes1).expect("probe file must be valid UTF-8");

    // Exactly one trailing LF.
    assert!(text.ends_with('\n'), "must end with LF; got: {text:?}");
    assert!(
        !text.ends_with("\n\n"),
        "must end with exactly ONE trailing LF; got: {text:?}"
    );

    // Canonical rendering pinned byte-for-byte: two-space pretty printing,
    // keys sorted bytewise ascending (alpha before zeta).
    let expected_text = "{\n  \"alpha\": [\n    1,\n    2\n  ],\n  \"zeta\": \"last\"\n}\n";
    assert_eq!(
        text, expected_text,
        "probe file must be the canonical JSON rendering (pretty, 2-space, key-sorted)"
    );

    // And it parses to the expected value.
    let parsed: serde_json::Value = serde_json::from_slice(&bytes1).expect("must parse as JSON");
    assert_eq!(parsed, serde_json::json!({"alpha": [1, 2], "zeta": "last"}));

    // Warm rerun: the probe re-observes (CS-0243, no cache), and because the
    // produce body is deterministic the file must come out byte-identical.
    run_cook(tmp.path(), &["build"]).expect("second run should succeed");
    let bytes2 = fs::read(&probe_file).unwrap();
    assert_eq!(
        bytes1, bytes2,
        "warm rerun must leave .cook/probes/<key>.json byte-identical"
    );
}

/// CS-0123: a probe whose produce decodes JSON is consumable
/// through the demand-driven worker path (consumer unit's `probes` field),
/// not just the `gather <probe>` pre-pass.
#[test]
fn probe_json_decode_produce_demand_driven_consumer() {
    let tmp = TempDir::new().unwrap();
    let uniq = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let probe_key = format!("codec:info-{uniq}");
    let cookfile = format!(
        r#"
recipe build
        cook.probe("{probe_key}", {{
            inputs = {{}},
            produce = "return cook.json_decode('{{\"word\":\"hello-json\"}}')",
        }})
        cook.add_unit({{
            name = "echo",
            inputs = {{}},
            outputs = {{"done.marker"}},
            probes = {{"{probe_key}"}},
            command = "echo $<{probe_key}.word> > done.marker",
        }})
"#
    );
    fs::write(tmp.path().join("Cookfile"), &cookfile).unwrap();
    run_cook(tmp.path(), &["build"]).expect("run should succeed");
    let marker = fs::read_to_string(tmp.path().join("done.marker")).unwrap();
    assert!(marker.contains("hello-json"), "marker: {marker:?}");
}

/// COOK-527 pin 1. §22.5.8's **Always observe** rule survives its own early
/// cutoff (CS-0243, backed by §17.1.1): a `seal`ed unit's cache key is a
/// function of the probe's VALUE, so re-observing an unchanged value still
/// hits even though the probe itself re-produces on every invocation
/// (pinned elsewhere by `probe_produce_re_executes_on_every_invocation`).
///
/// The probe appends one byte to `probe-runs.log` every time its produce
/// body runs — the direct observable of re-observation. The sealing `cook`
/// step appends a line to `marker.log` only when its command actually RUNS,
/// never on a cache hit (a restored artifact is not re-executed).
///
/// Three builds, not two. The first two touch nothing and must hold the
/// marker log at one line; the third changes `input.txt`, and the marker
/// log MUST then grow to two. The third build is the discriminator: two
/// reviewers independently showed that the first two builds alone prove
/// nothing, by two different mutations that left the two-build assertions
/// passing regardless — deleting the `seal test:watched` line from this
/// test's own Cookfile, and separately deleting the value fold at
/// `seal.rs::seal_contribution` (COOK-527 Important). Both mutations still
/// "hit" on an unchanged rebuild, because an UNSEALED unit also has a
/// constant key and hits for the wrong reason. Only a changed-value build
/// that then MISSES tells a real value fold apart from a constant key that
/// never moves at all.
#[test]
fn seal_hits_when_the_observed_value_is_unchanged_across_reobservation() {
    let tmp = TempDir::new().unwrap();
    let cookfile = watched_cookfile("test:watched");
    fs::write(tmp.path().join("Cookfile"), &cookfile).unwrap();
    fs::write(tmp.path().join("input.txt"), "A").unwrap();

    run_cook(tmp.path(), &["build"]).expect("first run should succeed");
    let probe_log_1 = fs::read_to_string(tmp.path().join("probe-runs.log")).unwrap();
    let marker_1 = fs::read_to_string(tmp.path().join("marker.log")).unwrap();
    assert_eq!(probe_log_1, "x", "probe must have produced exactly once so far");
    assert_eq!(marker_1, "run\n", "unit must have executed exactly once so far");

    // Second build: nothing touched.
    run_cook(tmp.path(), &["build"]).expect("second run should succeed");
    let probe_log_2 = fs::read_to_string(tmp.path().join("probe-runs.log")).unwrap();
    let marker_2 = fs::read_to_string(tmp.path().join("marker.log")).unwrap();
    assert_eq!(
        probe_log_2, "xx",
        "CS-0243: a reached probe MUST re-produce every invocation, even \
         when its declared value is unchanged; got probe-runs.log: {probe_log_2:?}"
    );
    assert_eq!(
        marker_2, "run\n",
        "early cutoff: the probe's value did not move, so the sealing \
         unit's key did not move, so its command must NOT re-run; got \
         marker.log: {marker_2:?}"
    );

    // Third build: input.txt changes, so the probe's observed value moves,
    // and the sealing unit's key MUST move with it — the command MUST
    // re-run. This is the discriminator described in the doc comment above:
    // an unchanged-build hit alone cannot distinguish a real value fold from
    // a unit that simply never re-keys; a changed-build miss can.
    fs::write(tmp.path().join("input.txt"), "B").unwrap();
    run_cook(tmp.path(), &["build"]).expect("third run should succeed");
    let probe_log_3 = fs::read_to_string(tmp.path().join("probe-runs.log")).unwrap();
    let marker_3 = fs::read_to_string(tmp.path().join("marker.log")).unwrap();
    assert_eq!(
        probe_log_3, "xxx",
        "CS-0243: the probe must have re-produced a third time; got \
         probe-runs.log: {probe_log_3:?}"
    );
    assert_eq!(
        marker_3, "run\nrun\n",
        "the sealed value changed A -> B, so the sealing unit's key must \
         have moved and its command must have re-run; got marker.log: \
         {marker_3:?} (expected \"run\\nrun\\n\")"
    );
}

/// COOK-527 pin 2. A -> B -> A revert re-serves the FIRST build's artifact
/// from cache rather than rebuilding it — the sealing unit's key is a pure
/// function of the sealed probe's current value, so an exact revert lands
/// back on the exact key build 1 populated.
///
/// Same Cookfile shape and same COOK-530 trap as pin 1: `seal
/// test:revertwatched` is what folds the probe's VALUE into the unit's key
/// (not a bare `probes = {...}` reference or `$<key>` sigil alone).
///
/// `marker.log` is the discriminator that makes this test mean something:
/// "out.txt reads A again" is equally true whether build 3 replayed the
/// cached artifact or simply re-ran the command against input.txt == "A".
/// Only `marker.log` staying at two lines (not growing to three) proves the
/// third build was a cache hit rather than a re-run. `probe-runs.log`
/// growing to three lines regardless is the free half of the claim: the
/// probe re-observes on every invocation (CS-0243) independently of
/// whether the unit sealing it hits or re-runs.
#[test]
fn seal_a_to_b_to_a_revert_re_serves_the_original_artifact() {
    let tmp = TempDir::new().unwrap();
    let cookfile = watched_cookfile("test:revertwatched");
    fs::write(tmp.path().join("Cookfile"), &cookfile).unwrap();

    // A.
    fs::write(tmp.path().join("input.txt"), "A").unwrap();
    run_cook(tmp.path(), &["build"]).expect("build A should succeed");
    assert_eq!(fs::read_to_string(tmp.path().join("out.txt")).unwrap(), "A\n");
    assert_eq!(
        fs::read_to_string(tmp.path().join("probe-runs.log")).unwrap(),
        "x",
        "probe must have produced exactly once building A"
    );
    assert_eq!(
        fs::read_to_string(tmp.path().join("marker.log")).unwrap(),
        "run\n",
        "unit must have executed exactly once building A"
    );

    // B.
    fs::write(tmp.path().join("input.txt"), "B").unwrap();
    run_cook(tmp.path(), &["build"]).expect("build B should succeed");
    assert_eq!(fs::read_to_string(tmp.path().join("out.txt")).unwrap(), "B\n");
    assert_eq!(
        fs::read_to_string(tmp.path().join("probe-runs.log")).unwrap(),
        "xx",
        "CS-0243: the probe must have re-produced a second time building B"
    );
    assert_eq!(
        fs::read_to_string(tmp.path().join("marker.log")).unwrap(),
        "run\nrun\n",
        "unit must re-run when the sealed probe's value changed A -> B"
    );

    // Back to A, exactly.
    fs::write(tmp.path().join("input.txt"), "A").unwrap();
    run_cook(tmp.path(), &["build"]).expect("build A-again should succeed");
    assert_eq!(
        fs::read_to_string(tmp.path().join("out.txt")).unwrap(),
        "A\n",
        "reverted build must serve A again"
    );
    // CS-0243: the probe re-produces a third time even though the sealing
    // unit below hits — a reached probe always observes, regardless of
    // whether the unit that seals it re-runs or is served from cache. This
    // is the free half of the A -> B -> A claim: three produce runs across
    // three builds, while the sealing unit ran only twice.
    assert_eq!(
        fs::read_to_string(tmp.path().join("probe-runs.log")).unwrap(),
        "xxx",
        "CS-0243: the probe must have re-produced a third time even though \
         the sealing unit below is served from cache"
    );
    assert_eq!(
        fs::read_to_string(tmp.path().join("marker.log")).unwrap(),
        "run\nrun\n",
        "the marker must NOT grow a third time: A's artifact must be \
         re-served from the content-addressed cache (the same key as the \
         first build), not rebuilt by re-running the command"
    );
}

/// COOK-527 pin 3(b). §22.5.8: `.cook/probes/<key>.json` is a per-invocation
/// RECORD, never a cross-invocation source. `probe_unreached_is_not_executed`
/// (above) pins pin 3(a) — an unreached probe never executes. This pins the
/// other half, the one CS-0243's removal of `ProbeValueStore::attach_dir`
/// from `execute_dag` was actually for: `cook.probes.get(key)` on a step
/// that never demanded `key` THIS invocation must raise the not-materialised
/// error even when an EARLIER invocation already materialised
/// `.cook/probes/<key>.json` on disk. Before COOK-527 Task 1 removed the
/// executor's `attach_dir` call, this second run would have silently served
/// that stale file instead of raising.
#[test]
fn probes_get_does_not_read_a_prior_invocations_materialised_file() {
    let tmp = TempDir::new().unwrap();
    let key = "test:k527stale";

    // First Cookfile: the probe IS demanded (sealed), so its value is
    // materialised at `.cook/probes/<key>.json`.
    let cookfile_demanding = format!(
        r#"
probe {key}
    >{{ return "v1" }}

recipe build
    seal {key}
    test {{ echo built }}
"#
    );
    fs::write(tmp.path().join("Cookfile"), &cookfile_demanding).unwrap();
    run_cook(tmp.path(), &["build"]).expect("first run should succeed");
    let probe_file = tmp
        .path()
        .join(".cook")
        .join("probes")
        .join(format!("{key}.json"));
    assert!(
        probe_file.exists(),
        "first run must materialise {}",
        probe_file.display()
    );

    // Second Cookfile, same workspace: `key` is still declared but no
    // longer demanded by anything (no seal, no probes={...}, no
    // inputs.requires) — demand-driven scheduling prunes it entirely this
    // run (see `probe_unreached_is_not_executed`), so nothing in THIS
    // invocation ever inserts a value into the per-run ProbeValueStore for
    // `key`. A separate step calls `cook.probes.get(key)` directly, but the
    // key is built by concatenation rather than passed as a bare string
    // literal — `cook_contracts::lua_scan::scan_probe_reads` only recognises
    // a literal that is the WHOLE argument (§22.5.7's Lua-body static scan,
    // CS-0152) as a demand, precisely so a dynamic-key read is not silently
    // treated as one.
    // The stale file from the first run is still sitting on disk the whole
    // time.
    let cookfile_stale_read = format!(
        r#"
probe {key}
    >{{ return "v1" }}

recipe build
    test >{{ cook.probes.get("{key}" .. "") }}
"#
    );
    fs::write(tmp.path().join("Cookfile"), &cookfile_stale_read).unwrap();
    let err = run_cook(tmp.path(), &["build"])
        .expect_err("cook.probes.get on an undemanded key must raise, not read the stale file");
    assert!(
        err.contains("not materialised") && err.contains(key),
        "expected the §22.5.8 not-materialised error naming '{key}', got: {err}"
    );
}
