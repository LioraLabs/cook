use std::collections::BTreeSet;

use crate::ast::*;
use crate::brace_scan::LuaScanner;
use crate::cook_line::*;
use crate::disposition::{parse_seal_operands, removed_trailing_seal, removed_unseal};
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
    // Lua reserved words (Lua 5.4 §3.1) rule out shapes like `local x = 1`
    // or `if true then ... end`. The list lives in `cook-contracts` because
    // `cook-cookfile` asks the same question when it writes a table key
    // (CS-0221), and two copies of it would agree only by coincidence.
    if cook_contracts::lua_scan::is_reserved_word(ident) {
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
            | Token::ProbeHeader { .. } | Token::FilesHeader { .. } | Token::ToolsHeader { .. } => break,
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
            | Token::ProbeHeader { .. } | Token::FilesHeader { .. } | Token::ToolsHeader { .. } => break,
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

/// Fold the recipe seal set into each cacheable unit.
///
/// Both `cook` and `test` steps are cacheable units, so the baseline applies
/// to both (§8.4.3 rule 1, CS-0159). Scope is declarative and
/// order-independent — a recipe-level `seal` applies to every unit in the
/// recipe regardless of textual position — so the fold runs once at recipe
/// finalize, after the whole body has been parsed.
fn apply_base_seal(steps: &mut [Step], base: &BTreeSet<String>) {
    for step in steps {
        // The seal slot differs per step kind; the fold does not.
        let seal: &mut BTreeSet<String> = match step {
            Step::Cook { step, .. } => &mut step.disposition.seal,
            Step::Test { step, .. } => &mut step.seal,
            _ => continue,
        };
        for r in base {
            seal.insert(r.clone());
        }
    }
}

fn finalize_base_seal(
    name: &str,
    recipe_line: usize,
    steps: &mut [Step],
    base: &BTreeSet<String>,
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
    apply_base_seal(steps, base);
    Ok(())
}

fn reject_test_tail(tail: &str, line: usize) -> Result<(), ParseError> {
    let Some(word) = tail.split_whitespace().next() else {
        return Ok(());
    };
    Err(match word {
        "seal" => removed_trailing_seal("test", line),
        "unseal" => removed_unseal(line),
        "should_fail" => ParseError::Parse { line, message: "test: should_fail was removed in v1.0 — invert the check in the body instead".to_string() },
        "timeout" => ParseError::Parse { line, message: "test: timeout was removed in v1.0 — enforce a deadline from inside the test body instead".to_string() },
        "as" => ParseError::Parse { line, message: "test: as was removed in v1.0 — test steps no longer take a custom name".to_string() },
        other => ParseError::Parse { line, message: format!("unexpected text after test body: '{other}'") },
    })
}

pub(crate) fn parse_recipe(
    name: String,
    deps: Vec<String>,
    recipe_line: usize,
    tokens: &[Located<Token>],
    start: usize,
    source_lines: &[&str],
) -> Result<(Recipe, Vec<Probe>, usize), ParseError> {
    let mut pos = start;
    let mut inputs = Vec::new();
    let mut excludes: Vec<String> = Vec::new();
    let mut steps: Vec<Step> = Vec::new();
    // §{steps.gather}: glob-pattern and bare-source `gather` are mutually
    // exclusive within a recipe, and at most one bare source is allowed.
    let mut member_source_seen = false;

    // The recipe seal set is folded into every cacheable unit at finalize, so
    // recipe-level seals are order-independent.
    let mut base_seal: BTreeSet<String> = BTreeSet::new();
    let mut inline_probes = Vec::new();

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
            | Token::ProbeHeader { .. }
            | Token::FilesHeader { .. }
            | Token::ToolsHeader { .. } => {
                finalize_base_seal(&name, recipe_line, &mut steps, &base_seal)?;
                return Ok((
                    Recipe {
                        name,
                        deps,
                        inputs,
                        excludes,
                        steps,
                        line: recipe_line,
                    },
                    inline_probes,
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
                        finalize_base_seal(&name, recipe_line, &mut steps, &base_seal)?;
                        return Ok((
                            Recipe {
                                name,
                                deps,
                                inputs,
                                excludes,
                                steps,
                                line: recipe_line,
                            },
                            inline_probes,
                            pos,
                        ));
                    }
                    // CS-0134: an indented bare module call is register-phase Lua.
                    let (code, new_pos) =
                        collect_module_call(text, tok.line, tokens, pos, source_lines)?;
                    steps.push(Step::InlineLua {
                        code,
                        line: tok.line,
                    });
                    pos = new_pos;
                    continue;
                }
                // COOK-171: `seal` is a recipe-body step (a determinant input
                // stream, sibling of `gather`). It contributes to the
                // recipe-level baseline applied to every cook at finalize.
                if let Some(rest) = strip_keyword(text, "seal") {
                    if rest.trim().is_empty() {
                        return Err(ParseError::Parse {
                            line: tok.line,
                            message: "seal: a recipe-level `seal` step requires at least one probe ref"
                                .to_string(),
                        });
                    }
                    let parsed = parse_seal_operands(rest, tok.line, &name)?;
                    for r in parsed.refs {
                        base_seal.insert(r);
                    }
                    inline_probes.extend(parsed.inline_probe);
                    pos += 1;
                    continue;
                }
                // CS-0225 removed every `unseal` position.
                if strip_keyword(text, "unseal").is_some() {
                    return Err(removed_unseal(tok.line));
                }
                // CS-0226 removed `envs`; CS-0235 gives the recipe-body position
                // the same named removal diagnostic the probe-producer position
                // already had. Without this branch the line fell through to the
                // rule-7 loose-shell rejection, whose advice — move it into a
                // `cook` body or a chore — is wrong for a keyword no position
                // accepts. Operands are the names, however the author separated
                // them, so the replacement names them back.
                if let Some(rest) = strip_keyword(text, "envs") {
                    let names: Vec<String> = rest
                        .split(|c: char| c == ',' || c.is_whitespace() || c == '{' || c == '}')
                        .filter(|t| !t.is_empty())
                        .map(str::to_string)
                        .collect();
                    return Err(crate::disposition::removed_envs("envs", &names, tok.line));
                }
                if strip_keyword(text, "ingredients").is_some() {
                    return Err(ParseError::Parse {
                        line: tok.line,
                        message: "`ingredients` was removed (CS-0229); use `gather` for iteration, or declare `files` and `seal` for determinants".to_string(),
                    });
                }
                let gather = strip_keyword(text, "gather");
                if let Some(rest) = gather {
                    if member_source_seen {
                        return Err(ParseError::Parse {
                            line: tok.line,
                            message: "a recipe may declare at most one bare `gather` source"
                                .to_string(),
                        });
                    }
                    let head = rest.trim_start();
                    if head.starts_with('"') || head.starts_with('!') {
                        // Glob inputs (existing path).
                        if !inputs.is_empty() || !excludes.is_empty() {
                            return Err(ParseError::Parse {
                                line: tok.line,
                                message: "duplicate 'gather' line".to_string(),
                            });
                        }
                        let (inc, exc, new_pos) =
                            parse_gather_line(rest, "gather", tok.line, tokens, pos, source_lines)?;
                        inputs = inc;
                        excludes = exc;
                        steps.push(Step::Gather { line: tok.line });
                        pos = new_pos;
                        continue;
                    } else {
                        // COOK-88: bare identifier => named member source. Desugar to MemberSource.
                        if !inputs.is_empty() || !excludes.is_empty() {
                            return Err(ParseError::Parse {
                                line: tok.line,
                                message: "gather: cannot mix glob patterns with a bare source"
                                    .to_string(),
                            });
                        }
                        let (fe, new_pos) = crate::cook_line::parse_gather_bare_source(
                            rest, tok.line, tokens, pos, true,
                        )?;
                        member_source_seen = true;
                        steps.push(Step::MemberSource {
                            step: fe,
                            line: tok.line,
                        });
                        pos = new_pos;
                        continue;
                    }
                } else if let Some(rest) = strip_keyword(text, "cook") {
                    // `cook_mods` is the optional share disposition. The recipe
                    // seal set is applied later at finalize.
                    let (cook_step, new_pos) =
                        parse_cook_line(rest, tok.line, tokens, pos, source_lines)?;
                    steps.push(Step::Cook {
                        step: cook_step,
                        line: tok.line,
                    });
                    pos = new_pos;
                    continue;
                } else if let Some(rest) = strip_keyword(text, "test") {
                    // Tests admit no tail; the recipe seal set is folded in at finalize.
                    let (body, trailing, new_pos) = crate::cook_line::parse_body_payload(
                        rest, tok.line, tokens, pos, source_lines, "test",
                    )?;
                    reject_test_tail(&trailing, tok.line)?;
                    steps.push(Step::Test {
                        step: TestStep {
                            body,
                            seal: BTreeSet::new(),
                        },
                        line: tok.line,
                    });
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
    finalize_base_seal(&name, recipe_line, &mut steps, &base_seal)?;

    // CS-0019: EOF terminates a body. No "missing end" error in v0.4.
    Ok((
        Recipe {
            name,
            deps,
            inputs,
            excludes,
            steps,
            line: recipe_line,
        },
        inline_probes,
        pos,
    ))
}

/// Classify a chore-body `Content` line against the three banned step kinds
/// (§{chores.body}, App. A.3.1 "Chore step-kind ban"; CS-0233).
///
/// The ban is on the step KIND, not on the keyword: a chore body is the
/// shell-first form, and `test`, `cook` and `gather` are all ordinary words a
/// shell command may open with. `test -f x` and `cook build` are shell; only a
/// line that actually carries the banned step's own syntax is that step.
/// Returns the offending keyword when the line is one of the banned kinds.
///
/// The discriminators are the ones `tree-sitter-cook`'s external scanner has
/// always used (`scan_shell_content`, the `quoted_step` / `test_body` /
/// `cook_lua_output` / `bare_gather` block): a quoted first operand, `test`
/// opening a body (`{` or `>{`), `cook` opening its Lua-expression output form
/// (`(`), or `gather` naming a bare member source and nothing else. Before
/// CS-0233 the reference parser tested the keyword prefix alone and the two
/// implementations disagreed; the scanner's reading is the one that holds.
pub(crate) fn chore_banned_step_kind(text: &str) -> Option<&'static str> {
    for keyword in ["gather", "cook", "test"] {
        let Some(rest) = strip_keyword(text, keyword) else {
            continue;
        };
        // A bare keyword with no operand is a shell command (`test` alone is
        // a real, if useless, invocation); it carries no step syntax.
        // A quoted first operand is a `cook`/`gather` shape only. A `test`
        // step's operand is its body, and the bare-string form `test "cmd"`
        // is not a test step in any position (§{steps.test}) — so
        // `test "$X" = y` is shell, not a malformed step.
        let quoted_step =
            keyword != "test" && (rest.starts_with('"') || rest.starts_with('\''));
        // `gather !"build/*"` is an exclude glob (App. A.4, `input ::= STRING |
        // "!" STRING`), so it is gather-shaped too. Scoped to `gather` because
        // a `cook` step takes no excludes: `parse_cook_line` requires a leading
        // `"` and passes `allow_exclude = false`. Without this the line lowers
        // to a shell step that dies at run time with `gather: command not
        // found` — a strictly worse answer than the parse error it replaced,
        // and the only error-to-wrong-meaning case this classification has.
        let exclude_glob = keyword == "gather" && rest.starts_with('!');
        let test_body = keyword == "test" && (rest.starts_with('{') || rest.starts_with('>'));
        let cook_lua_output = keyword == "cook" && rest.starts_with('(');
        let bare_gather = keyword == "gather"
            && rest.starts_with(|c: char| c.is_alphabetic() || c == '_')
            && rest
                .chars()
                .all(|c| c.is_alphanumeric() || matches!(c, '_' | '.' | '-' | ':'));
        if quoted_step || exclude_glob || test_body || cook_lua_output || bare_gather {
            return Some(keyword);
        }
        return None;
    }
    None
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
            "inputs" => "inputs",
            "cook" => "outputs",
            "test" => "tested outputs",
            _ => "targets",
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
            | Token::ProbeHeader { .. } | Token::FilesHeader { .. } | Token::ToolsHeader { .. } => {
                return Ok((
                    Chore {
                        name,
                        params,
                        deps,
                        steps,
                        line: chore_line,
                    },
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
                            Chore {
                                name,
                                params,
                                deps,
                                steps,
                                line: chore_line,
                            },
                            pos,
                        ));
                    }
                }
                let text = text.clone();
                if strip_keyword(&text, "ingredients").is_some() {
                    return Err(ParseError::Parse { line: tok.line,
                        message: "`ingredients` was removed (CS-0229); use `gather` for iteration, or declare `files` and `seal` for determinants".into() });
                } else if let Some(keyword) = chore_banned_step_kind(&text) {
                    return Err(chore_banned(keyword, tok.line));
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
                steps.push(Step::Lua {
                    code: code.clone(),
                    line: tok.line,
                });
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
                steps.push(Step::LuaBlock {
                    code,
                    line: block_line,
                });
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
    Ok((
        Chore {
            name,
            params,
            deps,
            steps,
            line: chore_line,
        },
        pos,
    ))
}
