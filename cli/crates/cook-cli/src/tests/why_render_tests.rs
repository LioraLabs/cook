use cook_engine::why::DeterminantDiff;

use super::{determinant_diff_json, render_diff};

/// `diff_against_manifest` (cook-engine) only reaches the `Probe` arm when
/// `seal_contribution` matches but the raw per-probe strings differ — see
/// `cook-engine/src/tests/why_tests.rs::diff_names_a_sealed_probe_value_difference`,
/// whose fixture is deliberately synthetic (a real key match implies the
/// fold that produced it matched too). This module's own black-box tests
/// (`tests/why_render.rs`) exercise the local-miss delta end to end
/// through the real binary; the shared-miss arm gets the same synthetic
/// fixture the engine test uses, so the rendering law is pinned at the
/// same altitude the diff itself is.
fn probe_diff() -> DeterminantDiff {
    DeterminantDiff::Probe {
        key: "host".into(),
        ours: Some("\"aarch64\"".into()),
        theirs: Some("\"x86_64\"".into()),
    }
}

/// CS-0245 / §17.1.6.1 acceptance criterion, in the shared-miss arm's own
/// words: "instead of dumping the whole sealed value". The pre-CS-0245
/// rendering was `probe host: ours Some("\"aarch64\"") != producer
/// Some("\"x86_64\"")` — the whole value, twice, `Debug`-escaped.
#[test]
fn shared_miss_probe_diff_renders_through_the_delta_law_not_a_debug_dump() {
    let text = render_diff(&probe_diff());
    assert!(text.contains("host"), "probe key must be named: {text}");
    assert!(text.contains("aarch64"), "{text}");
    assert!(text.contains("x86_64"), "{text}");
    // The old rendering wrapped each side in `Some(...)` via `{:?}` on an
    // `Option<String>`. Neither belongs in the delta rendering.
    assert!(!text.contains("Some("), "must not Debug-dump: {text}");
}

#[test]
fn shared_miss_probe_diff_json_carries_the_structured_delta_alongside_the_old_fields() {
    let v = determinant_diff_json(&probe_diff());
    // CS-0245: additive, no schema bump — the fields a pre-CS-0245
    // consumer already parses are untouched.
    assert_eq!(v["determinant"], "probe:host");
    assert_eq!(v["ours"], "\"aarch64\"");
    assert_eq!(v["producer"], "\"x86_64\"");
    // The new structured delta sits alongside them.
    assert_eq!(v["delta"]["kind"], "value");
    assert_eq!(v["delta"]["old"], "x86_64");
    assert_eq!(v["delta"]["new"], "aarch64");
}
