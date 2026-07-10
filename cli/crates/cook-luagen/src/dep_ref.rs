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

    // COOK-96: `$<recipe[]>` — per-member ref. Strip the empty bracket and
    // treat as a recipe-level edge (the producer must build first).
    if let Some(base) = token.strip_suffix("[]") {
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
mod tests {
    use super::*;

    fn make_cookfile(recipes: Vec<Recipe>) -> Cookfile {
        Cookfile {
            config_blocks: vec![],
            recipes,
            chores: vec![],
            uses: vec![],
            imports: vec![],
            register_blocks: vec![],
            top_level_module_calls: vec![],
            probes: vec![],
        }
    }

    fn make_recipe(name: &str, steps: Vec<Step>) -> Recipe {
        Recipe {
            name: name.to_string(),
            deps: vec![],
            ingredients: vec![],
            excludes: vec![],
            steps,
            line: 1,
        }
    }

    #[test]
    fn test_extract_recipe_names() {
        let cookfile = make_cookfile(vec![
            make_recipe("libmath", vec![]),
            make_recipe("backend", vec![]),
        ]);
        let names = extract_recipe_names(&cookfile);
        assert_eq!(names.len(), 2);
        assert!(names.contains("libmath"));
        assert!(names.contains("backend"));
    }

    #[test]
    fn test_extract_sigil_tokens() {
        let tokens = extract_sigil_tokens("gcc -c $<in> -o $<out> $<libmath>");
        assert_eq!(tokens, vec!["in", "out", "libmath"]);
    }

    #[test]
    fn test_extract_sigil_tokens_with_accessor() {
        let tokens = extract_sigil_tokens("build/$<protos.stem>.o");
        assert_eq!(tokens, vec!["protos.stem"]);
    }

    #[test]
    fn test_parse_dep_token_plain_recipe() {
        let mut names = BTreeSet::new();
        names.insert("libmath".to_string());
        let result = parse_dep_token("libmath", &names);
        assert_eq!(
            result,
            Some(DepRef { recipe_name: "libmath".to_string(), accessor: None })
        );
    }

    #[test]
    fn test_parse_dep_token_with_accessor() {
        let mut names = BTreeSet::new();
        names.insert("protos".to_string());
        let result = parse_dep_token("protos.stem", &names);
        assert_eq!(
            result,
            Some(DepRef {
                recipe_name: "protos".to_string(),
                accessor: Some("stem".to_string()),
            })
        );
    }

    #[test]
    fn test_parse_dep_token_dotted_recipe_name() {
        // "backend.build" is itself a recipe name — should match with no accessor
        let mut names = BTreeSet::new();
        names.insert("backend.build".to_string());
        let result = parse_dep_token("backend.build", &names);
        assert_eq!(
            result,
            Some(DepRef { recipe_name: "backend.build".to_string(), accessor: None })
        );
    }

    #[test]
    fn test_parse_dep_token_env_var() {
        let mut names = BTreeSet::new();
        names.insert("libmath".to_string());
        let result = parse_dep_token("CC", &names);
        assert_eq!(result, None);
    }

    #[test]
    fn test_parse_dep_token_builtin_ignored() {
        let names = BTreeSet::new();
        assert_eq!(parse_dep_token("in", &names), None);
        assert_eq!(parse_dep_token("out", &names), None);
        assert_eq!(parse_dep_token("stem", &names), None);
        // "all" is no longer a builtin (COOK-195); this stays None here only
        // because `names` has no recipe called "all" registered — not because
        // "all" is skipped as a builtin.
        assert_eq!(parse_dep_token("all", &names), None);
    }

    #[test]
    fn test_extract_dep_refs_from_cook_step() {
        let mut recipe_names = BTreeSet::new();
        recipe_names.insert("libmath".to_string());
        recipe_names.insert("libstr".to_string());

        let recipe = make_recipe(
            "app",
            vec![Step::Cook {
                step: CookStep {
                    outputs: vec![OutputPattern::Quoted("build/app".to_string())],
                    body: Some(Body::ShellBlock(
                        vec!["gcc -o $<out> $<in> $<libmath> $<libstr>".to_string()],
                    )),
                    disposition: Default::default(),
                },
                line: 2,
            }],
        );

        let refs = extract_dep_refs(&recipe, &recipe_names);
        assert_eq!(refs.len(), 2);
        assert!(refs.contains(&DepRef { recipe_name: "libmath".to_string(), accessor: None }));
        assert!(refs.contains(&DepRef { recipe_name: "libstr".to_string(), accessor: None }));
    }

    #[test]
    fn test_extract_dep_refs_from_output_pattern() {
        let mut recipe_names = BTreeSet::new();
        recipe_names.insert("protos".to_string());

        let recipe = make_recipe(
            "app",
            vec![Step::Cook {
                step: CookStep {
                    outputs: vec![OutputPattern::Quoted("build/$<protos.stem>.pb.cc".to_string())],
                    body: None,
                    disposition: Default::default(),
                },
                line: 2,
            }],
        );

        let refs = extract_dep_refs(&recipe, &recipe_names);
        assert_eq!(refs.len(), 1);
        assert!(refs.contains(&DepRef {
            recipe_name: "protos".to_string(),
            accessor: Some("stem".to_string()),
        }));
    }

    // ── CS-0022 tests ────────────────────────────────────────────────

    #[test]
    fn cs_0022_in_and_out_are_not_dep_refs() {
        let mut names = BTreeSet::new();
        names.insert("libmath".to_string());

        assert!(parse_dep_token("in.stem", &names).is_none(),
            "in.stem is an own-input accessor, not a dep ref");
        assert!(parse_dep_token("out.dir", &names).is_none(),
            "out.dir is an output accessor, not a dep ref");
        assert!(parse_dep_token("out_1.stem", &names).is_none(),
            "out_1.stem is a multi-output accessor, not a dep ref");
        assert_eq!(
            parse_dep_token("libmath.stem", &names).map(|d| d.recipe_name),
            Some("libmath".to_string()),
            "libmath.stem is a genuine dep ref"
        );
    }

    #[test]
    fn test_extract_recipe_names_with_imports_includes_aliased() {
        use std::collections::BTreeMap;

        let lib_cookfile = make_cookfile(vec![
            make_recipe("lib_build", vec![]),
            make_recipe("lib_test", vec![]),
        ]);
        let main_cookfile = make_cookfile(vec![make_recipe("demo", vec![])]);

        let mut imports_by_alias: BTreeMap<String, &Cookfile> = BTreeMap::new();
        imports_by_alias.insert("lib".to_string(), &lib_cookfile);

        let names = extract_recipe_names_with_imports(&main_cookfile, &imports_by_alias);
        assert!(names.contains("demo"));
        assert!(names.contains("lib.lib_build"));
        assert!(names.contains("lib.lib_test"));
        assert_eq!(names.len(), 3);
    }

    #[test]
    fn test_extract_recipe_names_with_imports_no_imports_equals_local() {
        use std::collections::BTreeMap;

        let cookfile = make_cookfile(vec![make_recipe("a", vec![]), make_recipe("b", vec![])]);
        let imports_by_alias: BTreeMap<String, &Cookfile> = BTreeMap::new();
        let names = extract_recipe_names_with_imports(&cookfile, &imports_by_alias);
        let local = extract_recipe_names(&cookfile);
        assert_eq!(names, local);
    }

    #[test]
    fn parse_dep_token_strips_bracket_for_recipe_member() {
        let mut names = BTreeSet::new();
        names.insert("render".to_string());
        assert_eq!(
            parse_dep_token("render[]", &names),
            Some(DepRef { recipe_name: "render".to_string(), accessor: None })
        );
        // bare recipe still works
        assert_eq!(
            parse_dep_token("render", &names),
            Some(DepRef { recipe_name: "render".to_string(), accessor: None })
        );
        // a [] on a non-recipe is not a dep
        assert_eq!(parse_dep_token("notarecipe[]", &names), None);
    }

    #[test]
    fn cs_0022_shell_block_dep_ref_extraction() {
        // Shell block with $<NAME> references must be extracted.
        let mut recipe_names = BTreeSet::new();
        recipe_names.insert("libmath".to_string());

        let recipe = make_recipe(
            "app",
            vec![Step::Cook {
                step: CookStep {
                    outputs: vec![OutputPattern::Quoted("build/app".to_string())],
                    body: Some(Body::ShellBlock(vec![
                        "gcc -o $<out> main.c $<libmath>".to_string(),
                    ])),
                    disposition: Default::default(),
                },
                line: 2,
            }],
        );

        let refs = extract_dep_refs(&recipe, &recipe_names);
        assert!(
            refs.iter().any(|r| r.recipe_name == "libmath" && r.accessor.is_none()),
            "shell block must contribute its $<libmath> reference to the dep graph; got: {:?}",
            refs
        );
    }
}
