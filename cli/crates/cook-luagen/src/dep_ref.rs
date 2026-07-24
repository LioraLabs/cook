use std::collections::{BTreeMap, BTreeSet};

use cook_contracts::ACCESSORS;
use cook_lang::ast::*;

use crate::sigil;

/// Built-in placeholders that are never recipe references.
/// Note: "out_N" forms are handled structurally in parse_dep_token.
const BUILTINS: &[&str] = &["in", "out"];

/// A reference to another recipe found in a step template.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct DepRef {
    /// The recipe being referenced (e.g., "libmath", "backend.proto").
    pub recipe_name: String,
    /// If present, the accessor (e.g., "stem" from `$<libmath.stem>`).
    pub accessor: Option<String>,
}

/// Extract all recipe names from a Cookfile.
pub fn extract_recipe_names(cookfile: &Cookfile) -> BTreeSet<String> {
    cookfile.recipes.iter().map(|r| r.name.clone()).collect()
}

/// Per §7.3, the lookup set for resolving qualified name references is the
/// union of:
/// - The current Cookfile's recipe names.
/// - The set `{alias.recipe : alias is an import alias of the current Cookfile,
///   recipe is a recipe in the imported Cookfile}`.
///
/// This helper builds that union. It is non-transitive: nested-import recipes
/// (e.g., `lib.shared.recipe`) are NOT included.
pub fn extract_recipe_names_with_imports(
    cookfile: &Cookfile,
    imports_by_alias: &BTreeMap<String, &Cookfile>,
) -> BTreeSet<String> {
    let mut set: BTreeSet<String> = cookfile.recipes.iter().map(|r| r.name.clone()).collect();
    for (alias, imp) in imports_by_alias {
        for r in &imp.recipes {
            set.insert(format!("{alias}.{}", r.name));
        }
    }
    set
}

/// Extract all $<dep> and $<dep.accessor> references from a recipe's steps,
/// given the set of known recipe names.
pub fn extract_dep_refs(recipe: &Recipe, recipe_names: &BTreeSet<String>) -> BTreeSet<DepRef> {
    extract_dep_refs_from_steps(&recipe.steps, recipe_names)
}

/// Step-level worker for `extract_dep_refs`, shared with the chore path:
/// per §10.6 a name reference in any step establishes a cross-recipe edge,
/// and chores carry the same `Step` list as recipes.
pub fn extract_dep_refs_from_steps(
    steps: &[Step],
    recipe_names: &BTreeSet<String>,
) -> BTreeSet<DepRef> {
    let mut refs = BTreeSet::new();

    for step in steps {
        let tokens = match step {
            Step::Cook { step: cook_step, .. } => {
                let mut t: Vec<String> = Vec::new();
                for pat in &cook_step.outputs {
                    t.extend(extract_sigil_tokens(pat.as_str()));
                }
                // Walk ShellBlock lines for $<NAME> tokens.
                if let Some(Body::ShellBlock(lines)) = &cook_step.body {
                    for line in lines {
                        t.extend(extract_sigil_tokens(line));
                    }
                }
                t
            }
            Step::Test { step: test_step, .. } => extract_body_tokens(&test_step.body),
            Step::Shell { command, .. } => extract_sigil_tokens(command),
            Step::Lua { .. } | Step::LuaBlock { .. } | Step::InlineLua { .. } => vec![],
            // `Step` is `#[non_exhaustive]`; unknown future variants contribute
            // no dep-refs in this analyzer until codegen learns about them.
            _ => vec![],
        };

        for token in tokens {
            if let Some(dep_ref) = parse_dep_token(&token, recipe_names) {
                refs.insert(dep_ref);
            }
        }
    }

    refs
}

/// Extract all $<IDENT> tokens from a template string. Returns ident strings.
pub fn extract_sigil_tokens(template: &str) -> Vec<String> {
    sigil::scan(template)
        .into_iter()
        .map(|s| s.ident)
        .collect()
}

/// Extract sigil-token dep refs from a `Body`, supporting both shell and Lua bodies.
///
/// For `ShellBlock` bodies, `$<NAME>` tokens are scanned exactly as in cook-step
/// shell lines.  For `LuaBlock` bodies, cross-recipe access goes via
/// `cook.dep_output()` (a Lua API call), which is opaque to the static sigil
/// scanner — return an empty list.
fn extract_body_tokens(body: &cook_lang::ast::Body) -> Vec<String> {
    use cook_lang::ast::Body;
    match body {
        Body::ShellBlock(lines) => {
            let joined = lines.join("\n");
            extract_sigil_tokens(&joined)
        }
        // Lua bodies do not participate in cross-recipe `$<NAME>` substitution
        // (Lua syntax owns the braces). Cross-recipe access in Lua bodies is
        // via `cook.dep_output()` — not extracted here.
        Body::LuaBlock(_) => Vec::new(),
    }
}

/// Parse a single $<FOO> token into a DepRef if it matches a recipe name.
///
/// Rules (CS-0033 updated):
/// 1. Skip builtins: `in`, `out`
/// 2. Skip CS-0022 dotted own-input/output forms: `in.X`, `out.X`, `out_N.X`
/// 3. If whole token is a recipe name → DepRef { recipe_name, accessor: None }
/// 4. If token has a dot, split on LAST dot: if suffix is a known accessor AND prefix
///    is a recipe name → DepRef with accessor
/// 5. Otherwise → None (it's an env var)
fn parse_dep_token(token: &str, recipe_names: &BTreeSet<String>) -> Option<DepRef> {
    // Rule 1: skip builtins
    if BUILTINS.contains(&token) {
        return None;
    }

    // Rule 1b: skip env. prefix (always env var, never recipe)
    if token.starts_with("env.") {
        return None;
    }

    // Rule 2: skip CS-0022 own-input/output accessor forms.
    if token.starts_with("in.") {
        return None;
    }
    if token.starts_with("out.") {
        return None;
    }
    if token.starts_with("out_") {
        let rest = &token[4..];
        let num_part = rest.split('.').next().unwrap_or(rest);
        if num_part.parse::<usize>().is_ok() {
            return None;
        }
    }

    // COOK-221 / CS-0137: `$<recipe[in]>` — per-member ref (formerly `$<recipe[]>`,
    // COOK-96). Strip the `[in]` index and treat as a recipe-level edge (the
    // producer must build first).
    if let Some(base) = token.strip_suffix("[in]") {
        if recipe_names.contains(base) {
            return Some(DepRef { recipe_name: base.to_string(), accessor: None });
        }
    }

    // Rule 3: whole token is a recipe name
    if recipe_names.contains(token) {
        return Some(DepRef {
            recipe_name: token.to_string(),
            accessor: None,
        });
    }

    // Rule 4: split on LAST dot, check if suffix is accessor and prefix is recipe name
    if let Some(dot_pos) = token.rfind('.') {
        let prefix = &token[..dot_pos];
        let suffix = &token[dot_pos + 1..];

        if ACCESSORS.contains(&suffix) && recipe_names.contains(prefix) {
            return Some(DepRef {
                recipe_name: prefix.to_string(),
                accessor: Some(suffix.to_string()),
            });
        }
    }

    // Rule 5: env var or unknown — skip
    None
}

#[cfg(test)]
#[path = "tests/dep_ref_tests.rs"]
mod tests;
