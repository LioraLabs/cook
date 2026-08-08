mod cook_step;

/// The generated-Lua local binding a chore body's declared parameters (the
/// bound-argv table). Luagen-private — the register side never reads the
/// name; it reaches the values through the closure parameter. Spelled once
/// (COOK-390): it appears in the chore wrapper's parameter list, per-param
/// locals, static-read rewrites, and the `cook.__quote_param` emission.
pub(crate) const COOK_PARAMS_LOCAL: &str = "__cook_params";
pub mod lua_var;
mod lua_scan;
mod lua_string;
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

pub use recipe::{compile_chore, generate_checked, CodegenError};
