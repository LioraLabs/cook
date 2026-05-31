mod cook_step;
mod lua_env;
mod lua_string;
mod plate_step;
mod probe;
mod recipe;
pub(crate) mod resolver;
pub mod sigil;
mod template;
mod test_step;

pub mod dep_ref;

#[cfg(test)]
mod tests;

pub use recipe::{
    compile_chore, generate, generate_with_names, generate_with_names_and_warnings,
    generate_with_names_checked, CodegenError,
};
