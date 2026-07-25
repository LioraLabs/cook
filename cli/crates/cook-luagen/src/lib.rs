mod cook_step;
pub mod lua_var;
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

pub use recipe::{
    compile_chore, generate, generate_with_names, generate_with_names_and_warnings,
    generate_with_names_checked, CodegenError,
};
