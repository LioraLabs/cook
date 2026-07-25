//! CLI variable overrides (`--set KEY=VALUE`).
//!
//! CS-0172: the declared-variable namespace is exactly what a Cookfile's
//! `config` blocks write to the `var` sink. It is NOT the process
//! environment. A build therefore starts with an empty namespace; config
//! blocks populate it, and `--set` overrides a name one of them declared.
//!
//! Before CS-0172 this module layered the ambient process environment and a
//! `.env` file underneath the config blocks, which made every ambient
//! variable an undeclared build variable: `$<HOME>` resolved in a Cookfile
//! that declared nothing, and the `config` sandbox's `host.env` gate — the
//! whole point of which is that a config body's inputs are declared — could
//! be bypassed by simply not declaring them. Both layers are gone. A step
//! still inherits the ambient environment as ordinary shell variables (`$HOME`
//! in a step body works); reading one as a *keyed determinant* is what the
//! `envs { ... }` probe (§22) is for.

use std::collections::HashMap;

use super::error::PipelineError;

/// Parse `KEY=VALUE` override strings (the CLI `--set` flags) into a map.
///
/// The engine applies these to the `var` namespace after the config blocks
/// run, so an explicit CLI override wins over a config-block default
/// regardless of how the block was authored. Overriding a name no config
/// block declared is an error, raised at that point (§5.3.1).
pub fn parse_cli_overrides(
    overrides: &[String],
) -> Result<HashMap<String, String>, PipelineError> {
    let mut map = HashMap::new();
    for set_arg in overrides {
        if let Some(eq_pos) = set_arg.find('=') {
            let key = set_arg[..eq_pos].to_string();
            let value = set_arg[eq_pos + 1..].to_string();
            map.insert(key, value);
        } else {
            return Err(PipelineError::InvalidSet(set_arg.clone()));
        }
    }
    Ok(map)
}

#[cfg(test)]
#[path = "tests/env_tests.rs"]
mod tests;
