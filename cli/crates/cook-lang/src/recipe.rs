use crate::ast::*;
use crate::cook_line::*;
use crate::lexer::*;
use crate::lua_block::{collect_lua_block, count_brace_delta};
use crate::ParseError;

/// Returns true if `text` looks like a module function call: `ident.ident...`
fn is_module_call(text: &str) -> bool {
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
fn collect_module_call(
    first_line_text: &str,
    line: usize,
    tokens: &[Located<Token>],
    current_pos: usize,
    source_lines: &[&str],
) -> Result<(String, usize), ParseError> {
    let mut depth = count_brace_delta(first_line_text);

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
        depth += count_brace_delta(raw_line);
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

pub(crate) fn parse_config_block_lua(
    tokens: &[Located<Token>],
    start: usize,
    open_line: usize,
    source_lines: &[&str],
) -> Result<(String, usize), ParseError> {
    // Find the closing `end` token; collect source lines between the header and `end`.
    let mut pos = start;
    while pos < tokens.len() {
        if matches!(&tokens[pos].value, Token::RecipeEnd) {
            let end_line = tokens[pos].line; // 1-indexed
            // Source lines between open_line (exclusive) and end_line (exclusive).
            // open_line is 1-indexed; source_lines is 0-indexed.
            let start_idx = open_line; // line after the header
            let end_idx = end_line.saturating_sub(1); // line of `end`, exclusive

            let body = if start_idx < end_idx && end_idx <= source_lines.len() {
                source_lines[start_idx..end_idx].join("\n")
            } else {
                String::new()
            };

            return Ok((body, pos + 1));
        }
        pos += 1;
    }

    Err(ParseError::Parse {
        line: open_line,
        message: "config block was not closed with 'end'".to_string(),
    })
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

    // Track the line on which the imperative region began (the first
    // imperative-region step). None until we see one. App. A.3 "Region
    // ordering rule" / §{recipes.step-kinds} Note 4.4.2: once set, no
    // declarative-region step may follow.
    let mut imperative_began: Option<usize> = None;

    let region_violation = |kind: &str, step_line: usize, started_line: usize| ParseError::Parse {
        line: step_line,
        message: format!(
            "{} step on line {} is not allowed after the imperative region began on line {}",
            kind, step_line, started_line
        ),
    };

    while pos < tokens.len() {
        let tok = &tokens[pos];
        match &tok.value {
            Token::RecipeEnd => {
                pos += 1;
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
                if let Some(rest) = strip_keyword(text, "ingredients") {
                    if let Some(started) = imperative_began {
                        return Err(region_violation("ingredients", tok.line, started));
                    }
                    if !ingredients.is_empty() || !excludes.is_empty() {
                        return Err(ParseError::Parse {
                            line: tok.line,
                            message: "duplicate 'ingredients' line".to_string(),
                        });
                    }
                    let (inc, exc) = parse_ingredients_line(rest, tok.line)?;
                    ingredients = inc;
                    excludes = exc;
                } else if let Some(rest) = strip_keyword(text, "cook") {
                    if let Some(started) = imperative_began {
                        return Err(region_violation("cook", tok.line, started));
                    }
                    let (cook_step, new_pos) =
                        parse_cook_line(rest, tok.line, tokens, pos, source_lines)?;
                    steps.push(Step::Cook {
                        step: cook_step,
                        line: tok.line,
                    });
                    pos = new_pos;
                    continue;
                } else if let Some(rest) = strip_keyword(text, "plate") {
                    if let Some(started) = imperative_began {
                        return Err(region_violation("plate", tok.line, started));
                    }
                    let command = parse_single_quoted_string(rest, tok.line)?;
                    steps.push(Step::Plate {
                        step: PlateStep { command },
                        line: tok.line,
                    });
                } else if let Some(rest) = strip_keyword(text, "test") {
                    if let Some(started) = imperative_began {
                        return Err(region_violation("test", tok.line, started));
                    }
                    let (command, rest) = parse_test_command(rest, tok.line)?;
                    let (timeout, rest) = parse_test_timeout(rest);
                    let should_fail = rest.trim() == "should_fail";
                    steps.push(Step::Test {
                        step: TestStep {
                            command,
                            timeout,
                            should_fail,
                        },
                        line: tok.line,
                    });
                } else if is_module_call(text) {
                    if let Some(started) = imperative_began {
                        return Err(region_violation("module-call", tok.line, started));
                    }
                    let (code, new_pos) =
                        collect_module_call(text, tok.line, tokens, pos, source_lines)?;
                    // Module calls are register-phase per §4.11. Single-line
                    // desugars to InlineLua, multi-line to InlineLuaBlock.
                    if code.contains('\n') {
                        steps.push(Step::InlineLuaBlock {
                            code,
                            line: tok.line,
                        });
                    } else {
                        steps.push(Step::InlineLua {
                            code,
                            line: tok.line,
                        });
                    }
                    pos = new_pos;
                    continue;
                } else if let Some(cmd) = text.strip_prefix('@') {
                    let cmd = cmd.to_string();
                    if cmd.is_empty() {
                        return Err(ParseError::Parse {
                            line: tok.line,
                            message: "interactive '@' prefix requires a command".to_string(),
                        });
                    }
                    if imperative_began.is_none() {
                        imperative_began = Some(tok.line);
                    }
                    steps.push(Step::Shell {
                        command: cmd,
                        line: tok.line,
                        interactive: true,
                    });
                } else {
                    if imperative_began.is_none() {
                        imperative_began = Some(tok.line);
                    }
                    steps.push(Step::Shell {
                        command: text.clone(),
                        line: tok.line,
                        interactive: false,
                    });
                }
                pos += 1;
            }
            Token::LuaLine(code) => {
                if imperative_began.is_none() {
                    imperative_began = Some(tok.line);
                }
                steps.push(Step::Lua {
                    code: code.clone(),
                    line: tok.line,
                });
                pos += 1;
            }
            Token::LuaBlockOpen => {
                if imperative_began.is_none() {
                    imperative_began = Some(tok.line);
                }
                let block_line = tok.line;
                pos += 1;
                let (code, new_pos) = collect_lua_block(block_line, tokens, pos, source_lines)?;
                steps.push(Step::LuaBlock {
                    code,
                    line: block_line,
                });
                pos = new_pos;
            }
            Token::InlineLuaLine(code) => {
                if let Some(started) = imperative_began {
                    return Err(region_violation("inline-lua (`>>`)", tok.line, started));
                }
                steps.push(Step::InlineLua {
                    code: code.clone(),
                    line: tok.line,
                });
                pos += 1;
            }
            Token::InlineLuaBlockOpen => {
                if let Some(started) = imperative_began {
                    return Err(region_violation("inline-lua-block (`>>{`)", tok.line, started));
                }
                let block_line = tok.line;
                pos += 1;
                let (code, new_pos) = collect_lua_block(block_line, tokens, pos, source_lines)?;
                steps.push(Step::InlineLuaBlock {
                    code,
                    line: block_line,
                });
                pos = new_pos;
            }
            Token::RecipeHeader { .. } => {
                return Err(ParseError::Parse {
                    line: tok.line,
                    message: format!("recipe '{}' was not closed with 'end'", name),
                });
            }
            Token::ConfigHeader { .. } => {
                return Err(ParseError::Parse {
                    line: tok.line,
                    message: "unexpected config declaration inside a recipe".to_string(),
                });
            }
            Token::UseDecl { .. } => {
                return Err(ParseError::Parse {
                    line: tok.line,
                    message: "use statements must appear before recipes".to_string(),
                });
            }
            Token::ImportDecl { .. } => {
                return Err(ParseError::Parse {
                    line: tok.line,
                    message: "import declarations must appear before recipes".to_string(),
                });
            }
        }
    }

    Err(ParseError::Parse {
        line: recipe_line,
        message: format!("recipe '{}' was not closed with 'end'", name),
    })
}
