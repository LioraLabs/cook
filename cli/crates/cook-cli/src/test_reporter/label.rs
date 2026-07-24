//! Display labels for tests, per §3.1 of the test-runner output design.
//!
//! Pure functions that map a test's identity to the human-readable
//! string the reporter prints (e.g. `recipe@line`, `recipe@line [item]`).

/// Produce the display label for a test.
///
/// - `recipe`        — recipe name (may be namespaced as `ns.recipe`)
/// - `line`          — source line of the `test` step in the Cookfile
/// - `iteration_item`— the iteration item (e.g. input filename), if any
/// - `multi_namespace`— true iff the run touches more than one namespace
///
/// CS-0135 removed the `as` modifier, so no surface supplies an explicit
/// test name; the unit's derived `<recipe>_test<N>` name (CS-0160) is
/// identity, not prose — the printed label stays `recipe@line`.
pub fn label(
    recipe: &str,
    line: u32,
    iteration_item: Option<&str>,
    multi_namespace: bool,
) -> String {
    let core = format!("{recipe}@{line}");
    let core = if multi_namespace {
        core
    } else {
        // Strip leading "ns." if recipe was already namespace-prefixed and
        // the run is single-namespace. Recipe names never contain '.' in
        // their local form, so a leading "ns." segment is unambiguous.
        match core.find('.') {
            Some(idx) if idx + 1 < core.len() => core[idx + 1..].to_string(),
            _ => core,
        }
    };
    match iteration_item {
        Some(item) if !item.is_empty() => format!("{core} [{item}]"),
        _ => core,
    }
}

#[cfg(test)]
#[path = "tests/label_tests.rs"]
mod tests;
