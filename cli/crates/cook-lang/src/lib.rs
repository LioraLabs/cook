pub mod ast;
pub mod lexer;
pub(crate) mod brace_scan;
pub(crate) mod cook_line;
pub(crate) mod lua_block;
pub(crate) mod recipe;
pub(crate) mod shell_block;

/// The Cook Standard version this crate claims to fully implement.
///
/// "Fully implement" means every case under `standard/conformance/` (relative
/// to the workspace root, or under `$COOK_CONFORMANCE_CORPUS` if set) passes
/// the conformance harness in `tests/conformance.rs`.
///
/// Move this constant in lockstep with `standard/VERSION` when the parser
/// catches up to a new cut. See `cli/crates/cook-lang/CONFORMANCE.md`.
pub const COOK_STANDARD_VERSION: &str = "0.9";

use ast::*;
use lexer::*;
use recipe::{parse_chore, parse_config_block_lua, parse_recipe, parse_register_block_lua};
use std::collections::BTreeMap;
use thiserror::Error;

#[derive(Error, Debug)]
#[non_exhaustive]
pub enum ParseError {
    #[error("{0}")]
    Lex(#[from] LexError),
    #[error("line {line}: {message}")]
    Parse { line: usize, message: String },
}

/// Validate an import path token and classify it as tree-relative or sigil-anchored.
/// Per §7.2, returns Err for paths containing `..`, absolute paths (other than `//` sigils),
/// and sigil paths with `..` after the sigil.
fn validate_and_classify_import_path(raw: &str, line: usize) -> Result<ast::ImportPath, ParseError> {
    if let Some(after_sigil) = raw.strip_prefix("//") {
        // Sigil-anchored. Reject `..` segments after the sigil and leading `/`.
        if after_sigil.starts_with('/') {
            return Err(ParseError::Parse {
                line,
                message: format!(
                    "import path '{raw}': '/' immediately after '//' is not permitted"
                ),
            });
        }
        if path_contains_dotdot_segment(after_sigil) {
            return Err(ParseError::Parse {
                line,
                message: format!(
                    "import path '{raw}': '..' segments are not permitted after '//'"
                ),
            });
        }
        return Ok(ast::ImportPath::Sigil(after_sigil.to_string()));
    }
    // Tree-relative. Reject absolute paths (anything starting with '/' that is not '//').
    if raw.starts_with('/') {
        return Err(ParseError::Parse {
            line,
            message: format!(
                "import path '{raw}': absolute paths are not permitted; tree-relative or '//' sigil"
            ),
        });
    }
    if path_contains_dotdot_segment(raw) {
        return Err(ParseError::Parse {
            line,
            message: format!(
                "import path '{raw}': '..' segments are not permitted; use the workspace-root sigil '//path' for cross-cutting imports"
            ),
        });
    }
    Ok(ast::ImportPath::Tree(raw.to_string()))
}

/// Returns true if `path` contains a `..` segment (a `..` between path separators
/// or as the entire path or a trailing/leading segment). Does NOT match `..` inside
/// a longer segment like `..foo`.
fn path_contains_dotdot_segment(path: &str) -> bool {
    path.split('/').any(|seg| seg == "..")
}

/// Kind of a callable declaration, recorded in the duplicate-name map.
///
/// Recipes and chores share a single callable namespace (App. A.2,
/// "Duplicate recipe / chore declaration name rule"): two declarations of
/// either kind that share a name are rejected at parse time, including
/// recipe-vs-chore collisions.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum CallableKind {
    Recipe,
    Chore,
}

impl CallableKind {
    fn label(self) -> &'static str {
        match self {
            CallableKind::Recipe => "recipe",
            CallableKind::Chore => "chore",
        }
    }
}

/// Build the App. A.2 duplicate-declaration diagnostic, naming the kind and
/// line of the prior declaration. Same wording for all three collision modes
/// (recipe-vs-recipe, chore-vs-chore, recipe-vs-chore).
fn duplicate_callable_error(
    new_kind: CallableKind,
    name: &str,
    new_line: usize,
    prior_kind: CallableKind,
    prior_line: usize,
) -> ParseError {
    ParseError::Parse {
        line: new_line,
        message: format!(
            "{} '{}': duplicate declaration (already declared as {} at line {})",
            new_kind.label(),
            name,
            prior_kind.label(),
            prior_line,
        ),
    }
}

pub fn parse(source: &str) -> Result<Cookfile, ParseError> {
    let tokens = tokenize(source)?;
    let source_lines: Vec<&str> = source.lines().collect();
    let mut pos = 0;
    let mut recipes = Vec::new();
    let mut chores: Vec<ast::Chore> = Vec::new();
    let mut config_blocks: Vec<ConfigBlock> = Vec::new();
    let mut uses = Vec::new();
    let mut imports = Vec::new();
    let mut seen_recipe = false;
    let mut register_blocks: Vec<ast::RegisterBlock> = Vec::new();
    let mut top_level_module_calls: Vec<ast::TopLevelModuleCall> = Vec::new();
    // App. A.2 "Duplicate recipe / chore declaration name rule": recipes and
    // chores share a single callable namespace. Track every declaration so
    // we can reject any subsequent collision (recipe-vs-recipe,
    // chore-vs-chore, recipe-vs-chore) with a diagnostic naming both lines.
    let mut callable_decls: BTreeMap<String, (CallableKind, usize)> = BTreeMap::new();

    while pos < tokens.len() {
        let tok = &tokens[pos];
        match &tok.value {
            Token::Comment(_) | Token::Blank => {
                pos += 1;
            }
            Token::ConfigHeader { name } => {
                if seen_recipe {
                    return Err(ParseError::Parse {
                        line: tok.line,
                        message: "config blocks must appear before recipes and chores".to_string(),
                    });
                }
                let header_line = tok.line;
                let block_name = name.clone();
                // Validation: duplicates
                match &block_name {
                    None => {
                        if config_blocks.iter().any(|b| b.name.is_none()) {
                            return Err(ParseError::Parse {
                                line: header_line,
                                message: "multiple unnamed config blocks (only one allowed)".to_string(),
                            });
                        }
                    }
                    Some(n) => {
                        if config_blocks.iter().any(|b| b.name.as_deref() == Some(n)) {
                            return Err(ParseError::Parse {
                                line: header_line,
                                message: format!("duplicate config block '{}'", n),
                            });
                        }
                    }
                }
                pos += 1;
                let (body, new_pos) = parse_config_block_lua(&tokens, pos, header_line, &source_lines)?;
                config_blocks.push(ConfigBlock {
                    name: block_name,
                    body,
                    line: header_line,
                });
                pos = new_pos;
            }
            Token::RecipeHeader { name, deps } => {
                seen_recipe = true;
                let recipe_line = tok.line;
                let name = name.clone();
                let deps = deps.clone();
                if let Some(&(prior_kind, prior_line)) = callable_decls.get(&name) {
                    return Err(duplicate_callable_error(
                        CallableKind::Recipe,
                        &name,
                        recipe_line,
                        prior_kind,
                        prior_line,
                    ));
                }
                callable_decls.insert(name.clone(), (CallableKind::Recipe, recipe_line));
                pos += 1;
                let (recipe, new_pos) =
                    parse_recipe(name, deps, recipe_line, &tokens, pos, &source_lines)?;
                recipes.push(recipe);
                pos = new_pos;
            }
            Token::ChoreHeader { name, deps } => {
                seen_recipe = true;  // chores count toward the ordering rule
                let chore_line = tok.line;
                let name = name.clone();
                let deps = deps.clone();
                if let Some(&(prior_kind, prior_line)) = callable_decls.get(&name) {
                    return Err(duplicate_callable_error(
                        CallableKind::Chore,
                        &name,
                        chore_line,
                        prior_kind,
                        prior_line,
                    ));
                }
                callable_decls.insert(name.clone(), (CallableKind::Chore, chore_line));
                pos += 1;
                let (chore, new_pos) =
                    parse_chore(name, deps, chore_line, &tokens, pos, &source_lines)?;
                chores.push(chore);
                pos = new_pos;
            }
            Token::Content(text) => {
                // SHI-216 §3.7.5: a Content line whose first token matches the
                // module-call shape `<id>.<id>(...)` is a top-level module_call.
                // Anything else is rejected as before.
                if recipe::is_module_call(text) {
                    let header_line = tok.line;
                    let text_clone = text.clone();
                    let (code, new_pos) = recipe::collect_module_call(
                        &text_clone,
                        header_line,
                        &tokens,
                        pos,
                        &source_lines,
                    )?;
                    top_level_module_calls.push(ast::TopLevelModuleCall { code, line: header_line });
                    pos = new_pos;
                } else {
                    return Err(ParseError::Parse {
                        line: tok.line,
                        message: "unexpected content outside of a recipe".to_string(),
                    });
                }
            }
            Token::LuaLine(_)
            | Token::LuaBlockOpen
            | Token::InlineLuaLine(_)
            | Token::InlineLuaBlockOpen => {
                return Err(ParseError::Parse {
                    line: tok.line,
                    message: "unexpected content outside of a recipe".to_string(),
                });
            }
            Token::UseDecl { name } => {
                if seen_recipe {
                    return Err(ParseError::Parse {
                        line: tok.line,
                        message: "use statements must appear before recipes and chores".to_string(),
                    });
                }
                uses.push(ast::UseStatement {
                    module_name: name.clone(),
                    line: tok.line,
                });
                pos += 1;
            }
            Token::ImportDecl { name, path } => {
                if seen_recipe {
                    return Err(ParseError::Parse {
                        line: tok.line,
                        message: "import declarations must appear before recipes and chores".to_string(),
                    });
                }
                if imports.iter().any(|i: &ast::ImportDecl| i.name == *name) {
                    return Err(ParseError::Parse {
                        line: tok.line,
                        message: format!("duplicate import name '{}'", name),
                    });
                }
                let parsed_path = validate_and_classify_import_path(path, tok.line)?;
                imports.push(ast::ImportDecl {
                    name: name.clone(),
                    path: parsed_path,
                    line: tok.line,
                });
                pos += 1;
            }
            Token::RegisterHeader => {
                let header_line = tok.line;
                // Reject `register foo`: detect non-empty content after the keyword.
                let raw = source_lines
                    .get(header_line.saturating_sub(1))
                    .copied()
                    .unwrap_or("");
                let after_kw = raw.trim_start().strip_prefix("register").unwrap_or("");
                if !after_kw.trim().is_empty() {
                    return Err(ParseError::Parse {
                        line: header_line,
                        message: "register block takes no name; remove the trailing arguments".to_string(),
                    });
                }
                pos += 1;
                let (body, new_pos) =
                    parse_register_block_lua(&tokens, pos, header_line, &source_lines)?;
                register_blocks.push(ast::RegisterBlock { body, line: header_line });
                pos = new_pos;
            }
        }
    }

    Ok(Cookfile { config_blocks, recipes, chores, uses, imports, register_blocks, top_level_module_calls })
}

#[cfg(test)]
mod tests;
