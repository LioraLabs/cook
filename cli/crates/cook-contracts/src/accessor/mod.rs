//! Path accessor names shared by parsers and resolvers.

/// Path accessor names admitted by `path.X(...)` and by placeholder
/// dotted-suffix forms (`{NAME.ACCESSOR}`, `$<NAME.ACCESSOR>`).
///
/// This constant is the single authoritative definition; every module that
/// validates accessor suffixes MUST import it from here.
pub const ACCESSORS: &[&str] = &["stem", "name", "ext", "dir"];
