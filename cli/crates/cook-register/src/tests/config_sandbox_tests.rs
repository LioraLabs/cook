use super::*;

/// Build a VM whose single function executes `body` under the config
/// sandbox, writing into a fresh output table (exposed to the body as the
/// `var` sink). Returns the VM (kept alive so callers can read the output
/// afterward), the call result, the output table, and the recorded reads.
/// `body` is the raw config-body Lua.
fn run_config(body: &str) -> (Lua, LuaResult<()>, LuaTable, Vec<HostRead>) {
    let lua = Lua::new();
    let out = lua.create_table().unwrap();
    let reads: SharedHostReads = Rc::new(RefCell::new(Vec::new()));
    let sandbox =
        build_config_sandbox_env(&lua, &out, Path::new("."), &reads).unwrap();

    let func = lua
        .load(format!("return function()\n{body}\nend"))
        .eval::<LuaFunction>()
        .unwrap();
    assert!(
        func.set_environment(sandbox).unwrap(),
        "config function must carry an _ENV upvalue to sandbox"
    );
    let result = func.call::<()>(());
    let captured = reads.borrow().clone();
    (lua, result, out, captured)
}

#[test]
fn rejects_os() {
    let (_lua, res, _out, _reads) =
        run_config(r#"var.X = os.getenv("HOME") or "d""#);
    let err = format!("{}", res.unwrap_err());
    assert!(err.contains("'os'"), "diagnostic must name os: {err}");
    assert!(err.contains("5.3"), "diagnostic must cite §5.3: {err}");
}

#[test]
fn rejects_io() {
    let (_lua, res, _out, _reads) = run_config(r#"var.X = io.open("f")"#);
    let err = format!("{}", res.unwrap_err());
    assert!(err.contains("'io'"), "diagnostic must name io: {err}");
}

#[test]
fn rejects_env_output_with_did_you_mean() {
    // The output sink is now `var` (CS-0164). A refugee writing `env.X`
    // reads the absent `env` global, which the sandbox traps with a
    // did-you-mean diagnostic rather than a cryptic nil-index error.
    let (_lua, res, _out, _reads) = run_config(r#"env.X = "y""#);
    let err = format!("{}", res.unwrap_err());
    assert!(err.contains("var."), "diagnostic must point at var.: {err}");
    assert!(
        err.contains("did you mean") || err.contains("§5.3.1"),
        "diagnostic must read as a did-you-mean: {err}"
    );
}

#[test]
fn var_sink_writes_reach_output() {
    let (_lua, res, out, _reads) = run_config(
        r#"
            var.CC = "gcc"
            var.CFLAGS = "-O2 " .. "-Wall"
            "#,
    );
    res.unwrap();
    assert_eq!(out.get::<String>("CC").unwrap(), "gcc");
    assert_eq!(out.get::<String>("CFLAGS").unwrap(), "-O2 -Wall");
}

#[test]
fn var_read_back_of_prior_value_works() {
    // `var.X = var.X or default` — reading back an unset sink key yields
    // nil (ordinary table read), so the `or` fallback applies.
    let (_lua, res, out, _reads) =
        run_config(r#"var.X = var.X or "fallback""#);
    res.unwrap();
    assert_eq!(out.get::<String>("X").unwrap(), "fallback");
}

#[test]
fn rejects_cook() {
    // `cook` is not in the sandbox, so `cook.platform` traps as a nil
    // index on the absent global (cook is not in the banned-with-hint set,
    // it simply does not exist).
    let (_lua, res, _out, _reads) = run_config(r#"var.X = cook.platform.os"#);
    assert!(res.is_err(), "cook.* must not be reachable in a config body");
}

#[test]
fn host_os_and_arch_resolve_and_record() {
    let (_lua, res, out, reads) = run_config(
        r#"
            var.OS = host.os
            var.ARCH = host.arch
            "#,
    );
    res.unwrap();
    let os: String = out.get("OS").unwrap();
    let arch: String = out.get("ARCH").unwrap();
    assert_eq!(os, std::env::consts::OS);
    assert_eq!(arch, std::env::consts::ARCH);
    assert!(reads.iter().any(|r| r.kind == HostReadKind::Os));
    assert!(reads.iter().any(|r| r.kind == HostReadKind::Arch));
}

#[test]
fn host_env_reads_with_default_and_records() {
    std::env::set_var("COOK_TEST_HOSTENV", "present");
    let (_lua, res, out, reads) = run_config(
        r#"
            var.PRESENT = host.env("COOK_TEST_HOSTENV", "fallback")
            var.MISSING = host.env("COOK_TEST_HOSTENV_UNSET", "fallback")
            "#,
    );
    res.unwrap();
    std::env::remove_var("COOK_TEST_HOSTENV");
    let present: String = out.get("PRESENT").unwrap();
    let missing: String = out.get("MISSING").unwrap();
    assert_eq!(present, "present");
    assert_eq!(missing, "fallback");
    assert_eq!(
        reads.iter().filter(|r| r.kind == HostReadKind::Env).count(),
        2
    );
}

#[test]
fn host_read_reads_relative_file_and_records() {
    let dir = std::env::temp_dir().join(format!(
        "cook-cfgsandbox-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("version.txt"), "9.9.9\n").unwrap();

    let lua = Lua::new();
    let out = lua.create_table().unwrap();
    let reads: SharedHostReads = Rc::new(RefCell::new(Vec::new()));
    let sandbox = build_config_sandbox_env(&lua, &out, &dir, &reads).unwrap();
    let func = lua
        .load("return function()\nvar.V = host.read(\"version.txt\")\nend")
        .eval::<LuaFunction>()
        .unwrap();
    func.set_environment(sandbox).unwrap();
    func.call::<()>(()).unwrap();

    let v: String = out.get("V").unwrap();
    assert_eq!(v, "9.9.9\n");
    assert!(reads.borrow().iter().any(|r| r.kind == HostReadKind::Read));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn pure_control_flow_and_string_methods_work() {
    let (_lua, res, output, _reads) = run_config(
        r#"
            local parts = {}
            for i = 1, 3 do
                parts[#parts + 1] = tostring(i)
            end
            local joined = table.concat(parts, "-")
            var.OUT = (host.os == "" and "?" or joined):upper()
            "#,
    );
    res.unwrap();
    let out: String = output.get("OUT").unwrap();
    assert_eq!(out, "1-2-3");
}

#[test]
fn math_random_is_removed() {
    let (_lua, res, _out, _reads) =
        run_config(r#"var.X = tostring(math.random())"#);
    // math.random was dropped, so this is a call on a nil value.
    assert!(res.is_err(), "math.random must not be available");
}

#[test]
fn undefined_non_banned_global_is_nil() {
    // A plain undefined global resolves to nil (normal Lua), so a guard
    // like `if maybe then` does not spuriously error.
    let (_lua, res, output, _reads) = run_config(
        r#"
            if some_undefined_flag then
                var.OUT = "yes"
            else
                var.OUT = "no"
            end
            "#,
    );
    res.unwrap();
    let out: String = output.get("OUT").unwrap();
    assert_eq!(out, "no");
}
