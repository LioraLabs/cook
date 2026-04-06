# Cross-Recipe Dependency Inference Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Enable recipes to reference other recipes' outputs via `{recipe_name}` syntax, inferring fine-grained unit-level dependencies and enabling maximal parallelism within a single DAG wave.

**Architecture:** The change spans the full pipeline: parse (pre-scan recipe names + extract `{dep}` refs) → codegen (emit `cook.dep_output()` calls and dep-driven iteration loops) → registration (store terminal outputs, expose `cook.dep_output()` Lua API, track fine-grained edges) → engine (two-tier wave grouping, fine-grained DAG wiring, multi-recipe wave execution).

**Tech Stack:** Rust, Lua (via mlua), cook-lang/cook-luagen/cook-register/cook-engine/cook-contracts/cook-cli crates.

**Design spec:** `docs/superpowers/specs/2026-04-06-cross-recipe-deps-design.md`

---

## File Structure

### New files
- `cli/crates/cook-luagen/src/dep_ref.rs` — `{dep}` reference extraction and classification
- `cli/crates/cook-register/src/dep_output_api.rs` — `cook.dep_output()` Lua API + terminal output storage
- `cli/crates/cook-engine/src/wave_grouper.rs` — Two-tier wave grouping from `: dep` and `{dep}` edges

### Modified files
- `cli/crates/cook-contracts/src/lib.rs` — Add `DepEdge` type and fields to `RecipeUnits`
- `cli/crates/cook-luagen/src/lib.rs` — Re-export new module, update `generate()` signature
- `cli/crates/cook-luagen/src/recipe.rs` — Thread `recipe_names` set into codegen
- `cli/crates/cook-luagen/src/template.rs` — Recipe-name-aware template expansion
- `cli/crates/cook-luagen/src/cook_step.rs` — Dep-driven iteration codegen path
- `cli/crates/cook-luagen/src/plate_step.rs` — `{dep}` in plate commands
- `cli/crates/cook-luagen/src/test_step.rs` — `{dep}` in test commands
- `cli/crates/cook-register/src/engine.rs` — Wire dep_output_api, pass terminal outputs between recipes
- `cli/crates/cook-register/src/lib.rs` — Export new types
- `cli/crates/cook-engine/src/dag_builder.rs` — Fine-grained cross-recipe edge wiring
- `cli/crates/cook-engine/src/run.rs` — Multi-recipe wave registration and execution
- `cli/crates/cook-engine/src/analyzer.rs` — Extract `{dep}` references for wave grouping
- `cli/crates/cook-engine/src/lib.rs` — Export new types
- `cli/crates/cook-cli/src/pipeline.rs` — Wire pre-scan into codegen, pass dep refs to engine
- `cli/crates/cook-lang/src/lexer.rs` — Reserved recipe name validation
- `examples/cross-recipe-deps/Cookfile` — Convert to `{dep}` syntax

---

## Chunk 1: Parse-Level Foundation

### Task 1: Reserved recipe name validation

**Files:**
- Modify: `cli/crates/cook-lang/src/lexer.rs`
- Test: `cli/crates/cook-lang/src/lexer.rs` (inline tests)

- [ ] **Step 1: Write failing test for reserved name rejection**

Add to the test module in `cli/crates/cook-lang/src/lexer.rs`:

```rust
#[test]
fn test_reserved_recipe_name_rejected() {
    for reserved in &["stem", "name", "ext", "dir", "in", "out", "all"] {
        let input = format!("recipe {}\n    echo hi\nend\n", reserved);
        let result = crate::parse(&input);
        assert!(result.is_err(), "recipe named '{}' should be rejected", reserved);
    }
}

#[test]
fn test_reserved_name_in_dotted_recipe_rejected() {
    let input = "recipe backend.stem\n    echo hi\nend\n";
    let result = crate::parse(&input);
    assert!(result.is_err(), "recipe with reserved final segment should be rejected");
}

#[test]
fn test_non_reserved_dotted_name_accepted() {
    let input = "recipe backend.build\n    echo hi\nend\n";
    let result = crate::parse(&input);
    assert!(result.is_ok(), "recipe with non-reserved final segment should be accepted");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p cook-lang test_reserved_recipe_name`
Expected: FAIL — no validation exists yet.

- [ ] **Step 3: Implement reserved name check**

In `cli/crates/cook-lang/src/lexer.rs`, find the function that parses the `RecipeHeader` token (the code that calls `parse_name()` after seeing `recipe` keyword). After extracting the recipe name, add validation:

```rust
const RESERVED_RECIPE_SEGMENTS: &[&str] = &["stem", "name", "ext", "dir", "in", "out", "all"];

fn validate_recipe_name(name: &str, line: usize) -> Result<(), ParseError> {
    let final_segment = name.rsplit('.').next().unwrap_or(name);
    if RESERVED_RECIPE_SEGMENTS.contains(&final_segment) {
        return Err(ParseError {
            message: format!(
                "line {}: recipe name '{}' uses reserved word '{}' as final segment (reserved: {})",
                line, name, final_segment, RESERVED_RECIPE_SEGMENTS.join(", ")
            ),
            line,
        });
    }
    Ok(())
}
```

Call `validate_recipe_name(&name, line)?` right after `parse_name()` succeeds in the recipe header parsing path.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p cook-lang test_reserved_recipe_name && cargo test -p cook-lang test_non_reserved`
Expected: All PASS.

- [ ] **Step 5: Run full cook-lang test suite**

Run: `cargo test -p cook-lang`
Expected: All existing tests still pass.

- [ ] **Step 6: Commit**

```bash
git add cli/crates/cook-lang/src/lexer.rs
git commit -m "feat(lang): reject reserved words as recipe name final segments"
```

---

### Task 2: Pre-scan — extract recipe names and `{dep}` references

**Files:**
- Create: `cli/crates/cook-luagen/src/dep_ref.rs`
- Modify: `cli/crates/cook-luagen/src/lib.rs`

This task creates the pre-scan that extracts: (1) all recipe names from a Cookfile, and (2) all `{recipe_name}` references from cook/plate/test step templates. This data is needed by both codegen (to distinguish recipe refs from env vars) and the engine (to build the wave graph).

- [ ] **Step 1: Write failing tests for dep_ref module**

Create `cli/crates/cook-luagen/src/dep_ref.rs`:

```rust
use std::collections::BTreeSet;
use cook_lang::ast::*;

/// Known accessor suffixes for `{dep.accessor}` syntax.
const ACCESSORS: &[&str] = &["stem", "name", "ext", "dir"];

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

/// Extract all `{dep}` and `{dep.accessor}` references from a recipe's steps,
/// given the set of known recipe names.
///
/// Returns the set of DepRefs found. Only tokens matching known recipe names
/// (or recipe_name.accessor patterns) are returned; other `{FOO}` tokens are
/// assumed to be environment variables.
pub fn extract_dep_refs(recipe: &Recipe, recipe_names: &BTreeSet<String>) -> BTreeSet<DepRef> {
    todo!()
}

/// Parse a single `{...}` token into a DepRef if it matches a recipe name.
///
/// Rules:
/// - `{foo}` where "foo" is a recipe name → DepRef { recipe_name: "foo", accessor: None }
/// - `{foo.stem}` where "foo" is a recipe name and "stem" is an accessor → DepRef with accessor
/// - `{backend.build}` where "backend.build" is a recipe name → DepRef, no accessor
/// - `{CC}` where "CC" is NOT a recipe name → None (it's an env var)
fn parse_dep_token(token: &str, recipe_names: &BTreeSet<String>) -> Option<DepRef> {
    todo!()
}

/// Extract all `{...}` tokens from a template string.
/// Returns the inner content (without braces).
fn extract_brace_tokens(template: &str) -> Vec<String> {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(slice: &[&str]) -> BTreeSet<String> {
        slice.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn test_extract_recipe_names() {
        let cookfile = Cookfile {
            vars: vec![],
            config_blocks: vec![],
            uses: vec![],
            imports: vec![],
            recipes: vec![
                Recipe {
                    name: "libmath".into(), deps: vec![], ingredients: vec![],
                    excludes: vec![], steps: vec![], line: 1,
                },
                Recipe {
                    name: "app".into(), deps: vec![], ingredients: vec![],
                    excludes: vec![], steps: vec![], line: 5,
                },
            ],
        };
        let names = extract_recipe_names(&cookfile);
        assert_eq!(names, BTreeSet::from(["app".into(), "libmath".into()]));
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
        let names = names(&["libmath", "libstr"]);
        let dep = parse_dep_token("libmath", &names);
        assert_eq!(dep, Some(DepRef { recipe_name: "libmath".into(), accessor: None }));
    }

    #[test]
    fn test_parse_dep_token_with_accessor() {
        let names = names(&["protos"]);
        let dep = parse_dep_token("protos.stem", &names);
        assert_eq!(dep, Some(DepRef { recipe_name: "protos".into(), accessor: Some("stem".into()) }));
    }

    #[test]
    fn test_parse_dep_token_dotted_recipe_name() {
        let names = names(&["backend.build"]);
        let dep = parse_dep_token("backend.build", &names);
        assert_eq!(dep, Some(DepRef { recipe_name: "backend.build".into(), accessor: None }));
    }

    #[test]
    fn test_parse_dep_token_env_var() {
        let names = names(&["libmath"]);
        let dep = parse_dep_token("CC", &names);
        assert_eq!(dep, None);
    }

    #[test]
    fn test_parse_dep_token_builtin_ignored() {
        let names = names(&["libmath"]);
        // Builtins like {in}, {out}, {stem}, etc. are not recipe names
        assert_eq!(parse_dep_token("in", &names), None);
        assert_eq!(parse_dep_token("out", &names), None);
        assert_eq!(parse_dep_token("stem", &names), None);
        assert_eq!(parse_dep_token("all", &names), None);
    }

    #[test]
    fn test_extract_dep_refs_from_cook_step() {
        let names = names(&["libmath", "libstr"]);
        let recipe = Recipe {
            name: "app".into(),
            deps: vec![],
            ingredients: vec!["src/main.c".into()],
            excludes: vec![],
            steps: vec![
                Step::Cook {
                    step: CookStep {
                        output_pattern: "build/obj/main.o".into(),
                        using_clause: Some(UsingClause::Shell(
                            "gcc -c {in} -o {out}".into(),
                        )),
                    },
                    line: 3,
                },
                Step::Cook {
                    step: CookStep {
                        output_pattern: "build/bin/app".into(),
                        using_clause: Some(UsingClause::Shell(
                            "gcc -o {out} {in} {libmath} {libstr}".into(),
                        )),
                    },
                    line: 4,
                },
            ],
            line: 1,
        };
        let refs = extract_dep_refs(&recipe, &names);
        assert_eq!(refs, BTreeSet::from([
            DepRef { recipe_name: "libmath".into(), accessor: None },
            DepRef { recipe_name: "libstr".into(), accessor: None },
        ]));
    }

    #[test]
    fn test_extract_dep_refs_from_output_pattern() {
        let names = names(&["protos"]);
        let recipe = Recipe {
            name: "compile_protos".into(),
            deps: vec![],
            ingredients: vec![],
            excludes: vec![],
            steps: vec![Step::Cook {
                step: CookStep {
                    output_pattern: "build/obj/{protos.stem}.o".into(),
                    using_clause: Some(UsingClause::Shell("gcc -c {in} -o {out}".into())),
                },
                line: 2,
            }],
            line: 1,
        };
        let refs = extract_dep_refs(&recipe, &names);
        assert_eq!(refs, BTreeSet::from([
            DepRef { recipe_name: "protos".into(), accessor: Some("stem".into()) },
        ]));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p cook-luagen dep_ref`
Expected: FAIL — all `todo!()` functions panic.

- [ ] **Step 3: Implement `extract_brace_tokens`**

```rust
fn extract_brace_tokens(template: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut remaining = template;
    while let Some(open) = remaining.find('{') {
        let after = &remaining[open + 1..];
        if let Some(close) = after.find('}') {
            tokens.push(after[..close].to_string());
            remaining = &after[close + 1..];
        } else {
            break;
        }
    }
    tokens
}
```

- [ ] **Step 4: Implement `parse_dep_token`**

```rust
/// Built-in placeholders that are never recipe references.
const BUILTINS: &[&str] = &["in", "out", "stem", "name", "ext", "dir", "all"];

fn parse_dep_token(token: &str, recipe_names: &BTreeSet<String>) -> Option<DepRef> {
    // Skip built-in placeholders
    if BUILTINS.contains(&token) {
        return None;
    }

    // Check if the whole token is a recipe name (handles dotted names like "backend.build")
    if recipe_names.contains(token) {
        return Some(DepRef {
            recipe_name: token.to_string(),
            accessor: None,
        });
    }

    // Check for {recipe.accessor} pattern: split on last dot
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

    None
}
```

- [ ] **Step 5: Implement `extract_dep_refs`**

```rust
pub fn extract_dep_refs(recipe: &Recipe, recipe_names: &BTreeSet<String>) -> BTreeSet<DepRef> {
    let mut refs = BTreeSet::new();

    for step in &recipe.steps {
        let templates: Vec<&str> = match step {
            Step::Cook { step, .. } => {
                let mut t = vec![step.output_pattern.as_str()];
                match &step.using_clause {
                    Some(UsingClause::Shell(cmd)) => t.push(cmd.as_str()),
                    Some(UsingClause::LuaBlock(_)) => {} // Lua blocks don't have {dep} syntax
                    None => {}
                }
                t
            }
            Step::Plate { step, .. } => vec![step.command.as_str()],
            Step::Test { step, .. } => vec![step.command.as_str()],
            Step::Shell { command, .. } => vec![command.as_str()],
            Step::Lua { .. } | Step::LuaBlock { .. } => vec![],
        };

        for template in templates {
            for token in extract_brace_tokens(template) {
                if let Some(dep_ref) = parse_dep_token(&token, recipe_names) {
                    refs.insert(dep_ref);
                }
            }
        }
    }

    refs
}
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p cook-luagen dep_ref`
Expected: All PASS.

- [ ] **Step 7: Wire into lib.rs**

In `cli/crates/cook-luagen/src/lib.rs`, add:

```rust
pub mod dep_ref;
```

And ensure the module compiles:

Run: `cargo check -p cook-luagen`
Expected: Success.

- [ ] **Step 8: Commit**

```bash
git add cli/crates/cook-luagen/src/dep_ref.rs cli/crates/cook-luagen/src/lib.rs
git commit -m "feat(luagen): add dep_ref module for extracting {recipe_name} references"
```

---

## Chunk 2: Codegen — Recipe-Aware Template Expansion

### Task 3: Thread recipe names into `generate()` and template expansion

**Files:**
- Modify: `cli/crates/cook-luagen/src/recipe.rs`
- Modify: `cli/crates/cook-luagen/src/template.rs`
- Modify: `cli/crates/cook-luagen/src/lib.rs`
- Modify: `cli/crates/cook-luagen/src/cook_step.rs`
- Modify: `cli/crates/cook-luagen/src/plate_step.rs`
- Modify: `cli/crates/cook-luagen/src/test_step.rs`
- Test: `cli/crates/cook-luagen/src/tests.rs`

This task changes the `generate()` signature to accept a `&BTreeSet<String>` of recipe names and threads it through to all template expansion functions. Template expansion for recipe refs emits `cook.dep_output("name")` instead of `cook.env["name"]`.

- [ ] **Step 1: Write failing test for recipe-aware template expansion**

Add to `cli/crates/cook-luagen/src/tests.rs`:

```rust
#[test]
fn test_dep_ref_in_command_emits_dep_output() {
    let names: std::collections::BTreeSet<String> =
        ["libmath", "libstr"].iter().map(|s| s.to_string()).collect();
    let cookfile = make_cookfile(vec![make_recipe(
        "app",
        vec![],
        vec!["src/main.c"],
        vec![
            Step::Cook {
                step: CookStep {
                    output_pattern: "build/obj/main.o".to_string(),
                    using_clause: Some(UsingClause::Shell("gcc -c {in} -o {out}".into())),
                },
                line: 3,
            },
            Step::Cook {
                step: CookStep {
                    output_pattern: "build/bin/app".to_string(),
                    using_clause: Some(UsingClause::Shell(
                        "gcc -o {out} {in} {libmath} {libstr}".into(),
                    )),
                },
                line: 4,
            },
        ],
    )]);
    let output = crate::generate_with_names(&cookfile, &names);
    assert!(
        output.contains(r#"cook.dep_output("libmath")"#),
        "expected cook.dep_output for libmath, got:\n{output}"
    );
    assert!(
        output.contains(r#"cook.dep_output("libstr")"#),
        "expected cook.dep_output for libstr, got:\n{output}"
    );
    // Built-in placeholders should still work
    assert!(output.contains("_cook_in"));
    assert!(output.contains("_cook_out"));
}

#[test]
fn test_env_var_still_works_when_not_recipe() {
    let names: std::collections::BTreeSet<String> =
        ["libmath"].iter().map(|s| s.to_string()).collect();
    let cookfile = make_cookfile(vec![make_recipe(
        "build",
        vec![],
        vec!["src/*.c"],
        vec![Step::Cook {
            step: CookStep {
                output_pattern: "build/{stem}.o".to_string(),
                using_clause: Some(UsingClause::Shell("{CC} -c {in} -o {out}".into())),
            },
            line: 3,
        }],
    )]);
    let output = crate::generate_with_names(&cookfile, &names);
    assert!(
        output.contains(r#"cook.env["CC"]"#),
        "CC is not a recipe name, should fall through to env, got:\n{output}"
    );
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p cook-luagen test_dep_ref_in_command`
Expected: FAIL — `generate_with_names` doesn't exist yet.

- [ ] **Step 3: Add `generate_with_names` to recipe.rs and update lib.rs**

In `cli/crates/cook-luagen/src/lib.rs`:

```rust
pub use recipe::{generate, generate_with_names};
```

In `cli/crates/cook-luagen/src/recipe.rs`, add a new entry point that accepts recipe names and refactor `generate` to call it with an empty set:

```rust
use std::collections::BTreeSet;

pub fn generate(cookfile: &Cookfile) -> String {
    generate_with_names(cookfile, &BTreeSet::new())
}

pub fn generate_with_names(cookfile: &Cookfile, recipe_names: &BTreeSet<String>) -> String {
    // ... existing generate body, but passing recipe_names through
}
```

- [ ] **Step 4: Update template.rs to accept recipe names**

Add a new function `expand_template_to_lua_with_deps` that takes a `&BTreeSet<String>`:

```rust
use std::collections::BTreeSet;

/// Expand a shell command template into a Lua expression, recognizing recipe names.
/// `{recipe_name}` becomes `cook.dep_output("recipe_name")` instead of `cook.env["..."]`.
pub(crate) fn expand_template_to_lua_with_deps(
    template: &str,
    recipe_names: &BTreeSet<String>,
) -> String {
    let builtins = &[
        ("{in}", "_cook_in"),
        ("{out}", "_cook_out"),
        ("{stem}", "_cook_stem"),
        ("{name}", "_cook_name"),
        ("{ext}", "_cook_ext"),
        ("{dir}", "_cook_dir"),
        ("{all}", "_cook_all"),
    ];
    expand_with_deps_fallback(template, builtins, recipe_names)
}
```

And the corresponding core expansion function that checks recipe names before falling back to env:

```rust
fn expand_with_deps_fallback(
    template: &str,
    builtins: &[(&str, &str)],
    recipe_names: &BTreeSet<String>,
) -> String {
    // Same two-pass logic as expand_template_with_env_fallback, but the
    // fallback branch checks recipe_names first:
    // 1. If inner matches a recipe name → cook.dep_output("name")
    // 2. If inner has a dot, split on last dot, check if suffix is an accessor
    //    and prefix is a recipe → cook.dep_output("prefix") (accessor handled in output pattern)
    // 3. Otherwise → cook.env["inner"]
}
```

The inner branch logic for the fallback (replacing the `cook.env` line in `expand_template_with_env_fallback`):

```rust
// In the else branch (not a builtin):
if recipe_names.contains(inner) {
    parts.push(format!("cook.dep_output(\"{}\")", escape_lua_string(inner)));
} else {
    // Check for {recipe.accessor} — in commands, {dep.stem} is NOT valid
    // (only valid in output patterns). But {dep} with dotted recipe names is.
    // Split on last dot, check if whole thing is a dotted recipe name.
    parts.push(format!("cook.env[\"{}\"]", escape_lua_string(inner)));
}
```

Also add `expand_output_pattern_with_deps` for output patterns. This function needs to detect `{dep.accessor}` patterns:

```rust
pub(crate) fn expand_output_pattern_with_deps(
    pattern: &str,
    recipe_names: &BTreeSet<String>,
) -> OutputPatternExpansion {
    // Returns structured result indicating whether dep-driven iteration was found
    todo!() // Implemented in Task 5
}
```

Keep the original functions unchanged for backward compatibility. The `_with_deps` variants are the new paths.

- [ ] **Step 5: Thread recipe_names through cook_step, plate_step, test_step**

In `cook_step.rs`, update `generate_cook_step` signature to accept `recipe_names: &BTreeSet<String>`, and use `expand_template_to_lua_with_deps` instead of `expand_template_to_lua`.

In `plate_step.rs`, update `generate_plate_step` to accept `recipe_names: &BTreeSet<String>`, and use `expand_plate_cmd_with_deps`.

In `test_step.rs`, update `generate_test_step` to accept `recipe_names: &BTreeSet<String>`, and use `expand_test_cmd_with_deps`.

In `recipe.rs` (`generate_with_names`), pass `recipe_names` to each step generation call.

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p cook-luagen test_dep_ref_in_command && cargo test -p cook-luagen test_env_var_still_works`
Expected: All PASS.

- [ ] **Step 7: Run full test suite to verify no regressions**

Run: `cargo test -p cook-luagen`
Expected: All existing tests still pass (original `generate()` calls `generate_with_names` with empty set, so all `{FOO}` tokens still fall through to `cook.env`).

- [ ] **Step 8: Commit**

```bash
git add cli/crates/cook-luagen/src/
git commit -m "feat(luagen): recipe-aware template expansion emits cook.dep_output()"
```

---

### Task 4: Dep-driven iteration codegen for cook steps

**Files:**
- Modify: `cli/crates/cook-luagen/src/template.rs`
- Modify: `cli/crates/cook-luagen/src/cook_step.rs`
- Test: `cli/crates/cook-luagen/src/tests.rs`

When a cook step's output pattern contains `{dep.stem}`, codegen should emit a loop over the dep's terminal outputs instead of the recipe's own inputs.

- [ ] **Step 1: Write failing tests**

Add to `cli/crates/cook-luagen/src/tests.rs`:

```rust
#[test]
fn test_dep_driven_iteration_codegen() {
    let names: std::collections::BTreeSet<String> =
        ["protos"].iter().map(|s| s.to_string()).collect();
    let cookfile = make_cookfile(vec![make_recipe(
        "compile_protos",
        vec![],
        vec![],
        vec![Step::Cook {
            step: CookStep {
                output_pattern: "build/obj/{protos.stem}.o".to_string(),
                using_clause: Some(UsingClause::Shell("gcc -c {in} -o {out}".into())),
            },
            line: 2,
        }],
    )]);
    let output = crate::generate_with_names(&cookfile, &names);
    // Should iterate over dep outputs, not recipe ingredients
    assert!(
        output.contains(r#"cook.dep_output("protos")"#),
        "should call cook.dep_output to get iteration source, got:\n{output}"
    );
    assert!(
        output.contains("path.stem(_cook_in)"),
        "should extract stem from dep output items, got:\n{output}"
    );
    assert!(
        !output.contains("recipe.ingredients"),
        "should NOT reference recipe.ingredients for dep-driven iteration, got:\n{output}"
    );
}

#[test]
fn test_dep_driven_iteration_followed_by_many_to_one() {
    let names: std::collections::BTreeSet<String> =
        ["protos"].iter().map(|s| s.to_string()).collect();
    let cookfile = make_cookfile(vec![make_recipe(
        "compile_protos",
        vec![],
        vec![],
        vec![
            Step::Cook {
                step: CookStep {
                    output_pattern: "build/obj/{protos.stem}.o".to_string(),
                    using_clause: Some(UsingClause::Shell("gcc -c {in} -o {out}".into())),
                },
                line: 2,
            },
            Step::Cook {
                step: CookStep {
                    output_pattern: "build/lib/libprotos.a".to_string(),
                    using_clause: Some(UsingClause::Shell("ar rcs {out} {all}".into())),
                },
                line: 3,
            },
        ],
    )]);
    let output = crate::generate_with_names(&cookfile, &names);
    // Second step should use _cook_outputs_1 (from first step), NOT dep outputs
    assert!(
        output.contains("table.concat(_cook_outputs_1"),
        "second step should chain from first step's outputs, got:\n{output}"
    );
}

#[test]
fn test_mixed_dep_substitution_and_own_iteration() {
    let names: std::collections::BTreeSet<String> =
        ["protos", "core"].iter().map(|s| s.to_string()).collect();
    let cookfile = make_cookfile(vec![make_recipe(
        "server",
        vec![],
        vec![],
        vec![Step::Cook {
            step: CookStep {
                output_pattern: "build/obj/{protos.stem}.o".to_string(),
                using_clause: Some(UsingClause::Shell(
                    "gcc -c {in} -I{core}/include -o {out}".into(),
                )),
            },
            line: 2,
        }],
    )]);
    let output = crate::generate_with_names(&cookfile, &names);
    // Iteration driven by protos
    assert!(output.contains(r#"cook.dep_output("protos")"#));
    // String substitution of core
    assert!(output.contains(r#"cook.dep_output("core")"#));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p cook-luagen test_dep_driven_iteration`
Expected: FAIL.

- [ ] **Step 3: Implement output pattern analysis in template.rs**

Add to `cli/crates/cook-luagen/src/template.rs`:

```rust
/// Result of analyzing an output pattern for dep-driven iteration.
pub(crate) enum OutputPatternKind {
    /// Normal pattern — iteration driven by recipe's own inputs.
    /// Contains the expanded Lua expression for the output path.
    OwnInputs(String),
    /// Dep-driven iteration — iteration driven by a dependency's terminal outputs.
    /// Contains (dep_recipe_name, expanded_lua_expr_for_output).
    DepDriven {
        dep_name: String,
        lua_expr: String,
    },
}

/// Analyze an output pattern and return whether it uses dep-driven iteration.
/// Recognizes `{dep.stem}`, `{dep.name}`, `{dep.ext}`, `{dep.dir}` as dep-driven.
pub(crate) fn analyze_output_pattern(
    pattern: &str,
    recipe_names: &BTreeSet<String>,
) -> OutputPatternKind {
    let dep_accessors: &[(&str, &str)] = &[
        ("stem", "_cook_stem"),
        ("name", "_cook_name"),
        ("ext", "_cook_ext"),
        ("dir", "_cook_dir"),
    ];

    // Scan for {recipe.accessor} in the pattern
    for token in crate::dep_ref::extract_brace_tokens_pub(pattern) {
        if let Some(dot_pos) = token.rfind('.') {
            let prefix = &token[..dot_pos];
            let suffix = &token[dot_pos + 1..];
            if recipe_names.contains(prefix)
                && dep_accessors.iter().any(|&(acc, _)| acc == suffix)
            {
                // Found dep-driven iteration. Expand the pattern treating
                // {dep.accessor} as the corresponding _cook_xxx variable
                // (since we'll set up _cook_stem etc. from the dep output).
                let own_builtins = &[
                    ("{stem}", "_cook_stem"),
                    ("{name}", "_cook_name"),
                    ("{ext}", "_cook_ext"),
                    ("{dir}", "_cook_dir"),
                    ("{in}", "_cook_in"),
                ];

                // Replace {dep.accessor} with the same variable as {accessor}
                let normalized = pattern.replace(
                    &format!("{{{}.{}}}", prefix, suffix),
                    &format!("{{{}}}", suffix),
                );

                let lua_expr = expand_template_with_env_fallback(&normalized, own_builtins);

                return OutputPatternKind::DepDriven {
                    dep_name: prefix.to_string(),
                    lua_expr,
                };
            }
        }
    }

    // No dep-driven iteration found — use normal expansion
    OutputPatternKind::OwnInputs(expand_output_pattern(pattern))
}
```

Note: You'll need to make `extract_brace_tokens` public (or add a `pub` wrapper `extract_brace_tokens_pub`) in `dep_ref.rs`.

- [ ] **Step 4: Update cook_step.rs to handle dep-driven iteration**

In `generate_cook_step`, after determining the `mode`, check the output pattern:

```rust
use crate::template::{analyze_output_pattern, OutputPatternKind};

// Before the existing match on mode, analyze the output pattern:
let pattern_kind = analyze_output_pattern(
    &cook_step.output_pattern,
    recipe_names,
);

match mode {
    CookMode::OneToOne => {
        match &pattern_kind {
            OutputPatternKind::DepDriven { dep_name, lua_expr } => {
                // Dep-driven iteration: loop over dep's terminal outputs
                out.push_str(&format!(
                    "    for _, _cook_in in ipairs(cook.dep_output_list(\"{}\")) do\n",
                    crate::lua_string::escape_lua_string(dep_name)
                ));
                out.push_str("        local _cook_stem = path.stem(_cook_in)\n");
                out.push_str("        local _cook_name = path.name(_cook_in)\n");
                out.push_str("        local _cook_ext = path.ext(_cook_in)\n");
                out.push_str("        local _cook_dir = path.dir(_cook_in)\n");
                out.push_str(&format!("        local _cook_out = {}\n", lua_expr));
                // ... rest is same as current OneToOne (add_unit, table.insert)
            }
            OutputPatternKind::OwnInputs(_) => {
                // Existing OneToOne logic (unchanged)
            }
        }
    }
    // ManyToOne and DeclarationOnly unchanged
}
```

The key difference: `for _, _cook_in in ipairs(cook.dep_output_list("protos"))` instead of `for _, _cook_in in ipairs(input_source)`.

Note: We need both `cook.dep_output(name)` (returns space-joined string for command substitution) and `cook.dep_output_list(name)` (returns Lua table for iteration). Both will be implemented in the registration task.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p cook-luagen test_dep_driven`
Expected: All PASS.

- [ ] **Step 6: Run full test suite**

Run: `cargo test -p cook-luagen`
Expected: All pass.

- [ ] **Step 7: Commit**

```bash
git add cli/crates/cook-luagen/src/
git commit -m "feat(luagen): dep-driven iteration codegen for {dep.stem} in output patterns"
```

---

## Chunk 3: Registration — Terminal Output Storage and Lua API

### Task 5: Add terminal output tracking to contracts and capture state

**Files:**
- Modify: `cli/crates/cook-contracts/src/lib.rs`
- Modify: `cli/crates/cook-register/src/lib.rs`

- [ ] **Step 1: Write failing tests**

Add to `cli/crates/cook-contracts/src/lib.rs` test module:

```rust
#[test]
fn recipe_units_with_terminal_outputs() {
    let recipe = RecipeUnits {
        recipe_name: "libmath".into(),
        deps: vec![],
        units: vec![],
        step_groups: vec![],
        working_dir: PathBuf::from("."),
        env_vars: BTreeMap::new(),
        terminal_outputs: vec!["build/lib/libmath.a".into()],
        dep_edges: vec![],
    };
    assert_eq!(recipe.terminal_outputs, vec!["build/lib/libmath.a"]);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p cook-contracts recipe_units_with_terminal`
Expected: FAIL — fields don't exist.

- [ ] **Step 3: Add fields to RecipeUnits**

In `cli/crates/cook-contracts/src/lib.rs`:

```rust
/// Result of registering a single recipe.
pub struct RecipeUnits {
    pub recipe_name: String,
    pub deps: Vec<String>,
    pub units: Vec<CapturedUnit>,
    pub step_groups: Vec<Vec<usize>>,
    pub working_dir: PathBuf,
    pub env_vars: BTreeMap<String, String>,
    /// Terminal outputs: the output paths from the recipe's final cook step.
    pub terminal_outputs: Vec<String>,
    /// Fine-grained cross-recipe dependency edges.
    /// Each entry is (unit_index_in_this_recipe, dep_recipe_name).
    /// Indicates that units[unit_index] consumes dep_recipe_name's terminal output.
    pub dep_edges: Vec<(usize, String)>,
}
```

- [ ] **Step 4: Fix all compilation errors from the new fields**

Every place that constructs `RecipeUnits` needs the new fields. Add `terminal_outputs: vec![], dep_edges: vec![]` to:
- `cli/crates/cook-register/src/engine.rs` (line ~129)
- `cli/crates/cook-engine/src/dag_builder.rs` test helpers
- `cli/crates/cook-contracts/src/lib.rs` tests
- Any other test files that construct RecipeUnits

Run: `cargo check --workspace`
Expected: Compiles.

- [ ] **Step 5: Add terminal_output tracking to CaptureState**

In `cli/crates/cook-register/src/lib.rs`, add to `CaptureState`:

```rust
pub struct CaptureState {
    pub inside_layer: bool,
    pub layer_commands: Vec<(String, usize)>,
    pub units: Vec<CapturedUnit>,
    pub current_group: Option<usize>,
    pub step_groups: Vec<Vec<usize>>,
    /// Tracks which cook step index is the latest (for terminal output detection).
    pub last_cook_step_outputs: Vec<String>,
    /// Tracks which units consumed which dep's outputs (unit_idx, dep_name).
    pub dep_edges: Vec<(usize, String)>,
}
```

Update `CaptureState::new()` to initialize these fields.

- [ ] **Step 6: Run all tests**

Run: `cargo test --workspace`
Expected: All pass (new fields are initialized empty, no behavior change).

- [ ] **Step 7: Commit**

```bash
git add cli/crates/cook-contracts/src/lib.rs cli/crates/cook-register/src/lib.rs cli/crates/cook-register/src/engine.rs cli/crates/cook-engine/src/dag_builder.rs
git commit -m "feat(contracts): add terminal_outputs and dep_edges to RecipeUnits"
```

---

### Task 6: Implement `cook.dep_output()` and `cook.dep_output_list()` Lua API

**Files:**
- Create: `cli/crates/cook-register/src/dep_output_api.rs`
- Modify: `cli/crates/cook-register/src/engine.rs`
- Modify: `cli/crates/cook-register/src/lib.rs`

- [ ] **Step 1: Write failing tests**

Create `cli/crates/cook-register/src/dep_output_api.rs`:

```rust
use mlua::prelude::*;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use crate::SharedCaptureState;

/// Shared storage for terminal outputs of registered recipes.
/// Key: recipe name, Value: list of terminal output paths.
pub type SharedTerminalOutputs = Rc<RefCell<BTreeMap<String, Vec<String>>>>;

/// Register `cook.dep_output(name)` and `cook.dep_output_list(name)` on the cook table.
///
/// - `cook.dep_output(name)` — returns terminal outputs as a space-joined string.
/// - `cook.dep_output_list(name)` — returns terminal outputs as a Lua table.
///
/// Both record the dependency edge in capture_state for fine-grained DAG wiring.
pub fn register_dep_output_api(
    lua: &Lua,
    terminal_outputs: SharedTerminalOutputs,
    capture_state: SharedCaptureState,
) -> LuaResult<()> {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CaptureState;

    fn setup_lua() -> (Lua, SharedTerminalOutputs, SharedCaptureState) {
        let lua = Lua::new();
        lua.globals().set("cook", lua.create_table().unwrap()).unwrap();
        let terminal_outputs: SharedTerminalOutputs =
            Rc::new(RefCell::new(BTreeMap::new()));
        let capture_state: SharedCaptureState =
            Rc::new(RefCell::new(CaptureState::new()));
        (lua, terminal_outputs, capture_state)
    }

    #[test]
    fn test_dep_output_returns_space_joined() {
        let (lua, outputs, cs) = setup_lua();
        outputs.borrow_mut().insert(
            "protos".into(),
            vec!["gen/foo.pb.o".into(), "gen/bar.pb.o".into()],
        );
        register_dep_output_api(&lua, outputs, cs).unwrap();

        let result: String = lua
            .load(r#"return cook.dep_output("protos")"#)
            .eval()
            .unwrap();
        assert_eq!(result, "gen/foo.pb.o gen/bar.pb.o");
    }

    #[test]
    fn test_dep_output_list_returns_table() {
        let (lua, outputs, cs) = setup_lua();
        outputs.borrow_mut().insert(
            "libmath".into(),
            vec!["build/lib/libmath.a".into()],
        );
        register_dep_output_api(&lua, outputs, cs).unwrap();

        let result: Vec<String> = lua
            .load(r#"return cook.dep_output_list("libmath")"#)
            .eval()
            .unwrap();
        assert_eq!(result, vec!["build/lib/libmath.a"]);
    }

    #[test]
    fn test_dep_output_unknown_recipe_errors() {
        let (lua, outputs, cs) = setup_lua();
        register_dep_output_api(&lua, outputs, cs).unwrap();

        let result = lua
            .load(r#"return cook.dep_output("nonexistent")"#)
            .eval::<String>();
        assert!(result.is_err());
    }

    #[test]
    fn test_dep_output_records_edge() {
        let (lua, outputs, cs) = setup_lua();
        outputs.borrow_mut().insert(
            "libmath".into(),
            vec!["build/lib/libmath.a".into()],
        );
        // Simulate being inside a unit capture by pre-adding a unit
        {
            let mut state = cs.borrow_mut();
            state.units.push(cook_contracts::CapturedUnit {
                payload: cook_contracts::WorkPayload::Shell {
                    cmd: "placeholder".into(),
                    line: 0,
                },
                cache_meta: None,
                dep_kind: cook_contracts::DepKind::Sequential,
            });
        }
        register_dep_output_api(&lua, outputs, cs.clone()).unwrap();

        lua.load(r#"cook.dep_output("libmath")"#).exec().unwrap();

        let state = cs.borrow();
        assert_eq!(state.dep_edges.len(), 1);
        // Edge points to the unit that was current when dep_output was called
        assert_eq!(state.dep_edges[0].1, "libmath");
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p cook-register dep_output`
Expected: FAIL — `todo!()` panics.

- [ ] **Step 3: Implement register_dep_output_api**

```rust
pub fn register_dep_output_api(
    lua: &Lua,
    terminal_outputs: SharedTerminalOutputs,
    capture_state: SharedCaptureState,
) -> LuaResult<()> {
    let cook: LuaTable = lua.globals().get("cook")?;

    // cook.dep_output(name) → space-joined string
    let to = terminal_outputs.clone();
    let cs = capture_state.clone();
    let dep_output_fn = lua.create_function(move |_, name: String| {
        let store = to.borrow();
        let outputs = store.get(&name).ok_or_else(|| {
            mlua::Error::RuntimeError(format!(
                "recipe '{}' has no terminal output (not registered or has no cook steps)",
                name
            ))
        })?;

        // Record the dep edge: current unit index → dep name
        {
            let mut state = cs.borrow_mut();
            let unit_idx = state.units.len().saturating_sub(1);
            state.dep_edges.push((unit_idx, name.clone()));
        }

        Ok(outputs.join(" "))
    })?;
    cook.set("dep_output", dep_output_fn)?;

    // cook.dep_output_list(name) → Lua table
    let to2 = terminal_outputs.clone();
    let cs2 = capture_state.clone();
    let dep_output_list_fn = lua.create_function(move |lua, name: String| {
        let store = to2.borrow();
        let outputs = store.get(&name).ok_or_else(|| {
            mlua::Error::RuntimeError(format!(
                "recipe '{}' has no terminal output (not registered or has no cook steps)",
                name
            ))
        })?;

        // Record the dep edge
        {
            let mut state = cs2.borrow_mut();
            let unit_idx = state.units.len().saturating_sub(1);
            state.dep_edges.push((unit_idx, name.clone()));
        }

        let table = lua.create_table()?;
        for (i, path) in outputs.iter().enumerate() {
            table.set(i + 1, path.as_str())?;
        }
        Ok(table)
    })?;
    cook.set("dep_output_list", dep_output_list_fn)?;

    Ok(())
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p cook-register dep_output`
Expected: All PASS.

- [ ] **Step 5: Wire into engine.rs**

In `cli/crates/cook-register/src/engine.rs`:
1. Add a `terminal_outputs: SharedTerminalOutputs` field to `Registry`
2. Initialize it in `Registry::new()`
3. In `register_recipe()`, call `register_dep_output_api()` after other API registrations
4. After recipe function executes, extract terminal outputs from capture state and store them:

```rust
// After func.call::<()>(())?; — extract terminal outputs
let cap = capture_state.borrow();

// Terminal outputs = outputs from the last cook step group
// We need to identify which units belong to the last cook step.
// The last step_group's outputs are the terminal outputs.
let terminal_outputs_list: Vec<String> = cap.last_cook_step_outputs.clone();

// Store for downstream recipes
self.terminal_outputs.borrow_mut().insert(
    recipe_name.to_string(),
    terminal_outputs_list.clone(),
);

Ok(RecipeUnits {
    // ... existing fields ...
    terminal_outputs: terminal_outputs_list,
    dep_edges: cap.dep_edges.clone(),
})
```

Also add `pub mod dep_output_api;` to `cli/crates/cook-register/src/lib.rs`.

- [ ] **Step 6: Run all tests**

Run: `cargo test --workspace`
Expected: All pass.

- [ ] **Step 7: Commit**

```bash
git add cli/crates/cook-register/src/dep_output_api.rs cli/crates/cook-register/src/engine.rs cli/crates/cook-register/src/lib.rs
git commit -m "feat(register): implement cook.dep_output() API with terminal output storage"
```

---

### Task 7: Track terminal outputs during unit capture

**Files:**
- Modify: `cli/crates/cook-register/src/unit_api.rs`

The `cook.add_unit()` function needs to track which outputs belong to the current cook step, so that after all steps run, we know the terminal (last cook step) outputs.

- [ ] **Step 1: Write failing test**

Add to `cli/crates/cook-register/src/unit_api.rs` test module:

```rust
#[test]
fn test_last_cook_step_outputs_tracked() {
    let (lua, capture_state) = make_lua_with_unit_api("recipe");
    lua.load(r#"
        -- First cook step (OneToOne produces 2 outputs)
        cook.step_group(function()
            cook.add_unit({ command = "gcc -c a.c -o a.o", inputs = {"a.c"}, output = "a.o" })
            cook.add_unit({ command = "gcc -c b.c -o b.o", inputs = {"b.c"}, output = "b.o" })
        end)
        -- Second cook step (ManyToOne produces 1 output)
        cook.step_group(function()
            cook.add_unit({ command = "ar rcs lib.a a.o b.o", inputs = {"a.o", "b.o"}, output = "lib.a" })
        end)
    "#).exec().unwrap();

    let state = capture_state.borrow();
    // Terminal outputs should be from the LAST step group: ["lib.a"]
    assert_eq!(state.last_cook_step_outputs, vec!["lib.a"]);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p cook-register test_last_cook_step_outputs`
Expected: FAIL.

- [ ] **Step 3: Update unit_api to track outputs per step group**

In `cook.add_unit()` handler, when a unit has an output and is inside a step group, record the output. In `cook.step_group()` handler, reset/capture the "current step outputs" after the group function completes, updating `last_cook_step_outputs`:

In the `step_group` handler:
```rust
// After func.call::<()>(()) and clearing current_group:
{
    let mut state = cs2.borrow_mut();
    state.current_group = None;
    // Capture outputs from this step group as potential terminal outputs.
    // Each step_group call overwrites — so the last one wins.
    state.last_cook_step_outputs = state.current_step_outputs.drain(..).collect();
}
```

In `add_unit`, when output is present:
```rust
if let Some(ref out) = output {
    state.current_step_outputs.push(out.clone());
}
```

Add `current_step_outputs: Vec<String>` to `CaptureState` and initialize in `new()`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p cook-register test_last_cook_step_outputs`
Expected: PASS.

- [ ] **Step 5: Run full test suite**

Run: `cargo test --workspace`
Expected: All pass.

- [ ] **Step 6: Commit**

```bash
git add cli/crates/cook-register/src/unit_api.rs cli/crates/cook-register/src/lib.rs
git commit -m "feat(register): track terminal outputs during step group capture"
```

---

## Chunk 4: Engine — Wave Grouping and DAG Wiring

### Task 8: Wave grouping from two dependency types

**Files:**
- Create: `cli/crates/cook-engine/src/wave_grouper.rs`
- Modify: `cli/crates/cook-engine/src/lib.rs`

This module takes `: dep` edges and `{dep}` edges and produces wave assignments: recipes connected by `{dep}` chains are merged into the same wave, `: dep` edges create wave boundaries.

- [ ] **Step 1: Write failing tests**

Create `cli/crates/cook-engine/src/wave_grouper.rs`:

```rust
use std::collections::{BTreeMap, BTreeSet};

/// A wave: a set of recipes to register and execute together.
/// Recipes within a wave are ordered by their `{dep}` toposort.
#[derive(Debug, Clone)]
pub struct Wave {
    /// Recipes in this wave, in toposorted order (dependencies first).
    pub recipes: Vec<String>,
}

/// Given explicit dep edges (wave boundaries) and inferred dep edges (same-wave),
/// compute the wave assignment for all recipes.
///
/// Returns waves in execution order: wave 0 runs first, wave 1 after, etc.
pub fn compute_waves(
    explicit_deps: &BTreeMap<String, Vec<String>>,
    inferred_deps: &BTreeMap<String, Vec<String>>,
    all_recipes: &BTreeSet<String>,
) -> Result<Vec<Wave>, String> {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_no_deps_single_wave() {
        let explicit = BTreeMap::new();
        let inferred = BTreeMap::new();
        let recipes = BTreeSet::from(["a".into(), "b".into(), "c".into()]);
        let waves = compute_waves(&explicit, &inferred, &recipes).unwrap();
        assert_eq!(waves.len(), 1);
        assert_eq!(waves[0].recipes.len(), 3);
    }

    #[test]
    fn test_explicit_dep_creates_wave_boundary() {
        // run: app (explicit dep)
        let mut explicit = BTreeMap::new();
        explicit.insert("run".to_string(), vec!["app".to_string()]);
        let inferred = BTreeMap::new();
        let recipes = BTreeSet::from(["app".into(), "run".into()]);
        let waves = compute_waves(&explicit, &inferred, &recipes).unwrap();
        assert_eq!(waves.len(), 2);
        assert!(waves[0].recipes.contains(&"app".to_string()));
        assert!(waves[1].recipes.contains(&"run".to_string()));
    }

    #[test]
    fn test_inferred_dep_same_wave() {
        // app uses {libmath} and {libstr}
        let explicit = BTreeMap::new();
        let mut inferred = BTreeMap::new();
        inferred.insert(
            "app".to_string(),
            vec!["libmath".to_string(), "libstr".to_string()],
        );
        let recipes = BTreeSet::from(["libmath".into(), "libstr".into(), "app".into()]);
        let waves = compute_waves(&explicit, &inferred, &recipes).unwrap();
        assert_eq!(waves.len(), 1);
        // Within the wave, libmath and libstr should come before app
        let app_pos = waves[0].recipes.iter().position(|r| r == "app").unwrap();
        let math_pos = waves[0].recipes.iter().position(|r| r == "libmath").unwrap();
        let str_pos = waves[0].recipes.iter().position(|r| r == "libstr").unwrap();
        assert!(math_pos < app_pos);
        assert!(str_pos < app_pos);
    }

    #[test]
    fn test_transitive_inferred_deps_collapse() {
        // server uses {core}, core uses {protos}
        let explicit = BTreeMap::new();
        let mut inferred = BTreeMap::new();
        inferred.insert("core".to_string(), vec!["protos".to_string()]);
        inferred.insert("server".to_string(), vec!["core".to_string()]);
        let recipes = BTreeSet::from(["protos".into(), "core".into(), "server".into()]);
        let waves = compute_waves(&explicit, &inferred, &recipes).unwrap();
        assert_eq!(waves.len(), 1, "all should be in one wave");
        // Order: protos, core, server
        let order: Vec<&str> = waves[0].recipes.iter().map(|s| s.as_str()).collect();
        assert!(order.iter().position(|&r| r == "protos").unwrap()
            < order.iter().position(|&r| r == "core").unwrap());
        assert!(order.iter().position(|&r| r == "core").unwrap()
            < order.iter().position(|&r| r == "server").unwrap());
    }

    #[test]
    fn test_mixed_explicit_and_inferred() {
        // libmath, libstr, app uses {libmath} {libstr}, run: app
        let mut explicit = BTreeMap::new();
        explicit.insert("run".to_string(), vec!["app".to_string()]);
        let mut inferred = BTreeMap::new();
        inferred.insert(
            "app".to_string(),
            vec!["libmath".to_string(), "libstr".to_string()],
        );
        let recipes = BTreeSet::from([
            "libmath".into(), "libstr".into(), "app".into(), "run".into(),
        ]);
        let waves = compute_waves(&explicit, &inferred, &recipes).unwrap();
        assert_eq!(waves.len(), 2);
        // Wave 1: libmath, libstr, app (same wave due to inferred deps)
        assert_eq!(waves[0].recipes.len(), 3);
        // Wave 2: run
        assert_eq!(waves[1].recipes, vec!["run".to_string()]);
    }

    #[test]
    fn test_inferred_cycle_detected() {
        let explicit = BTreeMap::new();
        let mut inferred = BTreeMap::new();
        inferred.insert("a".to_string(), vec!["b".to_string()]);
        inferred.insert("b".to_string(), vec!["a".to_string()]);
        let recipes = BTreeSet::from(["a".into(), "b".into()]);
        let result = compute_waves(&explicit, &inferred, &recipes);
        assert!(result.is_err());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p cook-engine wave_grouper`
Expected: FAIL.

- [ ] **Step 3: Implement compute_waves**

Algorithm:
1. Build a graph of explicit (wave boundary) edges.
2. Compute connected components using inferred edges (ignoring explicit edges). Recipes connected by `{dep}` chains belong to the same wave group.
3. Toposort the wave groups using explicit edges between groups.
4. Within each wave group, toposort recipes using inferred edges.

```rust
pub fn compute_waves(
    explicit_deps: &BTreeMap<String, Vec<String>>,
    inferred_deps: &BTreeMap<String, Vec<String>>,
    all_recipes: &BTreeSet<String>,
) -> Result<Vec<Wave>, String> {
    // 1. Build inferred-dep connected components using union-find or BFS.
    //    Recipes connected by {dep} chains are in the same group.
    let mut group_of: BTreeMap<String, usize> = BTreeMap::new();
    let mut groups: Vec<BTreeSet<String>> = Vec::new();

    for recipe in all_recipes {
        if !group_of.contains_key(recipe) {
            let group_idx = groups.len();
            let mut group = BTreeSet::new();
            let mut stack = vec![recipe.clone()];
            while let Some(r) = stack.pop() {
                if group.contains(&r) {
                    continue;
                }
                group.insert(r.clone());
                group_of.insert(r.clone(), group_idx);
                // Follow inferred edges in both directions
                if let Some(deps) = inferred_deps.get(&r) {
                    for d in deps {
                        if all_recipes.contains(d) {
                            stack.push(d.clone());
                        }
                    }
                }
                // Reverse edges
                for (other, deps) in inferred_deps {
                    if deps.contains(&r) && all_recipes.contains(other) {
                        stack.push(other.clone());
                    }
                }
            }
            groups.push(group);
        }
    }

    // 2. Build inter-group edges from explicit deps
    let mut group_deps: BTreeMap<usize, BTreeSet<usize>> = BTreeMap::new();
    for (recipe, deps) in explicit_deps {
        if let Some(&g1) = group_of.get(recipe) {
            for dep in deps {
                if let Some(&g2) = group_of.get(dep) {
                    if g1 != g2 {
                        group_deps.entry(g1).or_default().insert(g2);
                    }
                }
            }
        }
    }

    // 3. Toposort groups
    let num_groups = groups.len();
    let mut in_degree: Vec<usize> = vec![0; num_groups];
    for deps in group_deps.values() {
        for &d in deps {
            in_degree[d] += 1; // Wait, edges go from dependent TO dependency
        }
    }
    // Actually, explicit_deps maps recipe -> its deps. So if "run" depends on "app",
    // run's group must come AFTER app's group. group_deps[run_group] contains app_group.
    // For toposort: app_group must come first. So the edge is run_group -> app_group
    // meaning run_group depends on app_group.
    let mut in_degree: Vec<usize> = vec![0; num_groups];
    for (&g, deps) in &group_deps {
        // g depends on each dep group — so g has in-degree from deps
        in_degree[g] += deps.len(); // This is wrong, let me redo
    }

    // Redo: group_deps[g] = set of groups that g depends on.
    // For toposort, we need to process groups with 0 in-degree first.
    // in_degree[g] = number of groups that g depends on.
    let mut in_degree: Vec<usize> = vec![0; num_groups];
    for (&g, deps) in &group_deps {
        in_degree[g] = deps.len();
    }

    let mut queue: Vec<usize> = (0..num_groups)
        .filter(|&g| in_degree[g] == 0)
        .collect();
    let mut sorted_groups: Vec<usize> = Vec::new();

    while let Some(g) = queue.pop() {
        sorted_groups.push(g);
        // Find groups that depend on g and decrement their in-degree
        for (&dependent, deps) in &group_deps {
            if deps.contains(&g) {
                in_degree[dependent] -= 1;
                if in_degree[dependent] == 0 {
                    queue.push(dependent);
                }
            }
        }
    }

    if sorted_groups.len() != num_groups {
        return Err("cycle detected in explicit dependency graph".into());
    }

    // 4. For each group, toposort its recipes using inferred edges
    let mut waves = Vec::new();
    for &group_idx in &sorted_groups {
        let group = &groups[group_idx];
        let recipes = toposort_within_group(group, inferred_deps)?;
        waves.push(Wave { recipes });
    }

    Ok(waves)
}

fn toposort_within_group(
    group: &BTreeSet<String>,
    inferred_deps: &BTreeMap<String, Vec<String>>,
) -> Result<Vec<String>, String> {
    // Standard DFS toposort restricted to recipes in this group
    let mut state: BTreeMap<&str, u8> = BTreeMap::new(); // 0=unvisited, 1=visiting, 2=visited
    let mut result: Vec<String> = Vec::new();

    for recipe in group {
        state.insert(recipe.as_str(), 0);
    }

    fn visit<'a>(
        recipe: &'a str,
        group: &BTreeSet<String>,
        inferred_deps: &BTreeMap<String, Vec<String>>,
        state: &mut BTreeMap<&'a str, u8>,
        result: &mut Vec<String>,
    ) -> Result<(), String> {
        match state.get(recipe) {
            Some(1) => return Err(format!("cycle detected involving '{}'", recipe)),
            Some(2) => return Ok(()),
            _ => {}
        }
        state.insert(recipe, 1);
        if let Some(deps) = inferred_deps.get(recipe) {
            for dep in deps {
                if group.contains(dep) {
                    visit(dep, group, inferred_deps, state, result)?;
                }
            }
        }
        state.insert(recipe, 2);
        result.push(recipe.to_string());
        Ok(())
    }

    for recipe in group {
        if state.get(recipe.as_str()) == Some(&0) {
            visit(recipe, group, inferred_deps, &mut state, &mut result)?;
        }
    }

    Ok(result)
}
```

Note: The above is pseudocode-level Rust. The implementing agent should clean up lifetime issues and make it compile. The algorithm is: union-find for grouping → toposort groups by explicit edges → toposort within each group by inferred edges.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p cook-engine wave_grouper`
Expected: All PASS.

- [ ] **Step 5: Wire into engine lib.rs**

Add `pub mod wave_grouper;` to `cli/crates/cook-engine/src/lib.rs`.

- [ ] **Step 6: Commit**

```bash
git add cli/crates/cook-engine/src/wave_grouper.rs cli/crates/cook-engine/src/lib.rs
git commit -m "feat(engine): add wave_grouper for two-tier dependency grouping"
```

---

### Task 9: Fine-grained cross-recipe wiring in DAG builder

**Files:**
- Modify: `cli/crates/cook-engine/src/dag_builder.rs`

Update `build_dag` to use `dep_edges` from `RecipeUnits` for precise cross-recipe wiring instead of the coarse "root depends on all leaves" approach.

- [ ] **Step 1: Write failing test**

Add to `cli/crates/cook-engine/src/dag_builder.rs` test module:

```rust
#[test]
fn test_fine_grained_cross_recipe_deps() {
    // libmath: compile (group of 2) -> archive (sequential)
    let libmath = RecipeUnits {
        recipe_name: "libmath".into(),
        deps: vec![],
        units: vec![
            CapturedUnit {
                payload: shell("gcc -c add.c"),
                cache_meta: None,
                dep_kind: DepKind::StepGroup(0),
            },
            CapturedUnit {
                payload: shell("gcc -c mul.c"),
                cache_meta: None,
                dep_kind: DepKind::StepGroup(0),
            },
            CapturedUnit {
                payload: shell("ar rcs libmath.a"),
                cache_meta: None,
                dep_kind: DepKind::Sequential,
            },
        ],
        step_groups: vec![vec![0, 1]],
        working_dir: default_wd(),
        env_vars: default_env(),
        terminal_outputs: vec!["libmath.a".into()],
        dep_edges: vec![],
    };

    // app: compile (1 unit) -> link (1 unit, depends on libmath)
    let app = RecipeUnits {
        recipe_name: "app".into(),
        deps: vec![],
        units: vec![
            CapturedUnit {
                payload: shell("gcc -c main.c"),
                cache_meta: None,
                dep_kind: DepKind::StepGroup(0),
            },
            CapturedUnit {
                payload: shell("gcc -o app main.o libmath.a"),
                cache_meta: None,
                dep_kind: DepKind::Sequential,
            },
        ],
        step_groups: vec![vec![0]],
        working_dir: default_wd(),
        env_vars: default_env(),
        terminal_outputs: vec!["app".into()],
        // Unit 1 (the link step) depends on libmath
        dep_edges: vec![(1, "libmath".into())],
    };

    let dag = build_dag(vec![libmath, app]);
    assert_eq!(dag.len(), 5);

    // libmath units: nodes 0, 1, 2
    // app units: nodes 3, 4

    // app's compile step (node 3) should have 0 cross-recipe deps
    // — it can start immediately in parallel with libmath
    assert_eq!(
        dag.node(3).remaining_deps.load(Ordering::Relaxed),
        0,
        "app compile should have no deps (can run parallel with libmath)"
    );

    // app's link step (node 4) should depend on:
    // - node 3 (within-recipe sequential barrier from app's compile)
    // - node 2 (libmath's archive step = terminal output producer)
    assert_eq!(
        dag.node(4).remaining_deps.load(Ordering::Relaxed),
        2,
        "app link should depend on app compile + libmath archive"
    );
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p cook-engine test_fine_grained_cross`
Expected: FAIL — current build_dag uses coarse wiring.

- [ ] **Step 3: Implement fine-grained wiring**

Update `build_dag` in `dag_builder.rs`:

1. First pass: register all units from all recipes, building a map of `recipe_name -> Vec<dag_node_id>` and `recipe_name -> Vec<dag_node_id>` for terminal output producers (the last step group's nodes).

2. For cross-recipe deps, instead of connecting root units to all leaves, use `dep_edges`: for each `(unit_idx, dep_recipe_name)` in a recipe's `dep_edges`, connect that specific unit's dag node to the terminal output producer nodes of the dep recipe.

The key change is in how `all_deps` is computed for each unit:

```rust
// Instead of:
let all_deps = if within_deps.is_empty() {
    cross_deps.clone()
} else {
    within_deps
};

// Use:
let mut all_deps = within_deps;
// Add fine-grained cross-recipe deps for this specific unit
for (dep_unit_idx, dep_recipe_name) in &ru.dep_edges {
    if *dep_unit_idx == unit_idx {
        if let Some(terminal_nodes) = recipe_terminal_nodes.get(dep_recipe_name) {
            all_deps.extend(terminal_nodes);
        }
    }
}
```

Where `recipe_terminal_nodes` maps recipe names to the dag node IDs of their terminal output producers.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p cook-engine test_fine_grained_cross && cargo test -p cook-engine dag_builder`
Expected: All PASS.

- [ ] **Step 5: Commit**

```bash
git add cli/crates/cook-engine/src/dag_builder.rs
git commit -m "feat(engine): fine-grained cross-recipe DAG wiring via dep_edges"
```

---

### Task 10: Refactor run loop for multi-recipe waves

**Files:**
- Modify: `cli/crates/cook-engine/src/run.rs`

Update the wave loop to use `wave_grouper` for computing waves, register all recipes in a wave together, and build a single DAG per wave.

- [ ] **Step 1: Write failing test**

Add to `cli/crates/cook-engine/src/run.rs` test module:

```rust
#[test]
fn test_run_respects_wave_grouping() {
    // This is a structural test — verifying the run function accepts
    // the new inferred_deps parameter. Full integration tested in Task 14.
    // For now, just verify the API compiles and handles empty inferred deps.
    let mut recipes = BTreeMap::new();
    recipes.insert(
        "build".to_string(),
        RecipeInfo {
            ingredients: vec![],
            serves: vec![],
            requires: vec![],
        },
    );
    let registries = BTreeMap::new();
    let inferred = BTreeMap::new();
    let result = run(
        &recipes,
        &["build".to_string()],
        &registries,
        1,
        &inferred,
        |_| {},
    );
    // Will fail because no registry for "" prefix, but that's expected
    assert!(result.is_err());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p cook-engine test_run_respects_wave`
Expected: FAIL — `run()` doesn't accept `inferred_deps` parameter yet.

- [ ] **Step 3: Update run() signature and implementation**

Add `inferred_deps: &BTreeMap<String, Vec<String>>` parameter to `run()`.

Replace the wave loop logic:

```rust
// OLD: uses RecipeDag for wave scheduling based on explicit deps only
// NEW: uses wave_grouper::compute_waves for two-tier scheduling

use crate::wave_grouper;

// Compute waves from both explicit and inferred deps
let all_recipe_names: BTreeSet<String> = edges.keys().cloned().collect();
let waves = wave_grouper::compute_waves(&edges, inferred_deps, &all_recipe_names)
    .map_err(|e| EngineError::CycleDetected(e))?;

for wave in &waves {
    // Register ALL recipes in this wave in toposorted order
    let mut wave_units = Vec::new();
    let mut wave_cache_managers = BTreeMap::new();

    for name in &wave.recipes {
        // ... existing registration logic per recipe ...
        // (same as current wave loop body)
    }

    // Build ONE DAG for the entire wave
    let dag = dag_builder::build_dag(wave_units);
    if !dag.is_empty() {
        // ... existing execution logic ...
    }
}
```

- [ ] **Step 4: Update all callers of run()**

In `cli/crates/cook-cli/src/pipeline.rs`, update calls to `run()` to pass the new `inferred_deps` parameter. For now, pass `&BTreeMap::new()` — the pipeline wiring happens in Task 12.

Also update `cmd_dag` if it calls into the engine directly.

- [ ] **Step 5: Run all tests**

Run: `cargo test --workspace`
Expected: All pass.

- [ ] **Step 6: Commit**

```bash
git add cli/crates/cook-engine/src/run.rs cli/crates/cook-cli/src/pipeline.rs
git commit -m "feat(engine): refactor run loop for multi-recipe wave execution"
```

---

## Chunk 5: Pipeline Integration and Example

### Task 11: Wire pre-scan into pipeline

**Files:**
- Modify: `cli/crates/cook-cli/src/pipeline.rs`

Connect the pre-scan (recipe name extraction + dep ref extraction) to codegen and engine.

- [ ] **Step 1: Update read_and_parse to use generate_with_names**

```rust
pub fn read_and_parse(cli: &Cli) -> Result<(cook_lang::ast::Cookfile, String), CookError> {
    let source = std::fs::read_to_string(&cli.file)
        .map_err(|e| CookError::Other(format!("cannot read {}: {e}", cli.file.display())))?;

    let cookfile =
        cook_lang::parse(&source).map_err(|e| CookError::ParseError(e.to_string()))?;

    // Pre-scan: extract recipe names for codegen disambiguation
    let recipe_names = cook_luagen::dep_ref::extract_recipe_names(&cookfile);
    let lua_source = cook_luagen::generate_with_names(&cookfile, &recipe_names);

    Ok((cookfile, lua_source))
}
```

- [ ] **Step 2: Extract inferred deps and pass to engine**

In `cmd_run`, after building recipe_infos, extract inferred deps from the Cookfile:

```rust
let recipe_names = cook_luagen::dep_ref::extract_recipe_names(&cookfile);
let mut inferred_deps: BTreeMap<String, Vec<String>> = BTreeMap::new();
for recipe in &cookfile.recipes {
    let refs = cook_luagen::dep_ref::extract_dep_refs(recipe, &recipe_names);
    let dep_names: Vec<String> = refs
        .iter()
        .map(|r| r.recipe_name.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    if !dep_names.is_empty() {
        inferred_deps.insert(recipe.name.clone(), dep_names);
    }
}

// Pass to engine
run_with_progress(cli, &recipe_infos, &targets, &registries, num_jobs, &inferred_deps)?;
```

Update `run_with_progress` to accept and forward the `inferred_deps` parameter.

- [ ] **Step 3: Add warning for both `: dep` and `{dep}` on same dependency**

After extracting both explicit deps and inferred deps, check for overlap:

```rust
for recipe in &cookfile.recipes {
    let refs = cook_luagen::dep_ref::extract_dep_refs(recipe, &recipe_names);
    for dep_ref in &refs {
        if recipe.deps.contains(&dep_ref.recipe_name) {
            eprintln!(
                "cook: warning: recipe '{}' has both explicit ': {}' and inferred '{{{}}}' \
                 dependency — conflicting scheduling intent",
                recipe.name, dep_ref.recipe_name, dep_ref.recipe_name
            );
        }
    }
}
```

- [ ] **Step 4: Run cargo check**

Run: `cargo check --workspace`
Expected: Compiles.

- [ ] **Step 5: Run full test suite**

Run: `cargo test --workspace`
Expected: All pass.

- [ ] **Step 6: Commit**

```bash
git add cli/crates/cook-cli/src/pipeline.rs
git commit -m "feat(cli): wire pre-scan into pipeline, pass inferred deps to engine"
```

---

### Task 12: Update cross-recipe-deps example

**Files:**
- Modify: `examples/cross-recipe-deps/Cookfile`

- [ ] **Step 1: Update the Cookfile to use `{dep}` syntax**

```
# Cross-recipe dependency example — demonstrates {dep} references.
#
# Recipes reference other recipes' outputs via {recipe_name} syntax.
# The engine infers dependency edges and wires fine-grained unit-level
# dependencies automatically — no explicit : dep syntax needed.

recipe libmath
    ingredients "src/math/*.c"
    cook "build/obj/math/{stem}.o" using "mkdir -p build/obj/math && gcc -c {in} -Isrc -o {out}"
    cook "build/lib/libmath.a" using "mkdir -p build/lib && ar rcs {out} {all}"
end

recipe libstr
    ingredients "src/str/*.c"
    cook "build/obj/str/{stem}.o" using "mkdir -p build/obj/str && gcc -c {in} -Isrc -o {out}"
    cook "build/lib/libstr.a" using "mkdir -p build/lib && ar rcs {out} {all}"
end

recipe app
    ingredients "src/main.c"
    cook "build/obj/main.o" using "mkdir -p build/obj && gcc -c {in} -Isrc -o {out}"
    cook "build/bin/app" using "mkdir -p build/bin && gcc -o {out} {in} {libmath} {libstr}"
end

recipe run: app
    test "build/bin/app"
end

recipe clean
    rm -rf build .cook
end
```

Note: `recipe run: app` keeps the explicit `: dep` because `run` doesn't use `{app}` in its command — it just needs `app` to finish first.

- [ ] **Step 2: Test the example builds**

Run from repo root:
```bash
cd examples/cross-recipe-deps && ../../target/debug/cook run && echo "SUCCESS"
```

Expected: Builds and runs the app successfully.

- [ ] **Step 3: Verify the warning doesn't fire** (no overlap in this example)

Run: Build output should NOT contain any "conflicting scheduling intent" warning.

- [ ] **Step 4: Commit**

```bash
git add examples/cross-recipe-deps/Cookfile
git commit -m "feat(example): convert cross-recipe-deps to use {dep} syntax"
```

---

### Task 13: Integration test

**Files:**
- Create: `cli/tests/cross_recipe_deps.rs` (or add to existing integration test file)

- [ ] **Step 1: Write integration test**

Find the existing integration test structure. Add a test that:
1. Parses the cross-recipe-deps example Cookfile
2. Verifies pre-scan extracts the correct dep refs
3. Verifies codegen emits `cook.dep_output` calls
4. Verifies wave grouping puts libmath, libstr, app in one wave and run in another

```rust
#[test]
fn test_cross_recipe_deps_example() {
    let source = std::fs::read_to_string("../examples/cross-recipe-deps/Cookfile")
        .expect("cross-recipe-deps example should exist");

    let cookfile = cook_lang::parse(&source).expect("should parse");
    let recipe_names = cook_luagen::dep_ref::extract_recipe_names(&cookfile);

    // Verify recipe names extracted
    assert!(recipe_names.contains("libmath"));
    assert!(recipe_names.contains("libstr"));
    assert!(recipe_names.contains("app"));

    // Verify dep refs extracted for app
    let app_recipe = cookfile.recipes.iter().find(|r| r.name == "app").unwrap();
    let app_refs = cook_luagen::dep_ref::extract_dep_refs(app_recipe, &recipe_names);
    let dep_names: Vec<String> = app_refs.iter().map(|r| r.recipe_name.clone()).collect();
    assert!(dep_names.contains(&"libmath".to_string()));
    assert!(dep_names.contains(&"libstr".to_string()));

    // Verify codegen emits dep_output calls
    let lua = cook_luagen::generate_with_names(&cookfile, &recipe_names);
    assert!(lua.contains(r#"cook.dep_output("libmath")"#));
    assert!(lua.contains(r#"cook.dep_output("libstr")"#));
}
```

- [ ] **Step 2: Run integration test**

Run: `cargo test --test integration cross_recipe_deps` (or whatever the test file is named)
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add cli/tests/
git commit -m "test: add integration test for cross-recipe dependency inference"
```

---

## Summary of task dependencies

```
Task 1 (reserved names)        ─────────────────────────────────┐
Task 2 (pre-scan/dep_ref)      ─────────┐                       │
                                         ├── Task 3 (template expansion)
                                         │         │
                                         │         ├── Task 4 (dep-driven iteration codegen)
                                         │         │
Task 5 (contracts + capture)   ──────────┤         │
                                         ├── Task 6 (dep_output Lua API)
                                         │         │
                                         ├── Task 7 (terminal output tracking)
                                         │
Task 8 (wave grouper)         ───────────┤
                                         ├── Task 9 (fine-grained DAG wiring)
                                         │
                                         ├── Task 10 (run loop refactor)
                                         │
                                         ├── Task 11 (pipeline wiring)
                                         │         │
                                         │         ├── Task 12 (update example)
                                         │         │
                                         │         └── Task 13 (integration test)
```

Tasks 1, 2, 5, 8 can be started in parallel (no dependencies between them).
