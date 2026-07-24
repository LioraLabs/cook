//! Sandboxed environment for `config` block Lua bodies (Standard §5.3.2,
//! CS-0163).
//!
//! A `config` block is a **pure function** from a small set of declared host
//! reads to a named-value namespace: the only side effect a config body may
//! produce is writing `var.*` outputs (CS-0164), and the only external input
//! it may read is the `host.*` surface. Everything
//! that would let a config body observe ambient nondeterminism or reach outside
//! that contract — `os`, `io`, clocks, randomness, process spawning, filesystem
//! writes, module loading, `debug`, coroutines — is removed by construction.
//!
//! # Mechanism
//!
//! The whole register/recipe pass runs in one `Lua::unsafe_new()` VM, and
//! recipe and register bodies legitimately keep the full standard library
//! (`os.execute` in a recipe body is a supported escape hatch). So the sandbox
//! CANNOT strip globals from the VM. Instead it swaps the `_ENV` upvalue of the
//! single generated `__cook_run_config_blocks` function (via
//! [`mlua::Function::set_environment`]) for the restricted table built here.
//! `_ENV` governs only *free* (global) variable lookups; captured locals — a
//! `use`-imported module handle such as `cook_cc`, bound as a top-level
//! `local` and closed over by the config function — are upvalues and stay
//! reachable. The dispatcher applies this in
//! [`crate::engine::dispatch_config_blocks`].
//!
//! # Reads are provenance, not a second cache channel
//!
//! Each `host.*` read is recorded as a [`HostRead`]. Cache correctness for a
//! host-varying config value is already carried by consulted-value hashing (a
//! step keys on the resolved `$<NAME>` value it consults), so these records are
//! for provenance / purity attribution (a future `cook why`), NOT a second
//! keying channel (milestone §E). Recording is therefore a lightweight capture
//! with no cache plumbing.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use mlua::prelude::*;

/// Which `host.*` accessor produced a recorded read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostReadKind {
    /// `host.os`
    Os,
    /// `host.arch`
    Arch,
    /// `host.env(name, default)`
    Env,
    /// `host.read(path)`
    Read,
}

/// One `host.*` read observed while a config body executed.
///
/// `key` is the OS/arch identifier itself for [`HostReadKind::Os`] /
/// [`HostReadKind::Arch`], the env-var name for [`HostReadKind::Env`], and the
/// requested path for [`HostReadKind::Read`]. `value` is the string the
/// accessor returned (for `Read`, the file contents).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostRead {
    pub kind: HostReadKind,
    pub key: String,
    pub value: String,
}

/// Session-scoped sink the `host.*` accessors push reads onto.
pub type SharedHostReads = Rc<RefCell<Vec<HostRead>>>;

/// Globals that are deliberately unavailable inside a config body. Accessing
/// any of them raises a clear diagnostic (rather than the cryptic "index a nil
/// value") so an author who reaches for `os.getenv` is pointed at `host.*`.
const BANNED_GLOBALS: &[&str] = &[
    "os",
    "io",
    "print",
    "require",
    "load",
    "loadstring",
    "dofile",
    "loadfile",
    "package",
    "debug",
    "coroutine",
    "collectgarbage",
];

/// Pure globals copied verbatim from the host VM into the sandbox. Every entry
/// is deterministic and side-effect-free (no clock, no randomness, no I/O).
/// `math` is handled separately (its `random`/`randomseed` are dropped).
const PURE_GLOBALS: &[&str] = &[
    "string",
    "table",
    "tostring",
    "tonumber",
    "type",
    "pairs",
    "ipairs",
    "next",
    "select",
    "error",
    "assert",
    "pcall",
    "xpcall",
    "rawget",
    "rawset",
    "rawequal",
    "rawlen",
    "setmetatable",
    "getmetatable",
    "_VERSION",
];

/// Build the sandboxed `_ENV` table for config-block execution.
///
/// `output_env` is the live `cook.env` table; it is exposed to the body as the
/// global `var` (the output sink, CS-0164). `working_dir` roots relative
/// `host.read` paths. Every `host.*` read is pushed onto `reads`.
pub fn build_config_sandbox_env(
    lua: &Lua,
    output_env: &LuaTable,
    working_dir: &Path,
    reads: &SharedHostReads,
) -> LuaResult<LuaTable> {
    let sandbox = lua.create_table()?;

    // Output sink (CS-0164, Standard §5.3.1). A config body declares named
    // values by assignment onto this `var` table; `$<NAME>` later resolves
    // `var.NAME`. `env` is deliberately NOT a sink — reading it raises the
    // did-you-mean diagnostic in the metatable below.
    sandbox.set("var", output_env.clone())?;

    // The one external-input surface.
    sandbox.set("host", build_host_table(lua, working_dir, reads)?)?;

    // Pure standard-library surface, copied from the host VM.
    let globals = lua.globals();
    for name in PURE_GLOBALS {
        let v: LuaValue = globals.get(*name)?;
        sandbox.set(*name, v)?;
    }
    // `math` minus the nondeterministic members.
    if let Ok(math) = globals.get::<LuaTable>("math") {
        let filtered = lua.create_table()?;
        for pair in math.pairs::<LuaValue, LuaValue>() {
            let (k, v) = pair?;
            if let Some(name) = k.as_str() {
                if name.as_ref() == "random" || name.as_ref() == "randomseed" {
                    continue;
                }
            }
            filtered.set(k, v)?;
        }
        sandbox.set("math", filtered)?;
    }

    // Trap the banned globals: a missing raw key that names one of them raises
    // a §5.3.2 diagnostic; a read of `env` raises the §5.3.1 did-you-mean
    // diagnostic (the output sink is now `var`, CS-0164); any other missing
    // global resolves to nil, preserving ordinary Lua semantics
    // (`if maybe_undefined then ...`).
    let meta = lua.create_table()?;
    meta.set(
        "__index",
        lua.create_function(|_, (_t, key): (LuaTable, String)| {
            if key.as_str() == "env" {
                return Err(mlua::Error::runtime(
                    "config outputs use `var.NAME = ...`, not `env.NAME = ...` \
                     (Standard §5.3.1): the config output namespace is `var` — \
                     did you mean var.?"
                        .to_string(),
                ));
            }
            if BANNED_GLOBALS.contains(&key.as_str()) {
                return Err(mlua::Error::runtime(format!(
                    "config block is sandboxed (Standard §5.3): '{key}' is not \
                     available — a config body may read the host only through \
                     host.os / host.arch / host.env(name, default) / \
                     host.read(path), and write outputs via var.*"
                )));
            }
            Ok(LuaValue::Nil)
        })?,
    )?;
    sandbox.set_metatable(Some(meta));

    Ok(sandbox)
}

/// The `host` table: `env` / `read` accessor functions (recording on call) plus
/// `os` / `arch` fields resolved through a `__index` metamethod so a field read
/// is recorded too.
fn build_host_table(
    lua: &Lua,
    working_dir: &Path,
    reads: &SharedHostReads,
) -> LuaResult<LuaTable> {
    let host = lua.create_table()?;

    // host.env(name, default) — raw field; records the read on call.
    {
        let sink = reads.clone();
        host.set(
            "env",
            lua.create_function(
                move |_, (name, default): (String, Option<String>)| {
                    let value = std::env::var(&name).ok().or(default);
                    sink.borrow_mut().push(HostRead {
                        kind: HostReadKind::Env,
                        key: name,
                        value: value.clone().unwrap_or_default(),
                    });
                    Ok(value)
                },
            )?,
        )?;
    }

    // host.read(path) — raw field; records the read on call.
    {
        let sink = reads.clone();
        let wd: PathBuf = working_dir.to_path_buf();
        host.set(
            "read",
            lua.create_function(move |_, path: String| {
                let target = {
                    let p = Path::new(&path);
                    if p.is_absolute() {
                        p.to_path_buf()
                    } else {
                        wd.join(p)
                    }
                };
                let contents = std::fs::read_to_string(&target).map_err(|e| {
                    mlua::Error::runtime(format!("host.read({path:?}): {e}"))
                })?;
                sink.borrow_mut().push(HostRead {
                    kind: HostReadKind::Read,
                    key: path,
                    value: contents.clone(),
                });
                Ok(contents)
            })?,
        )?;
    }

    // host.os / host.arch — resolved (and recorded) through __index. `env` and
    // `read` are raw fields, so Lua finds them without consulting __index; only
    // the os/arch misses reach here.
    let meta = lua.create_table()?;
    {
        let sink = reads.clone();
        meta.set(
            "__index",
            lua.create_function(move |_, (_host, key): (LuaTable, String)| {
                let (kind, value) = match key.as_str() {
                    "os" => (HostReadKind::Os, std::env::consts::OS.to_string()),
                    "arch" => (HostReadKind::Arch, std::env::consts::ARCH.to_string()),
                    _ => return Ok(None),
                };
                sink.borrow_mut().push(HostRead {
                    kind,
                    key: value.clone(),
                    value: value.clone(),
                });
                Ok(Some(value))
            })?,
        )?;
    }
    host.set_metatable(Some(meta));

    Ok(host)
}

#[cfg(test)]
#[path = "tests/config_sandbox_tests.rs"]
mod tests;
