//! CookError: top-level error type with exit codes.

#[derive(Debug)]
#[allow(dead_code)]
pub enum CookError {
    ParseError(String),
    RecipeNotFound(String),
    CommandFailed(String),
    TestFailure(usize),
    Other(String),
}

impl CookError {
    pub fn exit_code(&self) -> i32 {
        match self {
            CookError::CommandFailed(_) => 1,
            CookError::TestFailure(_) => 1,
            CookError::ParseError(_) => 2,
            CookError::RecipeNotFound(_) => 3,
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
            CookError::CommandFailed(msg) => write!(f, "{msg}"),
            CookError::TestFailure(n) => write!(f, "{n} test(s) failed"),
            CookError::Other(msg) => write!(f, "{msg}"),
        }
    }
}
