use cook_lang::ast::{Probe, ProbeProduce, ShellProduceType};

use crate::lua_string::{escape_lua_string, wrap_lua_string};

/// Emit `cook.probe(key, { inputs = {...}, produce = "..." })` for one native
/// `probe` declaration. Pure surface sugar over the register-phase API
/// (§22.5.2); the runtime is unchanged.
pub(crate) fn emit_probe(out: &mut String, probe: &Probe) {
    out.push_str(&format!(
        "cook.probe(\"{}\", {{\n",
        escape_lua_string(&probe.name)
    ));
    out.push_str("  inputs = {\n");
    if !probe.ingredients.is_empty() || !probe.excludes.is_empty() {
        let inc = probe
            .ingredients
            .iter()
            .map(|s| format!("\"{}\"", escape_lua_string(s)))
            .collect::<Vec<_>>()
            .join(", ");
        let exc = probe
            .excludes
            .iter()
            .map(|s| format!("\"{}\"", escape_lua_string(s)))
            .collect::<Vec<_>>()
            .join(", ");
        out.push_str(&format!(
            "    files = cook.resolve_ingredients({{{}}}, {{{}}}),\n",
            inc, exc
        ));
    }
    if !probe.deps.is_empty() {
        let reqs = probe
            .deps
            .iter()
            .map(|s| format!("\"{}\"", escape_lua_string(s)))
            .collect::<Vec<_>>()
            .join(", ");
        out.push_str(&format!("    requires = {{{}}},\n", reqs));
    }
    // COOK-164: `tools { … }` / `envs { … }` declares the named tools/env-vars
    // as probe inputs so the fingerprint machinery (resolve_probe_inputs) folds
    // each tool's binary hash / each env value into the probe fingerprint. This
    // is what makes the hash/value the re-run trigger — the produce body only
    // computes the VALUE; the determinant lives in these declared inputs.
    match &probe.produce {
        ProbeProduce::Tools(names) => {
            out.push_str(&format!("    tools = {{{}}},\n", quoted_list(names)));
        }
        ProbeProduce::Envs(names) => {
            out.push_str(&format!("    env = {{{}}},\n", quoted_list(names)));
        }
        // CS-0148: `files { … }` declares its glob set as `inputs.files` —
        // register-time glob resolution, each file's content hash folding into
        // the fingerprint. The parser guarantees a `files` probe has no
        // `ingredients` line, so this is the only `files =` emission.
        ProbeProduce::Files { globs, excludes } => {
            out.push_str(&format!(
                "    files = cook.resolve_ingredients({{{}}}, {{{}}}),\n",
                quoted_list(globs),
                quoted_list(excludes),
            ));
        }
        ProbeProduce::Lua(_) | ProbeProduce::Shell { .. } => {}
    }
    out.push_str("  },\n");
    let produce_src = lower_produce(&probe.produce);
    out.push_str(&format!("  produce = {},\n", wrap_lua_string(&produce_src)));
    out.push_str("})\n\n");
}

/// Render a name list as a comma-separated Lua array body: `["a","b"]` → `"a", "b"`.
/// Names are validated bare IDENTs upstream, but escape defensively.
fn quoted_list(names: &[String]) -> String {
    names
        .iter()
        .map(|s| format!("\"{}\"", escape_lua_string(s)))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Lower a `ProbeProduce` to the Lua source string the `produce` field carries
/// (the body of the producing function — `cook.probe` wraps it in
/// `function() ... end`). Uses only existing worker APIs (`cook.sh`,
/// `cook.json_decode`), so the runtime is unchanged.
fn lower_produce(p: &ProbeProduce) -> String {
    match p {
        ProbeProduce::Lua(code) => code.clone(),
        ProbeProduce::Shell { commands, typing } => {
            let script = commands.join("\n");
            let sh = format!("cook.sh({})", wrap_lua_string(&script));
            match typing {
                ShellProduceType::String => {
                    format!("return ({sh}:gsub(\"\\n$\", \"\"))")
                }
                ShellProduceType::Json => {
                    format!("return cook.json_decode({sh})")
                }
                ShellProduceType::Lines => format!(
                    "local _o = {sh}\nlocal _r = {{}}\nfor _l in _o:gmatch(\"[^\\n]+\") do _r[#_r + 1] = _l end\nreturn _r"
                ),
            }
        }
        ProbeProduce::Tools(names) => {
            // Build the VALUE `{ NAME = { path, hash } }` by resolving via
            // `command -v` and hashing via `sha256sum`. The re-run TRIGGER is
            // the declared `inputs.tools` (see emit_probe), which the
            // fingerprint machinery resolves + hashes independently; the
            // resolved path stays in the value for COOK-165 `cook why`.
            let mut out = String::from("local _t = {}\n");
            for name in names {
                let resolve = format!("command -v {name}");
                let resolve_sh = format!("cook.sh({})", wrap_lua_string(&resolve));
                out.push_str(&format!(
                    "do\n  local _p = ({resolve_sh}):gsub(\"\\n$\", \"\")\n"
                ));
                out.push_str(&format!(
                    "  if _p == \"\" then error(\"tools probe: '{name}' not found on PATH\") end\n"
                ));
                // sha256sum '<path>' | cut -d' ' -f1. Escape any `'` in the
                // resolved path for the single-quoted shell argument
                // (`'` → `'\''`) so paths with quotes can't break out.
                out.push_str(
                    "  local _pq = _p:gsub(\"'\", \"'\\\\''\")\n",
                );
                out.push_str(
                    "  local _h = (cook.sh(\"sha256sum '\" .. _pq .. \"' | cut -d' ' -f1\")):gsub(\"\\n$\", \"\")\n",
                );
                // `name` is a validated bare IDENT, so a quoted-string key is
                // safe (a long-bracket `[[name]]` would be ambiguous as a table
                // index — `_t[[[name]]]`).
                out.push_str(&format!(
                    "  _t[\"{}\"] = {{ path = _p, hash = _h }}\n",
                    escape_lua_string(name)
                ));
                out.push_str("end\n");
            }
            out.push_str("return _t");
            out
        }
        ProbeProduce::Envs(names) => {
            // Read each env var via cook.env.<NAME> for the VALUE. The re-run
            // trigger is the declared `inputs.env` (see emit_probe). An unset
            // var assigns nil, which Lua never stores as a table key, so the
            // key is OMITTED from the resulting JSON object (§22.5.2).
            let mut out = String::from("local _e = {}\n");
            for name in names {
                // Quoted-string key (see Tools arm); `name` is a bare IDENT.
                out.push_str(&format!(
                    "_e[\"{}\"] = cook.env.{name}\n",
                    escape_lua_string(name)
                ));
            }
            out.push_str("return _e");
            out
        }
        // CS-0148: the reserved sentinel — not Lua, never dispatched to a
        // worker. The engine synthesises the value `{ [path] = hash }` from
        // the probe's resolved `inputs.files` (see emit_probe), the same
        // pairs the fingerprint folds, so trigger and value cannot drift and
        // the keys stay workspace-relative (portable across machines).
        ProbeProduce::Files { .. } => {
            cook_contracts::probe_value::FILES_MANIFEST_PRODUCE.to_string()
        }
    }
}
