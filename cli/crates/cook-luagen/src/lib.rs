mod cook_step;

/// The generated-Lua local binding a chore body's declared parameters (the
/// bound-argv table). Luagen-private — the register side never reads the
/// name; it reaches the values through the closure parameter. Spelled once
/// (COOK-390): it appears in the chore wrapper's parameter list, per-param
/// locals, static-read rewrites, and the `cook.__quote_param` emission.
pub(crate) const COOK_PARAMS_LOCAL: &str = "__cook_params";
mod lua_scan;
mod long_bracket;
mod probe;
mod recipe;
pub(crate) mod resolver;
pub mod sigil;
mod template;
mod test_step;
mod use_prelude;

pub mod dep_ref;

#[cfg(test)]
#[path = "tests/luagen_tests.rs"]
mod tests;

// The agreement test standing in for the edge this crate's emission templates
// cannot take: every `cook.<door>` name written here is a cook-contracts
// constant or a listed exception (COOK-439).
#[cfg(test)]
#[path = "tests/door_names_tests.rs"]
mod door_names_tests;

pub use recipe::{compile_chore, generate_checked, CodegenError};
