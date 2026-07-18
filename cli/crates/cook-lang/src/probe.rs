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
    let mut producer: Option<ProbeProduce> = None;

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
            Token::LuaBlockOpen => {
                // A bare `>{ … }` line is the Lua producer (§22.5.2). Unlike the
                // shell/`json`/`lines`/`tools`/`envs` forms (which lex as
                // `Content`), `>{` lexes as its own opener token whose remaining
                // line content the lexer dropped — recover it from the source so
                // both the inline `>{ … }` and multi-line forms parse uniformly
                // through `parse_producer`.
                if producer.is_some() {
                    return Err(ParseError::Parse { line: tok.line,
                        message: "probe: at most one producer per probe".into() });
                }
                let raw = source_lines
                    .get(tok.line.saturating_sub(1))
                    .copied()
                    .unwrap_or("")
                    .trim_start();
                let (p, new_pos) = parse_producer(raw, tok.line, tokens, pos, source_lines)?;
                producer = Some(p);
                pos = new_pos;
            }
            Token::Content(text) => {
                if crate::recipe::is_module_call(text) {
                    let raw = source_lines.get(tok.line.saturating_sub(1)).copied().unwrap_or("");
                    if !raw.starts_with(|c: char| c.is_whitespace()) { break; }
                }
                if let Some(rest) = strip_keyword(text, "ingredients") {
                    if producer.is_some() {
                        return Err(ParseError::Parse { line: tok.line,
                            message: "probe: `ingredients` must appear before the producer".into() });
                    }
                    if !ingredients.is_empty() || !excludes.is_empty() {
                        return Err(ParseError::Parse { line: tok.line,
                            message: "probe: at most one `ingredients` per probe".into() });
                    }
                    let (inc, exc, new_pos) =
                        parse_ingredients_line(rest, tok.line, tokens, pos, source_lines)?;
                    ingredients = inc; excludes = exc; pos = new_pos;
                    continue;
                } else {
                    // Any other body content is the producer (§22.5.2). The
                    // producer KIND leads; there is exactly one per probe.
                    if producer.is_some() {
                        return Err(ParseError::Parse { line: tok.line,
                            message: "probe: at most one producer per probe".into() });
                    }
                    let (p, new_pos) =
                        parse_producer(text, tok.line, tokens, pos, source_lines)?;
                    // A `files` producer's glob set IS its file-input
                    // fingerprint set (CS-0148); a separate `ingredients`
                    // line would declare a second, divergable one.
                    if matches!(p, ProbeProduce::Files { .. })
                        && (!ingredients.is_empty() || !excludes.is_empty())
                    {
                        return Err(ParseError::Parse { line: tok.line,
                            message: "probe: a `files` producer declares its own file set; \
                                a separate `ingredients` line is not allowed".into() });
                    }
                    producer = Some(p); pos = new_pos;
                    continue;
                }
            }
            _other => {
                return Err(ParseError::Parse { line: tok.line,
                    message: "probe body: only `ingredients` and a producer \
                        (`{ … }`, `json`/`lines`/`tools`/`envs`/`files`, or `>{ … }`) are allowed here"
                        .into() });
            }
        }
    }

    let produce = producer.ok_or_else(|| ParseError::Parse {
        line: probe_line,
        message: format!("probe '{name}' has no producer"),
    })?;

    Ok((Probe { name, deps, ingredients, excludes, produce, line: probe_line }, pos))
}

/// Parse a `tools`/`envs` brace name list: `{ a, b c }` → `["a","b","c"]`.
/// Separators are commas and/or whitespace (mixed allowed). Each name is a bare
/// `IDENT`. The list MUST be on one physical line and MUST be non-empty. The
/// `{ … }` here is a NAME LIST, not a shell/Lua body — so a `>{ … }` Lua block
/// is rejected by the caller before reaching this function.
fn parse_source_name_list(
    body_src: &str,
    line: usize,
    kind: &str, // "tools" or "envs", for diagnostics
) -> Result<Vec<String>, ParseError> {
    let s = body_src.trim_start();
    let inner = s
        .strip_prefix('{')
        .and_then(|t| t.trim_end().strip_suffix('}'))
        .ok_or_else(|| ParseError::Parse {
            line,
            message: format!(
                "{kind}: expected a brace name list `{{ a, b }}` on one line"
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
                    "{kind}: invalid name '{tok}'; names must be bare identifiers ([A-Za-z_][A-Za-z0-9_]*)"
                ),
            });
        }
        names.push(tok.to_string());
    }
    if names.is_empty() {
        return Err(ParseError::Parse {
            line,
            message: format!("{kind}: expected at least one name in `{{ … }}`"),
        });
    }
    Ok(names)
}

/// Parse a `files` brace glob list: `{ "a/*.c" !"a/gen/*.c" }` →
/// (globs, excludes). Each pattern is a quoted string following `ingredients`
/// syntax (`!"…"` excludes). The list MUST be on one physical line and MUST
/// contain at least one include glob. The `{ … }` here is a GLOB LIST, not a
/// shell/Lua body.
fn parse_files_glob_list(
    body_src: &str,
    line: usize,
) -> Result<(Vec<String>, Vec<String>), ParseError> {
    let s = body_src.trim_start();
    let inner = s
        .strip_prefix('{')
        .and_then(|t| t.trim_end().strip_suffix('}'))
        .ok_or_else(|| ParseError::Parse {
            line,
            message: "files: expected a brace glob list `{ \"a/*.c\" !\"a/gen/*.c\" }` on one line"
                .into(),
        })?;
    let mut globs = Vec::new();
    let mut excludes = Vec::new();
    let mut rest = inner.trim();
    while !rest.is_empty() {
        let is_exc = rest.starts_with('!');
        if is_exc {
            rest = rest[1..].trim_start();
        }
        let Some(r) = rest.strip_prefix('"') else {
            return Err(ParseError::Parse {
                line,
                message: format!(
                    "files: expected a quoted glob (`\"…\"` or `!\"…\"`), found: {rest}"
                ),
            });
        };
        let Some(end) = r.find('"') else {
            return Err(ParseError::Parse {
                line,
                message: "files: unterminated quoted glob".into(),
            });
        };
        let pat = &r[..end];
        if is_exc {
            excludes.push(pat.to_string());
        } else {
            globs.push(pat.to_string());
        }
        rest = r[end + 1..].trim_start_matches(',').trim_start();
    }
    if globs.is_empty() {
        return Err(ParseError::Parse {
            line,
            message: "files: expected at least one quoted glob in `{ … }`".into(),
        });
    }
    Ok((globs, excludes))
}

/// Finish a `files` producer: reject a `>{ … }` Lua block (a glob list, not a
/// body), parse the brace glob list, and advance past this physical line.
fn finish_files_list(
    tail: &str,
    line: usize,
    tokens: &[Located<Token>],
    current_pos: usize,
) -> Result<(ProbeProduce, usize), ParseError> {
    let t = tail.trim_start();
    if t.starts_with('>') {
        return Err(ParseError::Parse {
            line,
            message: "files: `{ \"glob\", … }` is a GLOB LIST, not a body; a `>{ … }` Lua block is not valid here"
                .into(),
        });
    }
    let (globs, excludes) = parse_files_glob_list(t, line)?;
    let mut new_pos = current_pos + 1;
    while new_pos < tokens.len() && tokens[new_pos].line <= line {
        new_pos += 1;
    }
    Ok((ProbeProduce::Files { globs, excludes }, new_pos))
}

/// Finish a `tools`/`envs` producer: reject a `>{ … }` Lua block (a body, not a
/// name list), parse the brace name list, and advance the token cursor past this
/// physical line.
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
                "{kind}: `{{ name, … }}` is a NAME LIST, not a body; a `>{{ … }}` Lua block is not valid here"
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
        "envs" => ProbeProduce::Envs(names),
        _ => unreachable!("finish_source_list called with kind={kind}"),
    };
    Ok((produce, new_pos))
}

/// Finish a `json`/`lines` typed shell producer. The leading keyword has already
/// been stripped; the remainder MUST be a `{ … }` shell block. A `>{ … }` Lua
/// block is rejected — `json`/`lines` type a shell block's stdout, and a Lua
/// block already returns a structured value (§22.5.2).
fn finish_typed_shell(
    tail: &str,
    typing: ShellProduceType,
    line: usize,
    tokens: &[Located<Token>],
    current_pos: usize,
    source_lines: &[&str],
) -> Result<(ProbeProduce, usize), ParseError> {
    let (body, new_pos) = crate::cook_line::parse_body_payload(
        tail, line, tokens, current_pos, source_lines, "probe",
    )?;
    match body {
        Body::LuaBlock(_) => Err(ParseError::Parse {
            line,
            message: "probe: `json`/`lines` is only valid on a shell block; a `>{ … }` Lua block already returns a structured value".into(),
        }),
        Body::ShellBlock(cmds) => Ok((ProbeProduce::Shell { commands: cmds, typing }, new_pos)),
    }
}

/// Parse a probe's producer (§22.5.2). The producer KIND leads; the braces are
/// that kind's body:
///
///   { … }            shell block  -> string (stdout, one trailing newline trimmed)
///   json  { … }      shell block  -> parsed + validated JSON
///   lines { … }      shell block  -> array of stdout lines
///   tools { cc, ld } name list    -> cached toolset fingerprint
///   envs  { CFLAGS } name list    -> cached env-set fingerprint
///   files { "a/*.c" } glob list   -> per-file content-hash manifest (CS-0148)
///   >{ … }           Lua block    -> structured value (the block's `return`)
///
/// `json`/`lines`/`tools`/`envs`/`files` are contextual keywords, valid only in this
/// probe-body position. A bare `{ … }`/`>{ … }` opener never matches a leading
/// keyword, so detection is unambiguous.
pub(crate) fn parse_producer(
    text: &str,
    line: usize,
    tokens: &[Located<Token>],
    current_pos: usize,
    source_lines: &[&str],
) -> Result<(ProbeProduce, usize), ParseError> {
    let text = text.trim_start();
    // Name-list producers: the braces hold a NAME LIST, not a body.
    if let Some(tail) = strip_keyword(text, "tools") {
        return finish_source_list(tail, line, tokens, current_pos, "tools");
    }
    if let Some(tail) = strip_keyword(text, "envs") {
        return finish_source_list(tail, line, tokens, current_pos, "envs");
    }
    // Glob-list producer: the braces hold a quoted GLOB LIST, not a body.
    if let Some(tail) = strip_keyword(text, "files") {
        return finish_files_list(tail, line, tokens, current_pos);
    }
    // Typed shell producers: the braces hold a shell block, typed.
    if let Some(tail) = strip_keyword(text, "json") {
        return finish_typed_shell(tail, ShellProduceType::Json, line, tokens, current_pos, source_lines);
    }
    if let Some(tail) = strip_keyword(text, "lines") {
        return finish_typed_shell(tail, ShellProduceType::Lines, line, tokens, current_pos, source_lines);
    }
    // Bare shell block (`{ … }` → string) or Lua block (`>{ … }` → structured).
    let (body, new_pos) = crate::cook_line::parse_body_payload(
        text, line, tokens, current_pos, source_lines, "probe",
    )?;
    Ok(match body {
        Body::LuaBlock(code) => (ProbeProduce::Lua(code), new_pos),
        Body::ShellBlock(cmds) => {
            (ProbeProduce::Shell { commands: cmds, typing: ShellProduceType::String }, new_pos)
        }
    })
}
