use crate::ast::*;
use crate::cook_line::{parse_ingredients_line, strip_keyword};
use crate::lexer::*;
use crate::ParseError;

pub(crate) fn parse_probe(
    name: String,
    deps: Vec<String>,
    probe_line: usize,
    tokens: &[Located<Token>],
    start: usize,
    source_lines: &[&str],
) -> Result<(Probe, usize), ParseError> {
    let mut pos = start;
    let mut ingredients: Vec<String> = Vec::new();
    let mut excludes: Vec<String> = Vec::new();
    let mut produce: Option<ProbeProduce> = None;

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
            Token::Comment(_) | Token::Blank => { pos += 1; }
            Token::Content(text) => {
                if crate::recipe::is_module_call(text) {
                    let raw = source_lines.get(tok.line.saturating_sub(1)).copied().unwrap_or("");
                    if !raw.starts_with(|c: char| c.is_whitespace()) { break; }
                }
                if let Some(rest) = strip_keyword(text, "ingredients") {
                    if produce.is_some() {
                        return Err(ParseError::Parse { line: tok.line,
                            message: "probe: `ingredients` must appear before `produce`".into() });
                    }
                    if !ingredients.is_empty() || !excludes.is_empty() {
                        return Err(ParseError::Parse { line: tok.line,
                            message: "probe: at most one `ingredients` per probe".into() });
                    }
                    let (inc, exc, new_pos) =
                        parse_ingredients_line(rest, tok.line, tokens, pos, source_lines)?;
                    ingredients = inc; excludes = exc; pos = new_pos;
                    continue;
                } else if let Some(rest) = strip_keyword(text, "produce") {
                    if produce.is_some() {
                        return Err(ParseError::Parse { line: tok.line,
                            message: "probe: at most one `produce` per probe".into() });
                    }
                    let (p, new_pos) =
                        parse_produce_step(rest, tok.line, tokens, pos, source_lines)?;
                    produce = Some(p); pos = new_pos;
                    continue;
                } else {
                    return Err(ParseError::Parse { line: tok.line, message: format!(
                        "probe body: expected `ingredients` or `produce`, found: {text}") });
                }
            }
            _other => {
                return Err(ParseError::Parse { line: tok.line,
                    message: "probe body: only `ingredients` and `produce` are allowed here \
                        (`>`/`>>` lines and shell/cook/plate/test steps are not valid in a probe body)"
                        .into() });
            }
        }
    }

    let produce = produce.ok_or_else(|| ParseError::Parse {
        line: probe_line,
        message: format!("probe '{name}' has no `produce` block"),
    })?;

    Ok((Probe { name, deps, ingredients, excludes, produce, line: probe_line }, pos))
}

/// Parse a `produce as tools|env` brace list: `{ a, b c }` → `["a","b","c"]`.
/// Separators are commas and/or whitespace (mixed allowed). Each name is a bare
/// `IDENT`. The list MUST be on one physical line and MUST be non-empty. The
/// `{ … }` here is a NAME LIST, not a shell/Lua body — so a `>{ … }` Lua block
/// is rejected by the caller before reaching this function.
fn parse_source_name_list(
    body_src: &str,
    line: usize,
    kind: &str, // "tools" or "env", for diagnostics
) -> Result<Vec<String>, ParseError> {
    let s = body_src.trim_start();
    let inner = s
        .strip_prefix('{')
        .and_then(|t| t.trim_end().strip_suffix('}'))
        .ok_or_else(|| ParseError::Parse {
            line,
            message: format!(
                "produce as {kind}: expected a brace name list `{{ a, b }}` on one line"
            ),
        })?;
    let mut names = Vec::new();
    for tok in inner.split(|c: char| c == ',' || c.is_whitespace()) {
        let tok = tok.trim();
        if tok.is_empty() {
            continue;
        }
        let mut chars = tok.chars();
        let head_ok = chars
            .next()
            .map(|c| c.is_ascii_alphabetic() || c == '_')
            .unwrap_or(false);
        let tail_ok = chars.all(|c| c.is_ascii_alphanumeric() || c == '_');
        if !head_ok || !tail_ok {
            return Err(ParseError::Parse {
                line,
                message: format!(
                    "produce as {kind}: invalid name '{tok}'; names must be bare identifiers ([A-Za-z_][A-Za-z0-9_]*)"
                ),
            });
        }
        names.push(tok.to_string());
    }
    if names.is_empty() {
        return Err(ParseError::Parse {
            line,
            message: format!("produce as {kind}: expected at least one name in `{{ … }}`"),
        });
    }
    Ok(names)
}

/// Finish a `produce as tools|env` step: reject a `>{ … }` Lua block (a body,
/// not a name list), parse the brace name list, and advance the token cursor
/// past this physical line.
fn finish_source_list(
    tail: &str,
    line: usize,
    tokens: &[Located<Token>],
    current_pos: usize,
    kind: &str,
) -> Result<(ProbeProduce, usize), ParseError> {
    let t = tail.trim_start();
    if t.starts_with('>') {
        return Err(ParseError::Parse {
            line,
            message: format!(
                "produce as {kind}: `{{ name, … }}` is a NAME LIST, not a body; a `>{{ … }}` Lua block is not valid here"
            ),
        });
    }
    let names = parse_source_name_list(t, line, kind)?;
    // The list is a single physical line: advance past every token on `line`.
    let mut new_pos = current_pos + 1;
    while new_pos < tokens.len() && tokens[new_pos].line <= line {
        new_pos += 1;
    }
    let produce = match kind {
        "tools" => ProbeProduce::Tools(names),
        "env" => ProbeProduce::Env(names),
        _ => unreachable!("finish_source_list called with kind={kind}"),
    };
    Ok((produce, new_pos))
}

/// Parse the `produce` step: an optional `as json|lines|tools|env` typing
/// modifier, then a body (`>{ … }` Lua block or `{ … }` shell block). The `as`
/// modifier is valid ONLY on the shell-block form for json/lines — a `>{ … }`
/// Lua block already returns a structured value, so `as` on it is a
/// register-phase error (§22.5). `as tools` and `as env` expect a brace NAME
/// LIST, not a shell or Lua body.
pub(crate) fn parse_produce_step(
    rest: &str,
    line: usize,
    tokens: &[Located<Token>],
    current_pos: usize,
    source_lines: &[&str],
) -> Result<(ProbeProduce, usize), ParseError> {
    let rest = rest.trim_start();
    // optional `as <kind>` modifier: json | lines | tools | env
    if let Some(after) = strip_keyword(rest, "as") {
        if let Some(tail) = strip_keyword(after, "tools") {
            return finish_source_list(tail, line, tokens, current_pos, "tools");
        }
        if let Some(tail) = strip_keyword(after, "env") {
            return finish_source_list(tail, line, tokens, current_pos, "env");
        }
        // json | lines fall through to the shell-body path below.
        let (typing, body_src): (ShellProduceType, &str) =
            if let Some(tail) = strip_keyword(after, "json") {
                (ShellProduceType::Json, tail)
            } else if let Some(tail) = strip_keyword(after, "lines") {
                (ShellProduceType::Lines, tail)
            } else {
                return Err(ParseError::Parse {
                    line,
                    message: "produce: `as` must be followed by `json`, `lines`, `tools`, or `env`".into(),
                });
            };
        let (body, new_pos) = crate::cook_line::parse_body_payload(
            body_src, line, tokens, current_pos, source_lines, "produce",
        )?;
        return match body {
            Body::LuaBlock(_) => Err(ParseError::Parse {
                line,
                message: "produce: `as` is only valid on a shell-block produce; a `>{ … }` Lua block already returns a typed value".into(),
            }),
            Body::ShellBlock(cmds) => Ok((ProbeProduce::Shell { commands: cmds, typing }, new_pos)),
        };
    }

    // No `as` modifier: bare shell block or `>{ … }` Lua block.
    let (body, new_pos) = crate::cook_line::parse_body_payload(
        rest, line, tokens, current_pos, source_lines, "produce",
    )?;
    Ok(match body {
        Body::LuaBlock(code) => (ProbeProduce::Lua(code), new_pos),
        Body::ShellBlock(cmds) => {
            (ProbeProduce::Shell { commands: cmds, typing: ShellProduceType::String }, new_pos)
        }
    })
}
