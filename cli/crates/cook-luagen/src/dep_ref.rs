use std::collections::{BTreeMap, BTreeSet};

use cook_lang::ast::*;

use crate::sigil;

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
            // CS-0239: `gather $<gen>` is a name reference like any other, so
            // §10.6's "every name reference creates its edge" binds it. The
            // edge is also what ORDERS the two registrations: the register
            // pass runs bodies in `requires` topological order, and this body
            // reads `gen`'s registered output list.
            Step::MemberSource { step, .. } => match &step.source {
                MemberSource::RecipeRef(name) => vec![name.clone()],
                _ => vec![],
            },
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

/// Parse a single `$<TOKEN>` into a `DepRef` if it names a recipe.
///
/// CS-0210: this asks the resolver and adapts the answer; it does not classify.
/// §{xref.dep-implications}'s edges are the edges §{xref.resolution} implies, so
/// the shapes that are NOT recipe references — the builtins, the retired `env.`
/// and `file:` prefixes, probe-value references, declared variables, the
/// malformed bracket indices — are excluded here by [`resolver::recipe_ref`]
/// answering `None`, at the same moment and for the same reason that
/// substitution excludes them.
///
/// It used to re-derive all of that: a private builtin table, a hand-rolled
/// `out_N` number parse, and prefix tests on `in.` / `out.` / `env.`. Those
/// tests were coarser than the resolver's — `in.` matched `$<in.foo>` even when
/// `in` was an import alias and `in.foo` a recipe in scope — so codegen emitted
/// `cook.dep_output("in.foo")` for a producer the DAG had no edge to.
fn parse_dep_token(token: &str, recipe_names: &BTreeSet<String>) -> Option<DepRef> {
    let r = crate::resolver::recipe_ref(token, recipe_names)?;
    Some(DepRef {
        recipe_name: r.name,
        accessor: r.accessor,
    })
}

#[cfg(test)]
#[path = "tests/dep_ref_tests.rs"]
mod tests;
