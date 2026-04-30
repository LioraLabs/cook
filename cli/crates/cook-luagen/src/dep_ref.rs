use std::collections::BTreeSet;

use cook_lang::ast::*;

/// Known accessor suffixes for `{dep.accessor}` syntax.
const ACCESSORS: &[&str] = &["stem", "name", "ext", "dir"];

/// Built-in placeholders that are never recipe references.
/// Note: "out_N" forms are handled structurally in parse_dep_token.
const BUILTINS: &[&str] = &["in", "out", "all"];

/// A reference to another recipe found in a step template.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct DepRef {
    /// The recipe being referenced (e.g., "libmath", "backend.proto").
    pub recipe_name: String,
    /// If present, the accessor (e.g., "stem" from `{libmath.stem}`).
    pub accessor: Option<String>,
}

/// Extract all recipe names from a Cookfile.
pub fn extract_recipe_names(cookfile: &Cookfile) -> BTreeSet<String> {
    cookfile.recipes.iter().map(|r| r.name.clone()).collect()
}

/// Extract all {dep} and {dep.accessor} references from a recipe's steps,
/// given the set of known recipe names.
pub fn extract_dep_refs(recipe: &Recipe, recipe_names: &BTreeSet<String>) -> BTreeSet<DepRef> {
    let mut refs = BTreeSet::new();

    for step in &recipe.steps {
        let tokens = match step {
            Step::Cook { step: cook_step, .. } => {
                let mut t: Vec<String> = Vec::new();
                for pat in &cook_step.outputs {
                    t.extend(extract_brace_tokens(pat));
                }
                // CS-0022: walk ShellBlock lines for {NAME} tokens (§5.5 surface extension).
                if let Some(UsingClause::ShellBlock(lines)) = &cook_step.using_clause {
                    for line in lines {
                        t.extend(extract_brace_tokens(line));
                    }
                }
                t
            }
            Step::Plate { step: plate_step, .. } => extract_brace_tokens(&plate_step.command),
            Step::Test { step: test_step, .. } => extract_brace_tokens(&test_step.command),
            Step::Shell { command, .. } => extract_brace_tokens(command),
            Step::Lua { .. }
            | Step::LuaBlock { .. }
            | Step::InlineLua { .. }
            | Step::InlineLuaBlock { .. } => vec![],
        };

        for token in tokens {
            if let Some(dep_ref) = parse_dep_token(&token, recipe_names) {
                refs.insert(dep_ref);
            }
        }
    }

    refs
}

/// Extract all {FOO} tokens from a template string. Returns inner content without braces.
/// (This needs to be public so template.rs can use it later)
pub fn extract_brace_tokens(template: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut remaining = template;

    while !remaining.is_empty() {
        match remaining.find('{') {
            None => break,
            Some(open) => {
                let after_open = &remaining[open + 1..];
                match after_open.find('}') {
                    None => break,
                    Some(close) => {
                        let inner = &after_open[..close];
                        if !inner.is_empty() {
                            tokens.push(inner.to_string());
                        }
                        remaining = &after_open[close + 1..];
                    }
                }
            }
        }
    }

    tokens
}

/// Parse a single {FOO} token into a DepRef if it matches a recipe name.
///
/// Rules (CS-0022 updated):
/// 1. Skip builtins: `in`, `out`, `all`
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

    // Rule 2: skip CS-0022 own-input/output accessor forms.
    // `in.X` — own-input accessor
    if token.starts_with("in.") {
        return None;
    }
    // `out.X` — single-output accessor
    if token.starts_with("out.") {
        return None;
    }
    // `out_N` and `out_N.X` — multi-output accessor
    if token.starts_with("out_") {
        // Check if the part after "out_" is numeric (possibly with .accessor suffix)
        let rest = &token[4..];
        let num_part = rest.split('.').next().unwrap_or(rest);
        if num_part.parse::<usize>().is_ok() {
            return None;
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
    fn test_extract_brace_tokens() {
        let tokens = extract_brace_tokens("gcc -c {in} -o {out} {libmath}");
        assert_eq!(tokens, vec!["in", "out", "libmath"]);
    }

    #[test]
    fn test_extract_brace_tokens_with_accessor() {
        let tokens = extract_brace_tokens("build/{protos.stem}.o");
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
                    outputs: vec!["build/app".to_string()],
                    using_clause: Some(UsingClause::ShellBlock(
                        vec!["gcc -o {out} {in} {libmath} {libstr}".to_string()],
                    )),
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
                    outputs: vec!["build/{protos.stem}.pb.cc".to_string()],
                    using_clause: None,
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
    fn cs_0022_shell_block_dep_ref_extraction() {
        // §5.5 surface extension: {NAME} inside shell_block lines must be extracted.
        let mut recipe_names = BTreeSet::new();
        recipe_names.insert("libmath".to_string());

        // Build the recipe manually (multi-line block to avoid single-line lexer issue).
        let recipe = make_recipe(
            "app",
            vec![Step::Cook {
                step: CookStep {
                    outputs: vec!["build/app".to_string()],
                    using_clause: Some(UsingClause::ShellBlock(vec![
                        "gcc -o {out} main.c {libmath}".to_string(),
                    ])),
                },
                line: 2,
            }],
        );

        let refs = extract_dep_refs(&recipe, &recipe_names);
        assert!(
            refs.iter().any(|r| r.recipe_name == "libmath" && r.accessor.is_none()),
            "shell block must contribute its {{libmath}} reference to the dep graph; got: {:?}",
            refs
        );
    }
}
