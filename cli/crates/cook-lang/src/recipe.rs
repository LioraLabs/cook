use std::collections::BTreeSet;

use crate::ast::*;
use crate::brace_scan::LuaScanner;
use crate::cook_line::*;
use crate::disposition::{parse_seal_refs, parse_test_modifiers};
use crate::lexer::*;
use crate::lua_block::collect_lua_block;
use crate::ParseError;

/// Returns true if `text` looks like a module function call: `ident.ident...`
pub(crate) fn is_module_call(text: &str) -> bool {
    let bytes = text.as_bytes();
    // Must start with an ASCII letter or underscore
    if bytes.is_empty() || !(bytes[0].is_ascii_alphabetic() || bytes[0] == b'_') {
        return false;
    }
    // Find the dot
    let mut i = 1;
    while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
        i += 1;
    }
    if i >= bytes.len() || bytes[i] != b'.' {
        return false;
    }
    // After the dot, must have at least one ident char
    i += 1;
    i < bytes.len() && (bytes[i].is_ascii_alphabetic() || bytes[i] == b'_')
}

/// Collects a module call that may span multiple lines (when braces are unbalanced).
///
/// A module call's body is Lua source, so the brace counter uses the stateful
/// [`LuaScanner`] (CS-0035): braces inside multi-line long strings or block
/// comments are treated as data and do not prematurely close the call.
pub(crate) fn collect_module_call(
    first_line_text: &str,
    line: usize,
    tokens: &[Located<Token>],
    current_pos: usize,
    source_lines: &[&str],
) -> Result<(String, usize), ParseError> {
    let mut scanner = LuaScanner::new();
    let mut depth = scanner.scan_line(first_line_text);

    if depth <= 0 {
        // Single-line call (balanced or no braces)
        return Ok((first_line_text.to_string(), current_pos + 1));
    }

    // Multi-line: collect subsequent source lines until braces balance
    let mut code_lines = vec![first_line_text.to_string()];
    // line is 1-indexed; source_lines is 0-indexed
    let mut line_idx = line; // next source line (0-indexed)

    while line_idx < source_lines.len() {
        let raw_line = source_lines[line_idx];
        depth += scanner.scan_line(raw_line);
        code_lines.push(raw_line.to_string());

        if depth <= 0 {
            break;
        }
        line_idx += 1;
    }

    if depth > 0 {
        return Err(ParseError::Parse {
            line,
            message: "unclosed brace in module call".to_string(),
        });
    }

    let code = code_lines.join("\n");
    let close_line_1indexed = line_idx + 1;

    // Skip all tokens whose line is <= the closing line
    let mut new_pos = current_pos + 1;
    while new_pos < tokens.len() && tokens[new_pos].line <= close_line_1indexed {
        new_pos += 1;
    }

    Ok((code, new_pos))
}

/// Lua reserved words (Lua 5.4 §3.1) — used to rule out shapes like
/// `local x = 1` or `if true then ... end` when detecting a bare
/// `NAME "value"` config statement (CS-0126).
const LUA_KEYWORDS: &[&str] = &[
    "and", "break", "do", "else", "elseif", "end", "false", "for", "function",
    "goto", "if", "in", "local", "nil", "not", "or", "repeat", "return",
    "then", "true", "until", "while",
];

/// Detects the make-refugee `NAME "value"` / `NAME value` statement shape
/// on a config-block body line (CS-0126). This is the pre-CS-0011 VarDecl
/// shape: a bare identifier followed by whitespace and a value, which is
/// *not* valid Lua (it parses as two separate statements/an ambiguous
/// function call) and previously only failed at register time with a
/// confusing "attempt to call a nil value" error deep in the Lua VM.
///
/// Returns `(name, suggested_value)` where `suggested_value` is always a
/// valid Lua expression string (bare unquoted words get wrapped in quotes).
///
/// Deliberately also flags paren-less Lua call statements (`print "x"`)
/// inside config blocks, since they share the exact same lexical shape as
/// the VarDecl antipattern; write the parenthesized form (`print("x")`).
fn detect_bare_config_value(line: &str) -> Option<(String, String)> {
    let t = line.trim();
    if t.is_empty() || t.starts_with('#') || t.starts_with("--") {
        return None;
    }
    let first = t.chars().next().unwrap();
    if !(first.is_ascii_alphabetic() || first == '_') {
        return None;
    }
    let ident_end = t.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))?;
    let (ident, rest) = t.split_at(ident_end);
    if LUA_KEYWORDS.contains(&ident) {
        return None;
    }
    if !rest.starts_with(|c: char| c.is_whitespace()) {
        return None; // rules out env.X, f(x), t[k], x=1, obj:m
    }
    let mut rest = rest.trim_start();
    if let Some(i) = rest.find(" --") {
        rest = rest[..i].trim_end(); // drop trailing Lua comment from the suggestion
    }
    if rest.starts_with('"') || rest.starts_with('\'') {
        return Some((ident.to_string(), rest.to_string()));
    }
    if !rest.is_empty()
        && rest.chars().next().unwrap().is_ascii_alphanumeric()
        && !rest.contains('=')
        && !rest.contains('(')
    {
        return Some((ident.to_string(), format!("\"{rest}\"")));
    }
    None
}

pub(crate) fn parse_config_block_lua(
    tokens: &[Located<Token>],
    start: usize,
    open_line: usize,
    source_lines: &[&str],
) -> Result<(String, usize), ParseError> {
    // CS-0019: scan to the next column-0 top-level keyword or EOF.
    // The terminating token is left in place for parse() to dispatch.
    let mut pos = start;
    while pos < tokens.len() {
        let tok = &tokens[pos];
        match &tok.value {
            Token::RecipeHeader { .. }
            | Token::ChoreHeader { .. }
            | Token::ConfigHeader { .. }
            | Token::UseDecl { .. }
            | Token::ImportDecl { .. }
            | Token::RegisterHeader
            | Token::ProbeHeader { .. } => break,
            // Top-level module_call (column-0 Content matching the module-call
            // shape) is also a terminator as of CS-0072. Check the raw source
            // line to distinguish column-0 from indented Content.
            Token::Content(text) if is_module_call(text) => {
                let raw = source_lines
                    .get(tok.line.saturating_sub(1))
                    .copied()
                    .unwrap_or("");
                if !raw.starts_with(|c: char| c.is_whitespace()) {
                    break;
                }
                pos += 1;
            }
            _ => pos += 1,
        }
    }

    let end_line = if pos < tokens.len() {
        tokens[pos].line
    } else {
        source_lines.len() + 1
    };

    let start_idx = open_line; // 1-indexed line of header; body starts at the next line
    let end_idx = end_line.saturating_sub(1);
    let body = if start_idx < end_idx && end_idx <= source_lines.len() {
        // Trim trailing blank lines so that a blank separator between the
        // config block body and the next keyword does not become part of
        // the body (consistent with v0.3 explicit-`end` behaviour).
        let lines = &source_lines[start_idx..end_idx];
        let trimmed_end = lines.iter().rposition(|l| !l.trim().is_empty())
            .map(|i| i + 1)
            .unwrap_or(0);

        // CS-0126: parse-time did-you-mean for the bare `NAME "value"` /
        // `NAME value` statement shape (the pre-CS-0011 VarDecl antipattern
        // every make/just refugee types). Checked here, before the body is
        // handed off as opaque Lua source, so the diagnostic is source-mapped
        // and never reaches the Lua VM.
        for (i, raw) in lines.iter().enumerate() {
            if let Some((name, value)) = detect_bare_config_value(raw) {
                let abs_line = start_idx + i + 1;
                return Err(ParseError::Parse {
                    line: abs_line,
                    message: format!(
                        "config values are Lua assignments — did you mean {} = {}?",
                        name, value
                    ),
                });
            }
        }

        lines[..trimmed_end].join("\n")
    } else {
        String::new()
    };

    Ok((body, pos))
}

pub(crate) fn parse_register_block_lua(
    tokens: &[Located<Token>],
    start: usize,
    open_line: usize,
    source_lines: &[&str],
) -> Result<(String, usize), ParseError> {
    // Mirror parse_config_block_lua: scan to the next column-0 top-level
    // keyword OR top-level module_call shape OR EOF. The terminating
    // token is left in place for parse() to dispatch.
    let mut pos = start;
    while pos < tokens.len() {
        let tok = &tokens[pos];
        match &tok.value {
            Token::RecipeHeader { .. }
            | Token::ChoreHeader { .. }
            | Token::ConfigHeader { .. }
            | Token::UseDecl { .. }
            | Token::ImportDecl { .. }
            | Token::RegisterHeader
            | Token::ProbeHeader { .. } => break,
            // Top-level module_call (Content matching <id>.<id>(...) shape)
            // is also a terminator (CS-0072 §4.1.1 clause b).
            // Only column-0 Content can be top-level: check the raw source line.
            Token::Content(text) if is_module_call(text) => {
                let raw = source_lines
                    .get(tok.line.saturating_sub(1))
                    .copied()
                    .unwrap_or("");
                if !raw.starts_with(|c: char| c.is_whitespace()) {
                    break;
                }
                pos += 1;
            }
            _ => pos += 1,
        }
    }

    let end_line = if pos < tokens.len() {
        tokens[pos].line
    } else {
        source_lines.len() + 1
    };

    let start_idx = open_line;
    let end_idx = end_line.saturating_sub(1);
    let body = if start_idx < end_idx && end_idx <= source_lines.len() {
        let lines = &source_lines[start_idx..end_idx];
        let trimmed_end = lines
            .iter()
            .rposition(|l| !l.trim().is_empty())
            .map(|i| i + 1)
            .unwrap_or(0);
        lines[..trimmed_end].join("\n")
    } else {
        String::new()
    };

    Ok((body, pos))
}

/// COOK-171 / CS-0159: fold the recipe-level `seal` baseline into each
/// *cacheable unit's* effective seal set, then apply that unit's per-unit
/// trailing `unseal`. `effective(unit) = (base ∪ step_seals) − step_unseals`.
///
/// Both `cook` and `test` steps are cacheable units, so the baseline applies
/// to both (§8.4.3 rule 1, CS-0159). Scope is declarative and
/// order-independent — a recipe-level `seal` applies to every unit in the
/// recipe regardless of textual position — so the fold runs once at recipe
/// finalize, after the whole body has been parsed. `unseals` carries
/// `(index-into-steps, that-unit's-unseal-set)` pairs, keyed by step index so
/// one map serves both step kinds.
fn apply_base_seal(
    steps: &mut [Step],
    base: &BTreeSet<String>,
    unseals: &[(usize, BTreeSet<String>)],
) {
    use std::collections::HashMap;
    let unseal_by: HashMap<usize, &BTreeSet<String>> =
        unseals.iter().map(|(i, s)| (*i, s)).collect();
    for (i, step) in steps.iter_mut().enumerate() {
        // The effective-set slot differs per step kind; the fold does not.
        let seal: &mut BTreeSet<String> = match step {
            Step::Cook { step, .. } => &mut step.disposition.seal,
            Step::Test { step, .. } => &mut step.seal,
            _ => continue,
        };
        for r in base {
            seal.insert(r.clone());
        }
        if let Some(u) = unseal_by.get(&i) {
            for r in u.iter() {
                seal.remove(r);
            }
        }
    }
}

fn finalize_base_seal(
    name: &str,
    recipe_line: usize,
    steps: &mut [Step],
    base: &BTreeSet<String>,
    unseals: &[(usize, BTreeSet<String>)],
) -> Result<(), ParseError> {
    // CS-0159: a `test`-only recipe is a legitimate seal target — a test unit
    // keys on its sealed probes exactly as a cook unit does (§17.4 rule 1), so
    // the baseline has somewhere to land. Only a recipe with no cacheable unit
    // at all leaves the seal dangling.
    let has_sealable = steps
        .iter()
        .any(|step| matches!(step, Step::Cook { .. } | Step::Test { .. }));
    if !base.is_empty() && !has_sealable {
        return Err(ParseError::Parse {
            line: recipe_line,
            message: format!("seal on recipe {name}: no cook or test units to apply to"),
        });
    }
    apply_base_seal(steps, base, unseals);
    Ok(())
}

pub(crate) fn parse_recipe(
    name: String,
    deps: Vec<String>,
    recipe_line: usize,
    tokens: &[Located<Token>],
    start: usize,
    source_lines: &[&str],
) -> Result<(Recipe, usize), ParseError> {
    let mut pos = start;
    let mut ingredients = Vec::new();
    let mut excludes: Vec<String> = Vec::new();
    let mut steps: Vec<Step> = Vec::new();
    // §{steps.ingredients}: glob-pattern `ingredients` and `ingredients <probe>`
    // (probe member source) are mutually exclusive within a recipe, and at most
    // one probe source is allowed per recipe.
    let mut for_each_seen = false;

    // COOK-171: recipe-level `seal` baseline (the determinant set applied to
    // every cook in the recipe) and each cook's per-unit trailing `unseal`
    // set. Both are folded into the cooks' effective seal sets at finalize
    // (`apply_base_seal`), so recipe-level seals are order-independent.
    let mut base_seal: BTreeSet<String> = BTreeSet::new();
    let mut cook_unseals: Vec<(usize, BTreeSet<String>)> = Vec::new();

    while pos < tokens.len() {
        let tok = &tokens[pos];
        match &tok.value {
            // CS-0019: implicit termination — the next column-0 top-level
            // keyword closes the body. The token is left in place so that
            // parse() dispatches it as the next toplevel_item.
            Token::RecipeHeader { .. }
            | Token::ChoreHeader { .. }
            | Token::ConfigHeader { .. }
            | Token::UseDecl { .. }
            | Token::ImportDecl { .. }
            | Token::RegisterHeader
            | Token::ProbeHeader { .. } => {
                finalize_base_seal(
                    &name,
                    recipe_line,
                    &mut steps,
                    &base_seal,
                    &cook_unseals,
                )?;
                return Ok((
                    Recipe {
                        name,
                        deps,
                        ingredients,
                        excludes,
                        steps,
                        line: recipe_line,
                    },
                    pos,
                ));
            }
            Token::Comment(_) | Token::Blank => {
                pos += 1;
            }
            Token::Content(text) => {
                // CS-0072: a column-0 Content token matching the module-call
                // shape is a top-level module_call, not a recipe step.
                // Terminate the recipe body and leave the token for parse()
                // to dispatch as a top-level module_call.
                if is_module_call(text) {
                    let raw = source_lines
                        .get(tok.line.saturating_sub(1))
                        .copied()
                        .unwrap_or("");
                    if !raw.starts_with(|c: char| c.is_whitespace()) {
                        // column-0 module call terminates the recipe body.
                        finalize_base_seal(
                            &name,
                            recipe_line,
                            &mut steps,
                            &base_seal,
                            &cook_unseals,
                        )?;
                        return Ok((
                            Recipe {
                                name,
                                deps,
                                ingredients,
                                excludes,
                                steps,
                                line: recipe_line,
                            },
                            pos,
                        ));
                    }
                    // CS-0134: an indented bare module call is register-phase Lua.
                    let (code, new_pos) =
                        collect_module_call(text, tok.line, tokens, pos, source_lines)?;
                    steps.push(Step::InlineLua { code, line: tok.line });
                    pos = new_pos;
                    continue;
                }
                // COOK-171: `seal` is a recipe-body step (a determinant input
                // stream, sibling of `ingredients`). It contributes to the
                // recipe-level baseline applied to every cook at finalize.
                if let Some(rest) = strip_keyword(text, "seal") {
                    let refs: Vec<String> =
                        rest.split_whitespace().map(str::to_string).collect();
                    if refs.is_empty() {
                        return Err(ParseError::Parse {
                            line: tok.line,
                            message: "seal: a recipe-level `seal` step requires at least one probe ref"
                                .to_string(),
                        });
                    }
                    for r in parse_seal_refs(&refs, tok.line)? {
                        base_seal.insert(r);
                    }
                    pos += 1;
                    continue;
                }
                // COOK-171: recipe-level `unseal` is rejected — `unseal` is a
                // trailing modifier on a `cook` or `test` step only (CS-0159).
                // The recipe is the outermost seal scope, so there is nothing
                // inherited to release.
                if strip_keyword(text, "unseal").is_some() {
                    return Err(ParseError::Parse {
                        line: tok.line,
                        message: "unseal is a trailing modifier on a `cook` or `test` step, not \
                                  a recipe-level step (the recipe is the outermost seal scope; \
                                  there is nothing to release)"
                            .to_string(),
                    });
                }
                if let Some(rest) = strip_keyword(text, "ingredients") {
                    if for_each_seen {
                        return Err(ParseError::Parse {
                            line: tok.line,
                            message:
                                "a recipe may declare at most one `ingredients <probe>` source"
                                    .to_string(),
                        });
                    }
                    let head = rest.trim_start();
                    if head.starts_with('"') || head.starts_with('!') {
                        // Glob ingredients (existing path).
                        if !ingredients.is_empty() || !excludes.is_empty() {
                            return Err(ParseError::Parse {
                                line: tok.line,
                                message: "duplicate 'ingredients' line".to_string(),
                            });
                        }
                        let (inc, exc, new_pos) =
                            parse_ingredients_line(rest, tok.line, tokens, pos, source_lines)?;
                        ingredients = inc;
                        excludes = exc;
                        pos = new_pos;
                        continue;
                    } else {
                        // COOK-88: bare identifier => probe member source. Desugar to ForEach.
                        if !ingredients.is_empty() || !excludes.is_empty() {
                            return Err(ParseError::Parse {
                                line: tok.line,
                                message: "ingredients: cannot mix glob patterns with a probe source"
                                    .to_string(),
                            });
                        }
                        let (fe, new_pos) =
                            crate::cook_line::parse_ingredients_probe_source(rest, tok.line, tokens, pos)?;
                        for_each_seen = true;
                        steps.push(Step::ForEach { step: fe, line: tok.line });
                        pos = new_pos;
                        continue;
                    }
                } else if let Some(rest) = strip_keyword(text, "cook") {
                    // COOK-171: parse_cook_line resolves the trailing `cook_mods`
                    // (per-unit seal/unseal + share_mod) onto the step's
                    // disposition. The `as` modifier rejection (CS-0061) is folded
                    // into the modifier parser. The recipe-level seal baseline is
                    // applied later at finalize (`apply_base_seal`).
                    let (cook_step, unseal, new_pos) =
                        parse_cook_line(rest, tok.line, tokens, pos, source_lines)?;
                    let idx = steps.len();
                    steps.push(Step::Cook {
                        step: cook_step,
                        line: tok.line,
                    });
                    if !unseal.is_empty() {
                        cook_unseals.push((idx, unseal));
                    }
                    pos = new_pos;
                    continue;
                } else if let Some(rest) = strip_keyword(text, "test") {
                    // CS-0159: a `test` step takes the input half of the
                    // trailing modifier tail (`seal`/`unseal`); the recipe-level
                    // baseline is folded in later at finalize
                    // (`apply_base_seal`), so per-unit seals here are additive
                    // and the unseals are recorded against this step's index.
                    let (body, trailing, new_pos) = crate::cook_line::parse_body_payload(
                        rest, tok.line, tokens, pos, source_lines, "test",
                    )?;
                    let mods = parse_test_modifiers(&trailing, tok.line)?;
                    let idx = steps.len();
                    steps.push(Step::Test {
                        step: TestStep { body, seal: mods.seal },
                        line: tok.line,
                    });
                    if !mods.unseal.is_empty() {
                        cook_unseals.push((idx, mods.unseal));
                    }
                    pos = new_pos;
                    continue;
                } else if text.starts_with('@') {
                    return Err(ParseError::Parse {
                        line: tok.line,
                        message:
                            "the `@` interactive prefix was removed from the language (CS-0134); \
                             recipes are declarative and chore commands are interactive by default — \
                             drop the `@`"
                                .to_string(),
                    });
                } else {
                    // SHI-216 / CS-0072 §3.9: reject `register` + separator inside a recipe body.
                    // An indented `register <args>` cannot be a RegisterHeader (column-0 only
                    // per §2.10) and is also not a permitted shell command in this position.
                    // The bare `register` identifier (trimmed == "register", no separator)
                    // remains a shell_command per the post-CS-0072 rule 6.
                    {
                        let trimmed = text.trim();
                        if trimmed.starts_with("register")
                            && trimmed.len() > 8
                            && {
                                let b = trimmed.as_bytes()[8];
                                b == b' ' || b == b'\t'
                            }
                        {
                            return Err(ParseError::Parse {
                                line: tok.line,
                                message:
                                    "`register` blocks are top-level only; move this outside the recipe body"
                                        .to_string(),
                            });
                        }
                    }
                    return Err(ParseError::Parse {
                        line: tok.line,
                        message: format!(
                            "loose shell commands are not allowed in a recipe body (CS-0134): `{}`; \
                             move it into a `cook \"out\" {{ … }}` body or a chore",
                            text.trim()
                        ),
                    });
                }
            }
            Token::LuaLine(_) => {
                return Err(ParseError::Parse {
                    line: tok.line,
                    message:
                        "execute-phase `>` Lua is not allowed in a recipe body (CS-0134); \
                         use `cook \"out\" >{ … }`, `test >{ … }`, or a chore"
                            .to_string(),
                });
            }
            Token::LuaBlockOpen => {
                return Err(ParseError::Parse {
                    line: tok.line,
                    message:
                        "execute-phase `>{ … }` Lua block is not allowed in a recipe body (CS-0134); \
                         use `cook \"out\" >{ … }`, `test >{ … }`, or a chore"
                            .to_string(),
                });
            }
            Token::InlineLuaLine(_) => {
                return Err(ParseError::Parse {
                    line: tok.line,
                    message:
                        "the register-phase `>>` sigil was removed (CS-0134); write a bare \
                         `module.call()` in the recipe body, or move register work to a \
                         top-level `register` block"
                            .to_string(),
                });
            }
            Token::InlineLuaBlockOpen => {
                return Err(ParseError::Parse {
                    line: tok.line,
                    message:
                        "the register-phase `>>{ … }` sigil was removed (CS-0134); move it into a \
                         top-level `register` block"
                            .to_string(),
                });
            }
        }
    }

    // COOK-171: fold the recipe-level seal baseline into each cook at finalize.
    finalize_base_seal(
        &name,
        recipe_line,
        &mut steps,
        &base_seal,
        &cook_unseals,
    )?;

    // CS-0019: EOF terminates a body. No "missing end" error in v0.4.
    Ok((
        Recipe {
            name,
            deps,
            ingredients,
            excludes,
            steps,
            line: recipe_line,
        },
        pos,
    ))
}

pub(crate) fn parse_chore(
    name: String,
    params: Vec<ChoreParam>,
    deps: Vec<String>,
    chore_line: usize,
    tokens: &[Located<Token>],
    start: usize,
    source_lines: &[&str],
) -> Result<(Chore, usize), ParseError> {
    let mut pos = start;
    let mut steps: Vec<Step> = Vec::new();

    let chore_banned = |keyword: &str, line: usize| -> ParseError {
        let kind_descriptor = match keyword {
            "ingredients" => "inputs",
            "cook"        => "outputs",
            "test"        => "tested outputs",
            _             => "targets",
        };
        ParseError::Parse {
            line,
            message: format!(
                "'{}' is not allowed in a chore; use 'recipe' for build {}",
                keyword, kind_descriptor
            ),
        }
    };

    while pos < tokens.len() {
        let tok = &tokens[pos];
        match &tok.value {
            // CS-0019/0020 implicit termination: any column-0 top-level
            // keyword closes the chore body. Token left in place for parse().
            Token::RecipeHeader { .. }
            | Token::ChoreHeader { .. }
            | Token::ConfigHeader { .. }
            | Token::UseDecl { .. }
            | Token::ImportDecl { .. }
            | Token::RegisterHeader
            | Token::ProbeHeader { .. } => {
                return Ok((
                    Chore { name, params, deps, steps, line: chore_line },
                    pos,
                ));
            }
            Token::Comment(_) | Token::Blank => {
                pos += 1;
            }
            Token::Content(text) => {
                // CS-0072: column-0 Content matching the module-call shape
                // terminates the chore body; token left for parse() dispatch.
                if is_module_call(text) {
                    let raw = source_lines
                        .get(tok.line.saturating_sub(1))
                        .copied()
                        .unwrap_or("");
                    if !raw.starts_with(|c: char| c.is_whitespace()) {
                        return Ok((
                            Chore { name, params, deps, steps, line: chore_line },
                            pos,
                        ));
                    }
                }
                let text = text.clone();
                if strip_keyword(&text, "ingredients").is_some() {
                    return Err(chore_banned("ingredients", tok.line));
                } else if strip_keyword(&text, "cook").is_some() {
                    return Err(chore_banned("cook", tok.line));
                } else if strip_keyword(&text, "test").is_some() {
                    return Err(chore_banned("test", tok.line));
                } else if text.starts_with('@') {
                    return Err(ParseError::Parse {
                        line: tok.line,
                        message:
                            "the `@` interactive prefix was removed from the language (CS-0134); \
                             chore commands are interactive by default — drop the `@`"
                                .to_string(),
                    });
                } else {
                    // SHI-216 / CS-0072 §3.9: reject `register` + separator inside a chore body.
                    // The bare `register` identifier (no separator) remains a shell_command per
                    // the post-CS-0072 rule 6.
                    {
                        let trimmed = text.trim();
                        if trimmed.starts_with("register")
                            && trimmed.len() > 8
                            && {
                                let b = trimmed.as_bytes()[8];
                                b == b' ' || b == b'\t'
                            }
                        {
                            return Err(ParseError::Parse {
                                line: tok.line,
                                message:
                                    "`register` blocks are top-level only; move this outside the chore body"
                                        .to_string(),
                            });
                        }
                    }
                    // Default-interactive — no `@` required
                    steps.push(Step::Shell {
                        command: text.clone(),
                        line: tok.line,
                        interactive: true,
                    });
                }
                pos += 1;
            }
            Token::LuaLine(code) => {
                steps.push(Step::Lua { code: code.clone(), line: tok.line });
                pos += 1;
            }
            Token::LuaBlockOpen => {
                let block_line = tok.line;
                // The `>{` opener lexes as a bare token whose remaining line
                // content the lexer dropped — recover it from the source so
                // the remainder is the block's first body segment (CS-0154).
                let after_open = source_lines
                    .get(block_line.saturating_sub(1))
                    .copied()
                    .unwrap_or("")
                    .trim_start()
                    .strip_prefix(">{")
                    .unwrap_or("");
                pos += 1;
                let (code, block_tail, new_pos) =
                    collect_lua_block(block_line, after_open, tokens, pos, source_lines)?;
                crate::shell_block::reject_stray_tail(&block_tail, block_line, "chore")?;
                steps.push(Step::LuaBlock { code, line: block_line });
                pos = new_pos;
            }
            Token::InlineLuaLine(_) => {
                return Err(ParseError::Parse {
                    line: tok.line,
                    message:
                        "the register-phase `>>` sigil was removed (CS-0134); write a bare \
                         `module.call()` in the recipe body, or move register work to a \
                         top-level `register` block"
                            .to_string(),
                });
            }
            Token::InlineLuaBlockOpen => {
                return Err(ParseError::Parse {
                    line: tok.line,
                    message:
                        "the register-phase `>>{ … }` sigil was removed (CS-0134); move it into a \
                         top-level `register` block"
                            .to_string(),
                });
            }
        }
    }

    // EOF terminates
    Ok((Chore { name, params, deps, steps, line: chore_line }, pos))
}
