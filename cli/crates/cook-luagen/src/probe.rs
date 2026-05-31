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
    out.push_str("  },\n");
    let produce_src = lower_produce(&probe.produce);
    out.push_str(&format!("  produce = {},\n", wrap_lua_string(&produce_src)));
    out.push_str("})\n\n");
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
    }
}
