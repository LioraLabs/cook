mod cook_step;
pub mod lua_var;
mod lua_scan;
mod lua_string;
mod probe;
mod recipe;
pub(crate) mod resolver;
pub mod sigil;
mod template;
mod test_step;

pub mod dep_ref;

#[cfg(test)]
#[path = "tests/luagen_tests.rs"]
mod tests;

pub use recipe::{compile_chore, generate_checked, CodegenError};
