//! COOK-187 end-to-end: probe-value references in NATIVE cook-step bodies
//! (Standard §22.5.7 / §8.4). Regression: luagen emitted the command as a
//! `function() ... end` closure which cook.add_unit coerced to "" — the unit
//! "ran" 0.00s, produced nothing, exited 0.

use std::fs;
use std::path::Path;
use std::process::Command;

fn cook_binary() -> std::path::PathBuf {
    let mut path = std::env::current_exe().unwrap();
    path.pop(); // .../target/debug/deps -> .../target/debug
    path.pop();
    path.push("cook");
    assert!(
        path.exists(),
        "cook binary not found at {} — run `cargo build -p cook-cli` first",
        path.display()
    );
    path
}

/// Private cache dir so runs never touch ~/.cache/cook/cloud (shared).
fn write_cloud_toml(wd: &Path, cache_dir: &Path) {
    fs::create_dir_all(wd.join(".cook")).unwrap();
    fs::write(
        wd.join(".cook/cloud.toml"),
        format!("[cache]\ncache_dir = {:?}\n", cache_dir.to_string_lossy()),
    )
    .unwrap();
}

fn run_cook(wd: &Path, target: &str) -> std::process::Output {
    Command::new(cook_binary())
        .arg(target)
        .current_dir(wd)
        .output()
        .expect("cook invocation")
}

#[test]
fn probe_ref_in_native_cook_body_substitutes_value() {
    let tmp = tempfile::tempdir().unwrap();
    let wd = tmp.path();
    let cache = wd.join("cache");
    write_cloud_toml(wd, &cache);
    fs::create_dir_all(wd.join("out")).unwrap();
    fs::write(
        wd.join("Cookfile"),
        r#"probe cook187a:val
    { printf 'zesty-e2e-value' }

recipe withprobe
    cook "out/b.txt" { echo "$<cook187a:val>" > $<out> }
"#,
    )
    .unwrap();

    let out = run_cook(wd, "withprobe");
    assert!(
        out.status.success(),
        "cook withprobe failed:\n{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    let b = wd.join("out/b.txt");
    assert!(b.exists(), "out/b.txt was not produced — probe-in-body silently no-op'd");
    let content = fs::read_to_string(&b).unwrap();
    assert_eq!(
        content.trim(),
        "zesty-e2e-value",
        "probe value must be substituted into the body; got: {content:?}"
    );
}

#[test]
fn probe_ref_mixed_with_dep_output_ref_in_one_body() {
    // Spec: a body containing BOTH a dep ref and a probe ref failed wholesale
    // (the closure carried the whole command). Both must substitute.
    let tmp = tempfile::tempdir().unwrap();
    let wd = tmp.path();
    let cache = wd.join("cache");
    write_cloud_toml(wd, &cache);
    fs::create_dir_all(wd.join("out")).unwrap();
    fs::write(
        wd.join("Cookfile"),
        r#"probe cook187b:val
    { printf 'probe-part' }

recipe gen
    cook "out/a.txt" { printf 'dep-part' > $<out> }

recipe combined: gen
    cook "out/c.txt" { printf '%s %s' "$(cat $<gen>)" "$<cook187b:val>" > $<out> }
"#,
    )
    .unwrap();

    let out = run_cook(wd, "combined");
    assert!(
        out.status.success(),
        "cook combined failed:\n{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    let c = wd.join("out/c.txt");
    assert!(c.exists(), "out/c.txt was not produced");
    let content = fs::read_to_string(&c).unwrap();
    assert_eq!(content.trim(), "dep-part probe-part", "got: {content:?}");
}

// CS-0152: Lua-body-only probe consumption. A `cook.add_unit` whose only
// probe reference lives inside a `>{ ... }` Lua payload (no `$<key>` sigil
// anywhere in a shell body) used to silently read `nil` — the static
// $<key> scanner that demand-schedules probes never looked inside Lua
// code, so the probe was never materialised before the consumer ran.
// The fix adds a companion static scanner over `lua_code` that recognizes
// literal `cook.probes.get("<key>")` calls and unions the keys into the
// unit's `probes` field, exactly as the shell-sigil scanner does for
// `$<key>`. The two tests below pin the resulting contract:
//   - a literal key is found by the scanner, demand-scheduled, and reads
//     back the real value (`lua_only_probes_get_demands_and_reads_value`);
//   - a dynamically-computed key is invisible to the scanner and, because
//     nothing else demands the probe, execute-phase `cook.probes.get` now
//     hard-errors instead of quietly returning nil
//     (`dynamic_probes_get_on_undemanded_key_hard_errors`).

#[test]
fn lua_only_probes_get_demands_and_reads_value() {
    let tmp = tempfile::tempdir().unwrap();
    let wd = tmp.path();
    let cache = wd.join("cache");
    write_cloud_toml(wd, &cache);
    fs::create_dir_all(wd.join("out")).unwrap();
    fs::write(
        wd.join("Cookfile"),
        r#"probe c217:x
    { printf 'c217-value' }

recipe luaonly
    cook "out/val.txt" >{
        local v = cook.probes.get("c217:x")
        fs.write(output, tostring(v))
    }
"#,
    )
    .unwrap();

    let out = run_cook(wd, "luaonly");
    assert!(
        out.status.success(),
        "cook luaonly failed:\n{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    let f = wd.join("out/val.txt");
    assert!(
        f.exists(),
        "out/val.txt was not produced — Lua-only probe consumption silently no-op'd"
    );
    let content = fs::read_to_string(&f).unwrap();
    assert_eq!(
        content.trim(),
        "c217-value",
        "probe value read via cook.probes.get() in a Lua-only body must be the real \
         materialised value, not nil; got: {content:?}"
    );
    assert_ne!(
        content.trim(),
        "nil",
        "regression: probe was never demand-scheduled, so cook.probes.get() returned nil"
    );
}

#[test]
fn dynamic_probes_get_on_undemanded_key_hard_errors() {
    let tmp = tempfile::tempdir().unwrap();
    let wd = tmp.path();
    let cache = wd.join("cache");
    write_cloud_toml(wd, &cache);
    fs::create_dir_all(wd.join("out")).unwrap();
    fs::write(
        wd.join("Cookfile"),
        r#"probe c217:x
    { printf 'c217-value' }

recipe dyn
    cook "out/dyn.txt" >{
        local k = "c217:" .. "x"
        local v = cook.probes.get(k)
        fs.write(output, tostring(v))
    }
"#,
    )
    .unwrap();

    let out = run_cook(wd, "dyn");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !out.status.success(),
        "cook dyn was expected to fail (dynamic key is invisible to the static scan, \
         so the probe is never demanded and must hard-error instead of reading nil):\n{combined}"
    );
    assert!(
        combined.contains("c217:x"),
        "error output must name the undemanded probe key 'c217:x':\n{combined}"
    );
    assert!(
        combined.contains("not materialised"),
        "error output must say the probe value was not materialised:\n{combined}"
    );
}

// ─── CS-0240: a colon-free probe key is reachable from a sigil (COOK-491) ───

#[test]
fn bare_probe_key_in_native_cook_body_substitutes_value() {
    // COOK-491. The native `probe` declaration mints colon-free keys, but
    // CS-0074 dispatched a sigil to the probe path only on a colon, so this
    // Cookfile failed at register with `no config block declares 'keyed_obs'`
    // — a probe declared three lines up, never mentioned by the diagnostic.
    let tmp = tempfile::tempdir().unwrap();
    let wd = tmp.path();
    let cache = wd.join("cache");
    write_cloud_toml(wd, &cache);
    fs::create_dir_all(wd.join("out")).unwrap();
    fs::write(
        wd.join("Cookfile"),
        r#"probe keyed_obs
    { printf 'observed-by-name' }

recipe show
    cook "out/b.txt" { echo "$<keyed_obs>" > $<out> }
"#,
    )
    .unwrap();

    let out = run_cook(wd, "show");
    assert!(
        out.status.success(),
        "cook show failed:\n{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    let b = wd.join("out/b.txt");
    assert!(b.exists(), "out/b.txt was not produced");
    assert_eq!(
        fs::read_to_string(&b).unwrap().trim(),
        "observed-by-name",
        "a bare probe key must substitute exactly as a colon-prefixed one does"
    );
}

#[test]
fn bare_probe_key_is_demanded_by_the_consumer() {
    // COOK-491: resolving by name is not enough — the key must also union into
    // the unit's `probes` list, or the consumer runs with no DAG edge and keys
    // on nothing the probe observed. §22.5.8 makes that observable: a probe no
    // scheduled unit demands MUST NOT execute and MUST be omitted from the
    // DAG, so the probe node appearing in the run is the edge.
    let tmp = tempfile::tempdir().unwrap();
    let wd = tmp.path();
    let cache = wd.join("cache");
    write_cloud_toml(wd, &cache);
    fs::create_dir_all(wd.join("out")).unwrap();
    fs::write(
        wd.join("Cookfile"),
        r#"probe demanded_by_name
    { printf 'v' }

recipe show
    cook "out/b.txt" { echo "$<demanded_by_name>" > $<out> }
"#,
    )
    .unwrap();

    let out = run_cook(wd, "show");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let log = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        log.contains("demanded_by_name"),
        "the probe must be scheduled as a node of this recipe — an undemanded \
         probe is silently omitted (§22.5.8), so its absence is the missing edge:\n{log}"
    );
}

#[test]
fn probe_shadowing_a_declared_var_warns_naming_both() {
    // COOK-491 / §10.2.3. CS-0240 chose the shadowing WARNING over a hard
    // error for probe-vs-var, because a config-block variable has no
    // declaration site a parser can name. That choice is only defensible if
    // the warning fires, and it did not: `warn_var_shadowing` intersected the
    // declared vars with recipe names alone. The scenario it left silent: a
    // config block exports `var.tag`, someone adds `probe tag`, and `$<tag>`
    // flips from the declared string to the observed value with no diagnostic.
    let tmp = tempfile::tempdir().unwrap();
    let wd = tmp.path();
    let cache = wd.join("cache");
    write_cloud_toml(wd, &cache);
    fs::create_dir_all(wd.join("out")).unwrap();
    fs::write(
        wd.join("Cookfile"),
        r#"config
    var.tag = "v1.0"

probe tag
    { printf 'observed' }

recipe show
    cook "out/b.txt" { echo "$<tag>" > $<out> }
"#,
    )
    .unwrap();

    let out = run_cook(wd, "show");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("probe 'tag' shadows declared var 'tag'"),
        "§10.2.3 requires a warning naming both; got:\n{err}"
    );
    // And the substitution really is the probe's, which is what makes the
    // warning worth emitting rather than a formality.
    assert_eq!(fs::read_to_string(wd.join("out/b.txt")).unwrap().trim(), "observed");
}
