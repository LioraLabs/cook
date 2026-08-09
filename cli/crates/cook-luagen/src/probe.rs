use cook_contracts::lua_string;
use cook_lang::ast::{Probe, ProbeProduce, ShellProduceType, UseStatement};

use crate::long_bracket::wrap_lua_string;

/// Emit `cook.probe(key, { inputs = {...}, produce = "..." })` for one native
/// `probe` declaration. Pure surface sugar over the register-phase API
/// (§22.5.2); the runtime is unchanged.
pub(crate) fn emit_probe(out: &mut String, probe: &Probe, uses: &[UseStatement]) {
    out.push_str(&format!(
        "cook.probe(\"{}\", {{\n",
        lua_string::escape_double_quoted(&probe.name)
    ));
    out.push_str("  inputs = {\n");
    if !probe.ingredients.is_empty() || !probe.excludes.is_empty() {
        let inc = probe
            .ingredients
            .iter()
            .map(|s| lua_string::literal(s))
            .collect::<Vec<_>>()
            .join(", ");
        let exc = probe
            .excludes
            .iter()
            .map(|s| lua_string::literal(s))
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
            .map(|s| lua_string::literal(s))
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
    let produce_src = lower_produce(&probe.produce, uses);
    out.push_str(&format!("  produce = {},\n", wrap_lua_string(&produce_src)));
    out.push_str("})\n\n");
}

/// Render a name list as a comma-separated Lua array body: `["a","b"]` → `"a", "b"`.
/// Names are validated bare IDENTs upstream, but escape defensively.
fn quoted_list(names: &[String]) -> String {
    names
        .iter()
        .map(|s| lua_string::literal(s))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Lower a `ProbeProduce` to the Lua source string the `produce` field carries
/// (the body of the producing function — `cook.probe` wraps it in
/// `function() ... end`). Uses only existing worker APIs (`cook.sh`,
/// `cook.json_decode`), so the runtime is unchanged.
fn lower_produce(p: &ProbeProduce, uses: &[UseStatement]) -> String {
    match p {
        // CS-0205: a probe's `produce` body is execute-phase Lua like any
        // other, so a `use` alias it names is bound the same way. The other
        // arms are generated Lua that can never name a user alias, and the
        // `files { }` and `tools { }` arms MUST stay byte-identical to their
        // reserved sentinels — `cook-probe` compares them by equality to
        // intercept the producer.
        ProbeProduce::Lua(code) => crate::use_prelude::with_execute_prelude(uses, code),
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
        // CS-0214: the second reserved sentinel, for the same reason as the
        // first. The engine synthesises `{ NAME = { hash } }` from the probe's
        // resolved `inputs.tools` (see emit_probe) — the same name→content-hash
        // pairs the fingerprint's TOOLS section folds — so the re-run trigger
        // and the value are one computation.
        //
        // Until CS-0214 this arm emitted a Lua program: `command -v` to
        // resolve, `sha256sum … | cut -d' ' -f1` to digest. That made the
        // producer a second implementation of an identity the fingerprint
        // already computed, in a different language, with a different
        // resolver, at a different moment in the run, agreeing only because
        // both happened to land on lowercase-hex SHA-256. It also could not
        // run at all on a host without GNU coreutils: stock macOS has
        // `shasum`, not `sha256sum`.
        //
        // CS-0157 (COOK-277) still holds and is now structural rather than
        // remembered: the synthesised value carries identity only. Path is
        // location, and folding it into the canonical bytes poisoned
        // seal_contribution with a machine-specific string, so identical
        // toolchains at different locations (homebrew vs /usr/bin, nix stores)
        // could never share sealed artifacts. Path rides the engine's per-run
        // tool metadata channel instead: `cook.probes.get` merges a
        // freshly-resolved `path` into the READ view and `cook why` displays it
        // from the same channel.
        ProbeProduce::Tools(_) => cook_contracts::probe_value::TOOLS_IDENTITY_PRODUCE.to_string(),
        ProbeProduce::Envs(names) => {
            // CS-0172: read the AMBIENT PROCESS environment via `os.getenv`.
            // An `envs { }` probe is the specced channel for making a host
            // environment value a keyed determinant (§22.5.2), so it must read
            // the process environment — not the declared-variable namespace.
            // Before CS-0172 it read `cook.env`, which was both at once; a
            // config block could therefore silently redefine what an `envs`
            // probe recorded about the host. The re-run trigger is the declared
            // `inputs.env` (see emit_probe). An unset var assigns nil, which Lua
            // never stores as a table key, so the key is OMITTED from the
            // resulting JSON object (§22.5.2).
            let mut out = String::from("local _e = {}\n");
            for name in names {
                // `name` is a validated bare IDENT, so a quoted-string key is
                // safe. A long-bracket `[[name]]` would be ambiguous as a table
                // index — `_e[[[name]]]`.
                out.push_str(&format!(
                    "_e[\"{}\"] = os.getenv(\"{}\")\n",
                    lua_string::escape_double_quoted(name),
                    lua_string::escape_double_quoted(name)
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
