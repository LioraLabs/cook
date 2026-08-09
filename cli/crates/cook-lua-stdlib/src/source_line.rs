//! Where in the Cookfile a Lua call came from.
//!
//! Both phases need this and for the same reason: a `cook.sh` that exits
//! non-zero becomes a `CommandFailure`, and a `CommandFailure` carrying a
//! line is rendered `Cookfile:LINE: command failed …` while one carrying `0`
//! is rendered with no location at all. Until COOK-422 the register phase
//! walked the stack and the execute phase passed a literal `0`, so the same
//! failure was located in one phase and anonymous in the other.
//!
//! The walk is here rather than in either host because it is Lua-touching
//! law with two consumers, which is exactly what this crate is the home for.

/// Upper bound on the call-stack walk.
///
/// A Cookfile frame is normally one or two levels up. Deep recursion in a
/// module could push it further, and an unbounded walk on a runaway stack
/// would cost more than the diagnostic is worth, so the walk gives up and
/// the caller degrades to "no line" rather than to a wrong one.
const MAX_LUA_STACK_DEPTH: usize = 40;

/// The current line of the innermost frame whose chunk is `target`, or
/// `None` when no such frame is on the stack within [`MAX_LUA_STACK_DEPTH`].
///
/// `target` is a chunk name: the register phase passes the Cookfile path it
/// loaded the chunk under, the execute phase the name it pads and loads
/// bodies under. Matching is exact or by suffix, because a chunk loaded
/// through the module loader carries Lua's `@` file prefix and the path a
/// caller holds does not.
///
/// A frame from a module called BY a Cookfile is skipped, so the answer is
/// the line in the Cookfile that entered the module — which is the line a
/// reader can act on. Levels: 1 is the closure asking, 2 its caller, and so
/// on outwards.
pub fn caller_line_in_source(lua: &mlua::Lua, target: &str) -> Option<usize> {
    for level in 1..MAX_LUA_STACK_DEPTH {
        let frame = lua.inspect_stack(level)?;
        let source = frame.source().source;
        let source: &str = source.as_deref().unwrap_or("");
        if source == target || source.ends_with(target) {
            return Some(frame.curr_line() as usize);
        }
    }
    None
}

#[cfg(test)]
#[path = "tests/source_line_tests.rs"]
mod tests;
