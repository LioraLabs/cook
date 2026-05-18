//! CookError: top-level error type with exit codes.

#[derive(Debug)]
#[allow(dead_code)]
pub enum CookError {
    ParseError(String),
    RecipeNotFound(String),
    /// A recipe name was registered more than once within a single Cookfile
    /// pass — e.g. a surface `recipe NAME` block + a `cook.recipe("NAME", ...)`
    /// call, or two `cook.recipe(...)` calls using the same name. The carried
    /// string is the fully-formatted multi-line diagnostic naming each
    /// registration site by line and kind (rendered at CLI emit time in
    /// `pipeline_error_to_cook_error`). Exit code 3, matching `RecipeNotFound`
    /// — both signal "the requested recipe surface is not in a runnable state".
    /// SHI-222 Phase 5 Task 5.6, spec §8.
    RecipeCollision(String),
    CommandFailed(String),
    /// One or more tests failed, were blocked, or timed out.
    /// Exit code 1 — distinct from `CommandFailed` so callers can distinguish
    /// "a build step failed" from "tests ran but reported failures".
    TestFailure(String),
    Other(String),
}

impl CookError {
    pub fn exit_code(&self) -> i32 {
        match self {
            CookError::CommandFailed(_) => 1,
            CookError::TestFailure(_) => 1,
            CookError::ParseError(_) => 2,
            CookError::RecipeNotFound(_) => 3,
            CookError::RecipeCollision(_) => 3,
            CookError::Other(_) => 1,
        }
    }
}

impl std::error::Error for CookError {}

impl std::fmt::Display for CookError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CookError::ParseError(msg) => write!(f, "parse error: {msg}"),
            CookError::RecipeNotFound(name) => write!(f, "recipe not found: {name}"),
            CookError::RecipeCollision(msg) => write!(f, "{msg}"),
            CookError::CommandFailed(msg) => write!(f, "{msg}"),
            CookError::TestFailure(msg) => write!(f, "{msg}"),
            CookError::Other(msg) => write!(f, "{msg}"),
        }
    }
}
