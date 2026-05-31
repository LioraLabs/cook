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

/// Parse the `produce` step: an optional `as json|lines` typing modifier, then
/// a body (`>{ … }` Lua block or `{ … }` shell block). The `as` modifier is
/// valid ONLY on the shell-block form — a `>{ … }` Lua block already returns a
/// structured value, so `as` on it is a register-phase error (§22.5).
pub(crate) fn parse_produce_step(
    rest: &str,
    line: usize,
    tokens: &[Located<Token>],
    current_pos: usize,
    source_lines: &[&str],
) -> Result<(ProbeProduce, usize), ParseError> {
    let rest = rest.trim_start();
    // optional `as json` / `as lines` typing modifier
    let (typing, body_src): (Option<ShellProduceType>, &str) =
        if let Some(after) = strip_keyword(rest, "as") {
            if let Some(tail) = strip_keyword(after, "json") {
                (Some(ShellProduceType::Json), tail)
            } else if let Some(tail) = strip_keyword(after, "lines") {
                (Some(ShellProduceType::Lines), tail)
            } else {
                return Err(ParseError::Parse { line, message:
                    "produce: `as` must be followed by `json` or `lines`".into() });
            }
        } else {
            (None, rest)
        };

    let (body, new_pos) =
        crate::cook_line::parse_body_payload(body_src, line, tokens, current_pos, source_lines, "produce")?;

    let produce = match (body, typing) {
        (Body::LuaBlock(_), Some(_)) => {
            return Err(ParseError::Parse { line, message:
                "produce: `as` is only valid on a shell-block produce; a `>{ … }` Lua block already returns a typed value".into() });
        }
        (Body::LuaBlock(code), None) => ProbeProduce::Lua(code),
        (Body::ShellBlock(cmds), Some(t)) => ProbeProduce::Shell { commands: cmds, typing: t },
        (Body::ShellBlock(cmds), None) => ProbeProduce::Shell { commands: cmds, typing: ShellProduceType::String },
    };
    Ok((produce, new_pos))
}
