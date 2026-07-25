use super::*;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

/// Tests that mutate `COOK_CLOUD_API_KEY` via `std::env::set_var` /
/// `remove_var` MUST hold this lock for their duration. Rust's parallel
/// test runner otherwise races, causing flakes where one test's
/// `set_var` is still visible to another's `resolved_api_key()` call.
static ENV_LOCK: Mutex<()> = Mutex::new(());

fn write_toml(dir: &Path, contents: &str) -> PathBuf {
    let cook_dir = dir.join(".cook");
    std::fs::create_dir_all(&cook_dir).expect("mkdir");
        let path = cook_dir.join("cloud.toml");
    let mut f = std::fs::File::create(&path).expect("create");
    f.write_all(contents.as_bytes()).expect("write");
    path
}

#[test]
fn missing_file_returns_default() {
    let dir = tempfile::tempdir().expect("tempdir");
        let cfg = CloudConfig::load_or_default(dir.path()).expect("load");
    assert!(!cfg.cloud.enabled);
    assert_eq!(cfg.project_id_or_fallback(dir.path()), dir.path().file_name().unwrap().to_string_lossy());
    assert!(cfg.cache_ignore_env().is_empty());
}

#[test]
fn cloud_disabled_no_project_required() {
    let dir = tempfile::tempdir().expect("tempdir");
        write_toml(dir.path(), r#"
[cloud]
enabled = false
"#);
    let cfg = CloudConfig::load_or_default(dir.path()).expect("load");
    assert!(!cfg.cloud.enabled);
    // No project required when disabled.
}

#[test]
fn cloud_enabled_requires_project() {
    let dir = tempfile::tempdir().expect("tempdir");
        write_toml(dir.path(), r#"
[cloud]
enabled = true
endpoint = "https://api.cook.dev"
"#);
    let result = CloudConfig::load_or_default(dir.path());
    assert!(result.is_err(), "missing project must error when cloud.enabled=true");
}

#[test]
fn cloud_enabled_with_project_ok() {
    // CS-0058 + CS-0059: cloud-enabled requires endpoint and a
    // resolvable env-var api_key. CS-0059 removed the TOML api_key
    // field; resolution is env-var-only now.
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // SAFETY: tests touching COOK_CLOUD_API_KEY hold ENV_LOCK; this
    // serialises set/remove across them.
    unsafe { std::env::set_var("COOK_CLOUD_API_KEY", "env-tok-12345"); }
    let dir = tempfile::tempdir().expect("tempdir");
        write_toml(dir.path(), r#"
[cloud]
enabled = true
endpoint = "https://api.cook.dev"
project = "cook"
"#);
    let cfg = CloudConfig::load_or_default(dir.path()).expect("load");
    assert!(cfg.cloud.enabled);
    assert_eq!(cfg.cloud.project.as_deref(), Some("cook"));
    assert_eq!(cfg.resolved_api_key().as_deref(), Some("env-tok-12345"));
    unsafe { std::env::remove_var("COOK_CLOUD_API_KEY"); }
}

#[test]
fn cache_ignore_env_parsed() {
    let dir = tempfile::tempdir().expect("tempdir");
        write_toml(dir.path(), r#"
[cache]
ignore_env = ["GITHUB_TOKEN", "MY_API_KEY"]
"#);
    let cfg = CloudConfig::load_or_default(dir.path()).expect("load");
    let ignore = cfg.cache_ignore_env();
    assert_eq!(ignore.len(), 2);
    assert!(ignore.contains(&"GITHUB_TOKEN".to_string()));
    assert!(ignore.contains(&"MY_API_KEY".to_string()));
}

#[test]
fn malformed_toml_errors() {
    let dir = tempfile::tempdir().expect("tempdir");
        write_toml(dir.path(), "this is not valid toml === ");
    assert!(CloudConfig::load_or_default(dir.path()).is_err());
}

#[test]
fn project_id_or_fallback_uses_dir_name_when_no_project() {
    let dir = tempfile::tempdir().expect("tempdir");
        let project_dir = dir.path().join("my-cool-project");
    std::fs::create_dir_all(&project_dir).expect("mkdir");
        let cfg = CloudConfig::load_or_default(&project_dir).expect("load");
    assert_eq!(cfg.project_id_or_fallback(&project_dir), "my-cool-project");
}

// ─── CS-0057: BackendConfig threading ───────────────────────────────────

/// An empty `[cloud]` section produces a `BackendConfig` exactly equal
/// to `BackendConfig::default()` — the no-tunables identity.
#[test]
fn backend_config_uses_defaults_when_unset() {
    let dir = tempfile::tempdir().expect("tempdir");
        let cfg = CloudConfig::load_or_default(dir.path()).expect("load");
    let bc = cfg.backend_config();
    let def = BackendConfig::default();
    assert_eq!(bc.timeout, def.timeout);
    assert_eq!(bc.max_retries, def.max_retries);
    assert_eq!(bc.backoff_initial, def.backoff_initial);
    assert_eq!(bc.backoff_max, def.backoff_max);
    assert_eq!(bc.max_artifact_bytes, def.max_artifact_bytes);
}

/// All five `[cloud]` knobs override the corresponding
/// `BackendConfig` fields with the user-provided values, including
/// the `_secs` / `_ms` / `_mib` unit conversions.
#[test]
fn backend_config_overrides_from_toml() {
    let dir = tempfile::tempdir().expect("tempdir");
        write_toml(dir.path(), r#"
[cloud]
timeout_secs = 90
max_retries = 7
backoff_initial_ms = 250
backoff_max_ms = 12000
max_artifact_mib = 256
"#);
    let cfg = CloudConfig::load_or_default(dir.path()).expect("load");
    let bc = cfg.backend_config();
    assert_eq!(bc.timeout, Duration::from_secs(90));
    assert_eq!(bc.max_retries, 7);
    assert_eq!(bc.backoff_initial, Duration::from_millis(250));
    assert_eq!(bc.backoff_max, Duration::from_millis(12_000));
    assert_eq!(bc.max_artifact_bytes, 256u64 * 1024 * 1024);
}

// ─── CS-0058: api_key validation + resolution ─────────────────────────

/// `cloud.enabled = true` with project + endpoint but neither an
/// `api_key` TOML field nor `COOK_CLOUD_API_KEY` env var → `MissingApiKey`.
#[test]
fn cloud_enabled_requires_api_key() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // SAFETY: we hold ENV_LOCK; no other test in this module mutates
    // COOK_CLOUD_API_KEY without taking the lock.
    unsafe { std::env::remove_var("COOK_CLOUD_API_KEY"); }
    let dir = tempfile::tempdir().expect("tempdir");
        write_toml(dir.path(), r#"
[cloud]
enabled = true
endpoint = "https://api.cook.dev"
project = "cook"
"#);
    let result = CloudConfig::load_or_default(dir.path());
    match result {
        Err(CloudConfigError::MissingApiKey) => {}
        other => panic!("expected MissingApiKey, got: {other:?}"),
    }
}

/// `cloud.enabled = true` with project + endpoint + `COOK_CLOUD_API_KEY`
/// env var (no TOML `api_key`) → validation passes; `resolved_api_key`
/// returns the env-var value.
#[test]
fn cloud_enabled_uses_env_var_api_key() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // SAFETY: ENV_LOCK serialises COOK_CLOUD_API_KEY mutation.
    unsafe { std::env::set_var("COOK_CLOUD_API_KEY", "env-tok-9999"); }
    let dir = tempfile::tempdir().expect("tempdir");
        write_toml(dir.path(), r#"
[cloud]
enabled = true
endpoint = "https://api.cook.dev"
project = "cook"
"#);
    let cfg = CloudConfig::load_or_default(dir.path()).expect("load");
    assert_eq!(cfg.resolved_api_key().as_deref(), Some("env-tok-9999"));
    // Cleanup so subsequent tests don't see this env var.
    unsafe { std::env::remove_var("COOK_CLOUD_API_KEY"); }
}

/// `cloud.enabled = true` without an endpoint → `MissingEndpoint`,
/// even with the api_key env var set.
#[test]
fn cloud_enabled_requires_endpoint() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    unsafe { std::env::set_var("COOK_CLOUD_API_KEY", "tok-for-endpoint-test"); }
    let dir = tempfile::tempdir().expect("tempdir");
        write_toml(dir.path(), r#"
[cloud]
enabled = true
project = "cook"
"#);
    let result = CloudConfig::load_or_default(dir.path());
    match result {
        Err(CloudConfigError::MissingEndpoint) => {}
        other => panic!("expected MissingEndpoint, got: {other:?}"),
    }
    unsafe { std::env::remove_var("COOK_CLOUD_API_KEY"); }
}

/// CS-0059. Empty env var (`COOK_CLOUD_API_KEY=""`) is treated as
/// unset; resolution returns None and `load_or_default` errors with
/// `MissingApiKey` for cloud-enabled configs. Pre-CS-0059 this case
/// fell through to a `[cloud] api_key` TOML field; that field is gone.
#[test]
fn cloud_empty_env_var_treated_as_unset() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    unsafe { std::env::set_var("COOK_CLOUD_API_KEY", ""); }
    let dir = tempfile::tempdir().expect("tempdir");
        write_toml(dir.path(), r#"
[cloud]
enabled = true
endpoint = "https://api.cook.dev"
project = "cook"
"#);
    let result = CloudConfig::load_or_default(dir.path());
    match result {
        Err(CloudConfigError::MissingApiKey) => {}
        other => panic!("expected MissingApiKey, got: {other:?}"),
    }
    // And in the disabled-cloud case, the empty env var is just None
    // — no error path is even invoked.
    let cfg = CloudConfig::default();
    assert_eq!(cfg.resolved_api_key(), None);
    unsafe { std::env::remove_var("COOK_CLOUD_API_KEY"); }
}

/// CS-0059. Stray `[cloud] api_key = "..."` lines that pre-date
/// CS-0059 deserialise cleanly because serde ignores unknown fields
/// by default — no `#[serde(deny_unknown_fields)]` on `CloudSection`.
/// A user upgrading from CS-0058 sees the field silently ignored;
/// resolution falls back to env-var-only and surfaces `MissingApiKey`
/// if the env var is unset, prompting the user to migrate.
#[test]
fn legacy_toml_api_key_field_silently_ignored() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    unsafe { std::env::set_var("COOK_CLOUD_API_KEY", "env-takes-precedence"); }
    let dir = tempfile::tempdir().expect("tempdir");
        write_toml(dir.path(), r#"
[cloud]
enabled = true
endpoint = "https://api.cook.dev"
project = "cook"
api_key = "stale-toml-secret-should-be-ignored"
"#);
    let cfg = CloudConfig::load_or_default(dir.path()).expect("legacy field is ignored, not rejected");
    assert_eq!(cfg.resolved_api_key().as_deref(), Some("env-takes-precedence"));
    unsafe { std::env::remove_var("COOK_CLOUD_API_KEY"); }
}

// ─── COOK-168: [cloud] publish field ─────────────────────────────────────

#[test]
fn publish_defaults_to_true_when_absent() {
    let dir = tempfile::tempdir().expect("tempdir");
        let cfg = CloudConfig::load_or_default(dir.path()).expect("load");
    assert!(cfg.publish(), "publish must default to true when [cloud] publish is unset");
}

#[test]
fn publish_false_parsed() {
    let dir = tempfile::tempdir().expect("tempdir");
        write_toml(dir.path(), r#"
[cloud]
publish = false
"#);
    let cfg = CloudConfig::load_or_default(dir.path()).expect("load");
    assert!(!cfg.publish(), "publish = false must parse to false");
}

#[test]
fn publish_off_does_not_require_cloud_enabled() {
    // publish-off is orthogonal to cloud.enabled; a publish=false config with
    // cloud disabled loads cleanly (no project/endpoint required).
    let dir = tempfile::tempdir().expect("tempdir");
        write_toml(dir.path(), r#"
[cloud]
publish = false
"#);
    assert!(CloudConfig::load_or_default(dir.path()).is_ok());
}

// ─── COOK-232: [cache] max_size byte-budget parsing ──────────────────────

#[test]
fn max_size_absent_yields_none() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cfg = CloudConfig::load_or_default(dir.path()).expect("load");
    assert_eq!(cfg.max_size_bytes().expect("parse"), None);
}

#[test]
fn max_size_decimal_units() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_toml(
        dir.path(),
        r#"
[cache]
max_size = "20GB"
"#,
    );
    let cfg = CloudConfig::load_or_default(dir.path()).expect("load");
    assert_eq!(cfg.max_size_bytes().expect("parse"), Some(20_000_000_000));
}

#[test]
fn max_size_binary_units() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_toml(
        dir.path(),
        r#"
[cache]
max_size = "20GiB"
"#,
    );
    let cfg = CloudConfig::load_or_default(dir.path()).expect("load");
    assert_eq!(cfg.max_size_bytes().expect("parse"), Some(21_474_836_480));
}

#[test]
fn max_size_lowercase_and_whitespace() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_toml(
        dir.path(),
        r#"
[cache]
max_size = "512mb"
"#,
    );
    let cfg = CloudConfig::load_or_default(dir.path()).expect("load");
    assert_eq!(cfg.max_size_bytes().expect("parse"), Some(512_000_000));

    let dir2 = tempfile::tempdir().expect("tempdir");
    write_toml(
        dir2.path(),
        r#"
[cache]
max_size = "512 MB"
"#,
    );
    let cfg2 = CloudConfig::load_or_default(dir2.path()).expect("load");
    assert_eq!(cfg2.max_size_bytes().expect("parse"), Some(512_000_000));
}

#[test]
fn max_size_fractional_truncates_toward_zero() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_toml(
        dir.path(),
        r#"
[cache]
max_size = "1.5GB"
"#,
    );
    let cfg = CloudConfig::load_or_default(dir.path()).expect("load");
    assert_eq!(cfg.max_size_bytes().expect("parse"), Some(1_500_000_000));
}

#[test]
fn max_size_bare_number_is_bytes() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_toml(
        dir.path(),
        r#"
[cache]
max_size = "4096"
"#,
    );
    let cfg = CloudConfig::load_or_default(dir.path()).expect("load");
    assert_eq!(cfg.max_size_bytes().expect("parse"), Some(4096));
}

#[test]
fn max_size_unparseable_literal_errors_naming_it() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_toml(
        dir.path(),
        r#"
[cache]
max_size = "twenty gigs"
"#,
    );
    let cfg = CloudConfig::load_or_default(dir.path()).expect("load");
    let err = cfg.max_size_bytes().expect_err("should error");
    let msg = err.to_string();
    assert!(
        msg.contains("twenty gigs"),
        "error message must name the offending literal, got: {msg}"
    );
}

#[test]
fn max_size_negative_literal_errors_naming_it() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_toml(
        dir.path(),
        r#"
[cache]
max_size = "-5GB"
"#,
    );
    let cfg = CloudConfig::load_or_default(dir.path()).expect("load");
    let err = cfg.max_size_bytes().expect_err("should error");
    let msg = err.to_string();
    assert!(
        msg.contains("-5GB"),
        "error message must name the offending literal, got: {msg}"
    );
}

/// A signed-zero literal (`-0`) must not slip past the negative check by
/// evaluating `-0.0 < 0.0` as false. A leading `-` is rejected outright.
#[test]
fn max_size_negative_zero_errors_naming_it() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_toml(
        dir.path(),
        r#"
[cache]
max_size = "-0"
"#,
    );
    let cfg = CloudConfig::load_or_default(dir.path()).expect("load");
    let err = cfg.max_size_bytes().expect_err("should error");
    let msg = err.to_string();
    assert!(
        msg.contains("-0"),
        "error message must name the offending literal, got: {msg}"
    );
}

/// Same signed-zero case with a unit suffix attached.
#[test]
fn max_size_negative_zero_with_unit_errors_naming_it() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_toml(
        dir.path(),
        r#"
[cache]
max_size = "-0GB"
"#,
    );
    let cfg = CloudConfig::load_or_default(dir.path()).expect("load");
    let err = cfg.max_size_bytes().expect_err("should error");
    let msg = err.to_string();
    assert!(
        msg.contains("-0GB"),
        "error message must name the offending literal, got: {msg}"
    );
}

/// A leading `+` is rejected too — a budget literal never needs an
/// explicit sign, so this is intended, not a regression.
#[test]
fn max_size_explicit_plus_sign_errors_naming_it() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_toml(
        dir.path(),
        r#"
[cache]
max_size = "+5GB"
"#,
    );
    let cfg = CloudConfig::load_or_default(dir.path()).expect("load");
    let err = cfg.max_size_bytes().expect_err("should error");
    let msg = err.to_string();
    assert!(
        msg.contains("+5GB"),
        "error message must name the offending literal, got: {msg}"
    );
}

/// A value that overflows `u64` must error, not silently saturate to
/// `u64::MAX` (which would read as "effectively no budget").
#[test]
fn max_size_overflow_unit_errors_naming_it() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_toml(
        dir.path(),
        r#"
[cache]
max_size = "999999999TB"
"#,
    );
    let cfg = CloudConfig::load_or_default(dir.path()).expect("load");
    let err = cfg.max_size_bytes().expect_err("should error");
    let msg = err.to_string();
    assert!(
        msg.contains("999999999TB"),
        "error message must name the offending literal, got: {msg}"
    );
}

/// A bare-byte literal one past `u64::MAX` must also error rather than
/// silently saturate.
#[test]
fn max_size_overflow_bare_number_errors_naming_it() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_toml(
        dir.path(),
        r#"
[cache]
max_size = "18446744073709551616"
"#,
    );
    let cfg = CloudConfig::load_or_default(dir.path()).expect("load");
    let err = cfg.max_size_bytes().expect_err("should error");
    let msg = err.to_string();
    assert!(
        msg.contains("18446744073709551616"),
        "error message must name the offending literal, got: {msg}"
    );
}

#[test]
fn max_size_absent_does_not_change_load_or_default_behaviour() {
    // A cloud.toml that omits [cache] max_size still loads exactly as
    // before COOK-232 — same validation, same defaults.
    let dir = tempfile::tempdir().expect("tempdir");
    write_toml(
        dir.path(),
        r#"
[cache]
ignore_env = ["GITHUB_TOKEN"]
"#,
    );
    let cfg = CloudConfig::load_or_default(dir.path()).expect("load");
    assert_eq!(cfg.cache_ignore_env(), &["GITHUB_TOKEN".to_string()]);
    assert_eq!(cfg.max_size_bytes().expect("parse"), None);
}

// ─── auto_gc: opt-in sweep vs. warn-only default ─────────────────────────

/// Milestone decision D4: `auto_gc` defaults to `false` (warn-only) across
/// all three "unset" shapes — no cloud.toml file at all, an empty
/// `[cache]` section, and an absent `auto_gc` key alongside other cache
/// settings. This default must never silently flip.
#[test]
fn auto_gc_defaults_to_false_when_file_absent() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cfg = CloudConfig::load_or_default(dir.path()).expect("load");
    assert!(!cfg.auto_gc(), "auto_gc must default to false with no cloud.toml at all");
}

#[test]
fn auto_gc_defaults_to_false_when_cache_section_empty() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_toml(
        dir.path(),
        r#"
[cache]
"#,
    );
    let cfg = CloudConfig::load_or_default(dir.path()).expect("load");
    assert!(!cfg.auto_gc(), "auto_gc must default to false with an empty [cache] section");
}

#[test]
fn auto_gc_defaults_to_false_when_key_absent() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_toml(
        dir.path(),
        r#"
[cache]
max_size = "20GB"
"#,
    );
    let cfg = CloudConfig::load_or_default(dir.path()).expect("load");
    assert!(
        !cfg.auto_gc(),
        "auto_gc must default to false when absent, even alongside other [cache] keys"
    );
}

#[test]
fn auto_gc_true_parses_true() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_toml(
        dir.path(),
        r#"
[cache]
auto_gc = true
"#,
    );
    let cfg = CloudConfig::load_or_default(dir.path()).expect("load");
    assert!(cfg.auto_gc(), "auto_gc = true must parse to true");
}

#[test]
fn auto_gc_false_parses_false() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_toml(
        dir.path(),
        r#"
[cache]
auto_gc = false
"#,
    );
    let cfg = CloudConfig::load_or_default(dir.path()).expect("load");
    assert!(!cfg.auto_gc(), "auto_gc = false must parse to false");
}

/// A non-boolean `auto_gc` must surface as a TOML type error through the
/// existing `CloudConfigError::Parse` path, not silently coerce to false.
#[test]
fn auto_gc_non_boolean_is_parse_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_toml(
        dir.path(),
        r#"
[cache]
auto_gc = "yes"
"#,
    );
    let result = CloudConfig::load_or_default(dir.path());
    match result {
        Err(CloudConfigError::Parse(_)) => {}
        other => panic!("expected Parse error for non-boolean auto_gc, got: {other:?}"),
    }
}

/// `auto_gc = true` with no `max_size` set must not be a load-time error:
/// no budget means no check, so the sweep simply never runs. The two
/// knobs are deliberately uncoupled at load time.
#[test]
fn auto_gc_true_without_max_size_is_not_an_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_toml(
        dir.path(),
        r#"
[cache]
auto_gc = true
"#,
    );
    let cfg = CloudConfig::load_or_default(dir.path()).expect("auto_gc=true with no max_size must load fine");
    assert!(cfg.auto_gc());
    assert_eq!(cfg.max_size_bytes().expect("parse"), None);
}

// ─── max_size_literal: verbatim echo for remediation commands ───────────

#[test]
fn max_size_literal_absent_yields_none() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cfg = CloudConfig::load_or_default(dir.path()).expect("load");
    assert_eq!(cfg.max_size_literal(), None);
}

/// `max_size_literal` must return the exact string the user typed, not a
/// re-rendered/normalised form — "20GB" must stay "20GB", not become
/// "20 GB" or a byte count.
#[test]
fn max_size_literal_round_trips_verbatim() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_toml(
        dir.path(),
        r#"
[cache]
max_size = "20GB"
"#,
    );
    let cfg = CloudConfig::load_or_default(dir.path()).expect("load");
    assert_eq!(cfg.max_size_literal(), Some("20GB"));
}

/// `max_size_literal` returns the literal verbatim even when it's not a
/// valid size — it's a raw echo, not a validated accessor. Validation is
/// `max_size_bytes()`'s job.
#[test]
fn max_size_literal_round_trips_even_when_invalid() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_toml(
        dir.path(),
        r#"
[cache]
max_size = "twenty gigs"
"#,
    );
    let cfg = CloudConfig::load_or_default(dir.path()).expect("load");
    assert_eq!(cfg.max_size_literal(), Some("twenty gigs"));
    assert!(cfg.max_size_bytes().is_err());
}
