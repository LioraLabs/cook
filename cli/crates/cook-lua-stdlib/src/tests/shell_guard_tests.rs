use super::*;
use crate::sandbox::SandboxPolicy;
use std::sync::{Arc, Mutex};

/// Confined VM: os.execute MUST raise a Lua error mentioning
/// CS-0045. We don't run the original — the guard is the only
/// caller of os.execute on the test path.
#[test]
fn confined_os_execute_raises() {
    let lua = Lua::new();
    install_shell_escape_guards(
        &lua,
        SandboxSource::confined(std::path::PathBuf::from("/proj")),
    )
    .unwrap();
    let err = lua
        .load(r#"os.execute("echo escape")"#)
        .exec()
        .unwrap_err()
        .to_string();
    assert!(err.contains("shell escape hatch is disabled"), "missing guard text: {err}");
    assert!(err.contains("os.execute"), "missing api name: {err}");
}

/// Off VM: os.execute MUST be a no-op pass-through. Use a harmless
/// command (`true` on POSIX) so the test does not depend on the
/// host having any specific binary at a specific path.
#[test]
fn off_os_execute_passes_through() {
    let lua = Lua::new();
    install_shell_escape_guards(&lua, SandboxSource::off()).unwrap();
    // `true` exits 0; we don't assert on the return value because
    // mlua's coercion of multi-return values varies by version.
    // The point is: it MUST NOT raise.
    lua.load(r#"os.execute("true")"#).exec().unwrap();
}

/// io.popen behaves the same way under Confined.
#[test]
fn confined_io_popen_raises() {
    let lua = Lua::new();
    install_shell_escape_guards(
        &lua,
        SandboxSource::confined(std::path::PathBuf::from("/proj")),
    )
    .unwrap();
    let err = lua
        .load(r#"return io.popen("echo x")"#)
        .exec()
        .unwrap_err()
        .to_string();
    assert!(err.contains("shell escape hatch is disabled"), "missing guard text: {err}");
    assert!(err.contains("io.popen"), "missing api name: {err}");
}

/// Live source observes per-item policy changes. The same VM
/// flips from rejecting to permitting based on slot mutation.
#[test]
fn live_source_flips_per_call() {
    let lua = Lua::new();
    let slot = Arc::new(Mutex::new(SandboxPolicy::Confined {
        project_root: std::path::PathBuf::from("/proj"),
    }));
    install_shell_escape_guards(&lua, SandboxSource::Live(Arc::clone(&slot))).unwrap();

    // First call: confined, MUST raise.
    let err = lua
        .load(r#"os.execute("true")"#)
        .exec()
        .unwrap_err()
        .to_string();
    assert!(err.contains("shell escape hatch is disabled"));

    // Flip to Off; same VM, same closure.
    *slot.lock().unwrap() = SandboxPolicy::Off;
    lua.load(r#"os.execute("true")"#).exec().unwrap();
}
