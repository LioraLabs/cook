pub mod ast;
pub(crate) mod brace_scan;
pub(crate) mod cook_line;
pub(crate) mod disposition;
pub mod lexer;
pub(crate) mod lua_block;
pub(crate) mod probe;
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
pub const COOK_STANDARD_VERSION: &str = "0.18";

pub use brace_scan::shell_placeholder_contexts;

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
fn validate_and_classify_import_path(
    raw: &str,
    line: usize,
) -> Result<ast::ImportPath, ParseError> {
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

/// Kind of a declaration recorded in the duplicate-name map.
///
/// Recipes and chores share a single callable namespace (App. A.2,
/// "Duplicate recipe / chore declaration name rule"): two declarations of
/// either kind that share a name are rejected at parse time, including
/// recipe-vs-chore collisions.
///
/// CS-0240 puts probe keys in that namespace too, and for the reason it
/// exists. A probe key now resolves in §10.2's cascade, so `$<foo>` with both
/// a probe `foo` and a recipe `foo` in scope would have two readings, and
/// `$<foo.x>` three; rejecting the collision at load time is what lets the
/// dotted reading be decided by the base name's KIND rather than by a third
/// heuristic layered onto §10.2.1's two. Import aliases are in the namespace
/// for the same reason (`$<alias.recipe>` against probe `alias` field
/// `recipe`) and carry this label, but only probes are checked against them:
/// making recipe-vs-alias an error is a separate rule this change does not
/// need and would break Cookfiles CS-0240 leaves alone.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum CallableKind {
    Recipe,
    Chore,
    /// A `probe` block. `files` and `tools` mint keys in the same namespace
    /// but are their own kinds: App. A.2 requires the diagnostic to identify
    /// the offending kind, and "probe 'lib': duplicate declaration" for a line
    /// reading `files lib` names a keyword the author did not write.
    Probe,
    Files,
    Tools,
    Import,
}

impl CallableKind {
    fn label(self) -> &'static str {
        match self {
            CallableKind::Recipe => "recipe",
            CallableKind::Chore => "chore",
            CallableKind::Probe => "probe",
            CallableKind::Files => "files declaration",
            CallableKind::Tools => "tools declaration",
            CallableKind::Import => "import alias",
        }
    }
}

/// Build the App. A.2 duplicate-declaration diagnostic, naming the kind and
/// line of the prior declaration. One wording for every collision mode
/// (recipe/chore/probe in any order, and probe-vs-import-alias), so a reader
/// who has seen one has seen them all.
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

/// Record a top-level `probe` / `files` / `tools` declaration in the shared
/// name space, rejecting a collision with anything already declared there.
///
/// CS-0240. Called from the three declaration arms rather than folded into a
/// post-parse sweep so the diagnostic can name the prior declaration's line
/// whichever order the two appear in: probes carry no before-recipes ordering
/// rule, so both directions are reachable.
fn note_probe_decl(
    callable_decls: &mut BTreeMap<String, (CallableKind, usize)>,
    kind: CallableKind,
    name: &str,
    line: usize,
) -> Result<(), ParseError> {
    if let Some(&(prior_kind, prior_line)) = callable_decls.get(name) {
        return Err(duplicate_callable_error(kind, name, line, prior_kind, prior_line));
    }
    callable_decls.insert(name.to_string(), (kind, line));
    Ok(())
}

fn preceding_comment_block(tokens: &[Located<Token>], pos: usize) -> Option<String> {
    let mut lines = Vec::new();
    for token in tokens[..pos].iter().rev() {
        let Token::Comment(text) = &token.value else {
            break;
        };
        lines.push(text.trim().to_string());
    }
    (!lines.is_empty()).then(|| {
        lines.reverse();
        lines.join("\n")
    })
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
    let mut probes: Vec<ast::Probe> = Vec::new();
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
                let description = preceding_comment_block(&tokens, pos);
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
                let (mut recipe, inline_probes, new_pos) =
                    parse_recipe(name, deps, recipe_line, &tokens, pos, &source_lines)?;
                recipe.description = description;
                recipes.push(recipe);
                probes.extend(inline_probes);
                pos = new_pos;
            }
            Token::ChoreHeader { name, params, deps } => {
                seen_recipe = true;  // chores count toward the ordering rule
                let description = preceding_comment_block(&tokens, pos);
                let chore_line = tok.line;
                let name = name.clone();
                let params = params.clone();
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
                let (mut chore, new_pos) =
                    parse_chore(name, params, deps, chore_line, &tokens, pos, &source_lines)?;
                chore.description = description;
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
                    top_level_module_calls.push(ast::TopLevelModuleCall {
                        code,
                        line: header_line,
                    });
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
            Token::UseDecl { alias, target } => {
                if seen_recipe {
                    return Err(ParseError::Parse {
                        line: tok.line,
                        message: "use statements must appear before recipes and chores".to_string(),
                    });
                }
                // §12.1 (CS-0206): one identifier, one target. Two declarations
                // naming the SAME target are not a conflict — §12.3.2 already
                // makes the second a memo hit — but two that bind one alias to
                // two different things are the last-wins hazard C.11.1 refuses
                // for `import`, and it is cheapest to see at the line that
                // wrote it. Reachable in the name form too (`use a` twice is
                // fine; `use helpers` beside `use helpers ./x.lua` is not).
                if let Some(prior) = uses.iter().find(|u: &&ast::UseStatement| u.alias == *alias) {
                    if prior.target != *target {
                        return Err(ParseError::Parse {
                            line: tok.line,
                            message: format!(
                                "'use' binds '{}' to '{}' here, but line {} already bound it to '{}'; \
                                 give one of them an explicit alias",
                                alias, target, prior.line, prior.target
                            ),
                        });
                    }
                }
                uses.push(ast::UseStatement {
                    alias: alias.clone(),
                    target: target.clone(),
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
                // CS-0240: `var.` is a reserved placeholder namespace (§10.7)
                // and is resolved ahead of every lookup step, so a recipe
                // reached as `$<var.NAME>` through an alias spelled `var` is
                // unreachable by construction. The reservation already covers
                // recipe names; it did not cover the alias, which left
                // `import var sub` declaring recipes no reference could name.
                // Refused at the declaration, where the fix is renaming one
                // word, rather than at each use site.
                if name == "var" {
                    return Err(ParseError::Parse {
                        line: tok.line,
                        message: "import alias 'var': `var` is the reserved \
                                  declared-variable namespace (Standard \
                                  §10.7), so `$<var.NAME>` always names a \
                                  variable and can never reach a recipe \
                                  through this alias; choose another alias"
                            .to_string(),
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
            Token::ProbeHeader { name, deps } => {
                let probe_line = tok.line;
                let name = name.clone();
                if name.starts_with("@seal:") {
                    return Err(ParseError::Parse { line: probe_line, message: "probe: keys beginning `@seal:` are reserved for inline file determinants".into() });
                }
                note_probe_decl(&mut callable_decls, CallableKind::Probe, &name, probe_line)?;
                let deps = deps.clone();
                pos += 1;
                let (probe, inline_probes, new_pos) =
                    probe::parse_probe(name, deps, probe_line, &tokens, pos, &source_lines)?;
                probes.push(probe);
                probes.extend(inline_probes);
                pos = new_pos;
            }
            Token::FilesHeader { name } => {
                let line = tok.line; let name = name.clone(); pos += 1;
                if name.starts_with("@seal:") { return Err(ParseError::Parse { line, message: "files: keys beginning `@seal:` are reserved for inline file determinants".into() }); }
                note_probe_decl(&mut callable_decls, CallableKind::Files, &name, line)?;
                let (probe, new_pos) = probe::parse_files_declaration(name, line, &tokens, pos, &source_lines)?;
                probes.push(probe); pos = new_pos;
            }
            Token::ToolsHeader { name } => {
                let line = tok.line; let name = name.clone(); pos += 1;
                if name.starts_with("@seal:") { return Err(ParseError::Parse { line, message: "tools: keys beginning `@seal:` are reserved for inline file determinants".into() }); }
                note_probe_decl(&mut callable_decls, CallableKind::Tools, &name, line)?;
                let (probe, new_pos) = probe::parse_tools_declaration(name, line, &tokens, pos, &source_lines)?;
                probes.push(probe); pos = new_pos;
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
                register_blocks.push(ast::RegisterBlock {
                    body,
                    line: header_line,
                });
                pos = new_pos;
            }
        }
    }

    // CS-0236: an anonymous seal key is a fold of its operands, so two
    // identical inline seals name one determinant. Keep the first and drop the
    // rest — reaching the register phase with both would be a duplicate-key
    // error over two declarations that are the same declaration.
    let mut seen_anonymous = std::collections::HashSet::new();
    probes.retain(|p| !p.name.starts_with("@seal:") || seen_anonymous.insert(p.name.clone()));

    // CS-0240: a probe key and an import alias share the `$<…>` name space.
    // With probe keys in §10.2's cascade, `$<alias.recipe>` and probe `alias`
    // field `recipe` are the same token with two readings, so the collision is
    // rejected here. Checked after the loop rather than in the two declaration
    // arms because either can come first: imports must precede recipes, probes
    // need not, so neither map is complete while the other is being built.
    // Runs after CS-0236's dedup, though the order is unobservable: an
    // anonymous `@seal:` key carries a colon and can never equal a bare alias.
    for probe in &probes {
        if let Some(imp) = imports.iter().find(|i| i.name == probe.name) {
            // The kind comes from the map rather than being assumed `Probe`:
            // `cookfile.probes` is flat, and a `files`/`tools` declaration must
            // name its own keyword here too.
            let kind = callable_decls
                .get(&probe.name)
                .map(|&(k, _)| k)
                .unwrap_or(CallableKind::Probe);
            return Err(duplicate_callable_error(
                kind,
                &probe.name,
                probe.line,
                CallableKind::Import,
                imp.line,
            ));
        }
    }

    Ok(Cookfile {
        config_blocks,
        recipes,
        chores,
        uses,
        imports,
        register_blocks,
        top_level_module_calls,
        probes,
    })
}

#[cfg(test)]
#[path = "tests/lang_tests.rs"]
mod tests;
