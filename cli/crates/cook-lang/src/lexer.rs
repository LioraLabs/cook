use thiserror::Error;

#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    Comment(String),
    RecipeHeader { name: String, deps: Vec<String> },
    ChoreHeader { name: String, params: Vec<crate::ast::ChoreParam>, deps: Vec<String> },
    ConfigHeader { name: Option<String> },
    UseDecl { name: String },
    ImportDecl { name: String, path: String },
    RegisterHeader,
    ProbeHeader { name: String, deps: Vec<String> },
    LuaLine(String),
    LuaBlockOpen,
    InlineLuaLine(String),
    InlineLuaBlockOpen,
    Blank,
    Content(String),
}

#[derive(Debug, Clone)]
pub struct Located<T> {
    pub value: T,
    pub line: usize,
}

#[derive(Error, Debug)]
pub enum LexError {
    #[error("line {line}: unterminated string")]
    UnterminatedString { line: usize },
    #[error("line {line}: expected quoted name after keyword")]
    MissingRecipeName { line: usize },
    #[error("line {line}: '{segment}' is a reserved word and cannot be used in this position in a dotted recipe name")]
    ReservedRecipeName { line: usize, segment: String },
    #[error("line {line}: a run of three or more `>` characters at line start is reserved")]
    ReservedTripleArrow { line: usize },
    #[error("line {line}: 'use' name '{name}' is not a valid Lua identifier (must match /^[A-Za-z_][A-Za-z0-9_]*$/; '-' and '.' are not permitted)")]
    InvalidUseName { name: String, line: usize },
    #[error("line {line}: recipe name '{name}': dotted recipe names are not permitted at the declaration site; use 'import alias path' for cross-Cookfile namespacing")]
    DottedDeclaredRecipeName { name: String, line: usize },
    #[error("line {line}: chore name '{name}': dotted chore names are not permitted at the declaration site; use 'import alias path' for cross-Cookfile namespacing")]
    DottedDeclaredChoreName { name: String, line: usize },
    #[error("line {line}: chore '{chore}': duplicate parameter '{name}'")]
    DuplicateChoreParam { line: usize, chore: String, name: String },
    #[error("line {line}: chore '{chore}': required parameter '{required}' must precede defaulted parameter '{defaulted}'")]
    RequiredAfterDefaulted { line: usize, chore: String, required: String, defaulted: String },
    #[error("line {line}: chore '{chore}': default for parameter '{name}' must be a quoted string")]
    BadChoreParamDefault { line: usize, chore: String, name: String },
    #[error("line {line}: chore '{chore}': unclosed default for parameter '{name}' (expected closing '\"' or ')')")]
    UnclosedChoreParamDefault { line: usize, chore: String, name: String },
    #[error("line {line}: recipe '{name}': recipes don't take parameters; use a 'chore' or a config preset")]
    RecipeWithParams { line: usize, name: String },
    #[error("line {line}: expected a probe name after `probe`")]
    MissingProbeName { line: usize },
    #[error("line {line}: probe '{name}': unexpected token after the probe name; a dependency list is introduced with ':' (e.g. `probe {name}: dep1 dep2`)")]
    ProbeExtraTokens { name: String, line: usize },
    #[error("line {line}: probe name '{name}': a probe key is at most `IDENT:IDENT`; '{name}' has too many ':' segments")]
    MalformedProbeName { name: String, line: usize },
    #[error("line {line}: chore '{chore}': variadic parameter '{name}' must be the final parameter")]
    VariadicNotLast { line: usize, chore: String, name: String },
    #[error("line {line}: chore '{chore}': at most one variadic parameter permitted; found '{first}' and '{second}'")]
    MultipleVariadics { line: usize, chore: String, first: String, second: String },
    #[error("line {line}: chore '{chore}': variadic parameter '{name}' cannot have a default; use '*{name}' for an optional variadic")]
    VariadicWithDefault { line: usize, chore: String, name: String },
    #[error("line {line}: chore '{chore}': parameter name '{name}' contains '.'; parameter names must not contain '.'")]
    DottedChoreParam { line: usize, chore: String, name: String },
}

const RESERVED_RECIPE_SEGMENTS: &[&str] = &["stem", "name", "ext", "dir", "in", "out", "env"];

fn check_reserved_recipe_name(name: &str, line: usize) -> Result<(), LexError> {
    let first_segment = name.split('.').next().unwrap_or(name);
    if first_segment == "env" {
        return Err(LexError::ReservedRecipeName {
            line,
            segment: "env".to_string(),
        });
    }
    let last_segment = name.rsplit('.').next().unwrap_or(name);
    if RESERVED_RECIPE_SEGMENTS.contains(&last_segment) {
        return Err(LexError::ReservedRecipeName {
            line,
            segment: last_segment.to_string(),
        });
    }
    Ok(())
}

fn is_ident_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

fn is_ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.'
}

/// Validate that a `use NAME` argument is a Lua identifier — `^[A-Za-z_][A-Za-z0-9_]*$`.
/// Per CS-0035, `use` names are dropped verbatim into a `local NAME = ...` Lua binding
/// by the codegen layer, so they MUST be syntactically valid Lua identifiers; otherwise
/// the generated Lua is malformed and the failure surfaces far from the source. Hyphens
/// and dots are rejected outright; in particular, hyphen rejection eliminates the
/// `foo-bar` / `foo_bar` collision under the codegen's `replace('-', "_")` workaround.
fn check_use_name(name: &str, line: usize) -> Result<(), LexError> {
    let mut chars = name.chars();
    let ok_start = matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_');
    let ok_rest = chars.all(|c| c.is_ascii_alphanumeric() || c == '_');
    if !ok_start || !ok_rest || name.is_empty() {
        return Err(LexError::InvalidUseName { name: name.to_string(), line });
    }
    Ok(())
}

/// Parse either a quoted name (`"foo"`) or a bare identifier (`foo`, `backend.build`).
/// Returns `(name, remaining_text)`.
fn parse_name(text: &str, line: usize) -> Result<(String, &str), LexError> {
    let text = text.trim_start();
    if text.starts_with('"') {
        let rest = &text[1..];
        let end = rest
            .find('"')
            .ok_or(LexError::UnterminatedString { line })?;
        Ok((rest[..end].to_string(), rest[end + 1..].trim_start()))
    } else {
        let end = text
            .find(|c: char| !is_ident_char(c))
            .unwrap_or(text.len());
        if end == 0 || !is_ident_start(text.as_bytes()[0] as char) {
            return Err(LexError::MissingRecipeName { line });
        }
        Ok((text[..end].to_string(), text[end..].trim_start()))
    }
}

/// Brace-balanced scan for a Lua-expression default (`=( EXPR )`).
///
/// `text` is the content AFTER the opening `(`. Scans until the matching
/// `)` is found, honouring nested parens and basic double/single-quoted
/// string literals. Returns `(trimmed_expr, text_after_close_paren)`.
///
/// **Limitation (v1):** Lua long-bracket strings (`[[...]]`) inside the
/// expression are NOT handled by this scanner; they are treated as ordinary
/// bracket characters. Authors who need a long-bracket string inside a
/// default expression must use a regular double-quoted string instead.
/// This limitation is documented in §7.1.1 of the Cook Standard.
fn scan_balanced_paren<'a>(
    text: &'a str,
    line: usize,
    chore: &str,
    name: &str,
) -> Result<(String, &'a str), LexError> {
    let bytes = text.as_bytes();
    let mut depth = 1usize;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'(' => {
                depth += 1;
                i += 1;
            }
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Ok((text[..i].trim().to_string(), &text[i + 1..]));
                }
                i += 1;
            }
            b'"' | b'\'' => {
                let quote = bytes[i];
                i += 1;
                while i < bytes.len() && bytes[i] != quote {
                    if bytes[i] == b'\\' && i + 1 < bytes.len() {
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
                if i >= bytes.len() {
                    return Err(LexError::UnclosedChoreParamDefault {
                        line,
                        chore: chore.into(),
                        name: name.into(),
                    });
                }
                i += 1; // skip closing quote
            }
            _ => {
                i += 1;
            }
        }
    }
    Err(LexError::UnclosedChoreParamDefault {
        line,
        chore: chore.into(),
        name: name.into(),
    })
}

/// Parse the chore parameter list from `text`.
///
/// Reads bare-identifier params with optional `="STRING"` or `=( EXPR )`
/// defaults, enforces required-before-defaulted ordering and duplicate-name
/// rejection. Stops at `:` or end-of-input.
///
/// Returns `(params, remaining_text)`.
fn parse_chore_params<'a>(
    text: &'a str,
    chore_name: &str,
    line: usize,
) -> Result<(Vec<crate::ast::ChoreParam>, &'a str), LexError> {
    use crate::ast::ChoreParam;

    // NOTE: every `col: 0` below is a known placeholder. The ChoreParam AST
    // variants carry a `col` field for future column-precise diagnostics
    // ("bad chore param at line 3, col 12"), but parse_chore_params operates
    // on the already-trimmed slice after the chore-name and doesn't know the
    // absolute column. Threading the original-line offset through is tracked
    // separately and will be wired up when a diagnostic renderer needs it.
    let mut params: Vec<ChoreParam> = Vec::new();
    let mut seen_names: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut seen_defaulted = false;
    let mut seen_variadic: Option<String> = None; // name of the variadic param if one was seen
    let mut remaining = text.trim_start();

    loop {
        // Stop if we're at a `:` (dep list) or end of input.
        if remaining.is_empty() || remaining.starts_with(':') {
            break;
        }

        // Check for variadic sigil (`+` or `*`).
        let variadic_sigil: Option<char>;
        if remaining.starts_with('+') || remaining.starts_with('*') {
            let sigil = remaining.as_bytes()[0] as char;
            variadic_sigil = Some(sigil);
            remaining = remaining[1..].trim_start_matches(' ');
        } else {
            variadic_sigil = None;
        }

        // If a variadic was already seen and we're still parsing params,
        // that means there's a non-variadic (or another variadic) after the variadic.
        // We detect this after parsing the name below.

        // Must start with an ident-start character; otherwise stop (unknown token).
        if remaining.is_empty() || !is_ident_start(remaining.as_bytes()[0] as char) {
            break;
        }

        // Parse bare identifier (param name). Param names use only [A-Za-z0-9_];
        // dots and hyphens are NOT allowed in param names.
        let end = remaining
            .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
            .unwrap_or(remaining.len());
        let param_name = remaining[..end].to_string();
        remaining = &remaining[end..];

        // Dot-in-name check (§7.1.1: parameter names MUST NOT contain '.').
        // Catches `chore lint foo.bar` before it becomes a confusing runtime
        // "unknown placeholder '$<bar>'" error: the bare-ident scan above
        // stops at the dot, so without this check the trailing `.bar` is
        // silently dropped at parameter-list level.
        if remaining.starts_with('.') {
            let after_dot = &remaining[1..];
            let tail_end = after_dot
                .find(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != '.')
                .unwrap_or(after_dot.len());
            if tail_end > 0 {
                let full_name = format!("{}.{}", param_name, &after_dot[..tail_end]);
                return Err(LexError::DottedChoreParam {
                    line,
                    chore: chore_name.to_string(),
                    name: full_name,
                });
            }
        }
        remaining = remaining.trim_start();

        // CS-0132: the reserved-segment ban (§A.2) is gated to dotted
        // reference surfaces and the env-first prefix; it does NOT constrain
        // chore parameter names. `name`, `in`, `out`, etc. are all legal.

        // Duplicate-name check.
        if !seen_names.insert(param_name.clone()) {
            return Err(LexError::DuplicateChoreParam {
                line,
                chore: chore_name.to_string(),
                name: param_name,
            });
        }

        // Handle variadic sigil path.
        if let Some(sigil) = variadic_sigil {
            // Multiple-variadic check: if a prior variadic was already seen, error.
            if let Some(ref first_var) = seen_variadic {
                return Err(LexError::MultipleVariadics {
                    line,
                    chore: chore_name.to_string(),
                    first: first_var.clone(),
                    second: param_name,
                });
            }

            // Variadic-with-default check: `=` immediately after name is an error.
            if remaining.starts_with('=') {
                return Err(LexError::VariadicWithDefault {
                    line,
                    chore: chore_name.to_string(),
                    name: param_name,
                });
            }

            // Record variadic name (clone before moving into the ChoreParam).
            seen_variadic = Some(param_name.clone());

            // Push the variadic variant.
            match sigil {
                '+' => params.push(ChoreParam::VariadicPlus { name: param_name.clone(), line, col: 0 }),
                '*' => params.push(ChoreParam::VariadicStar { name: param_name.clone(), line, col: 0 }),
                _ => unreachable!(),
            }

            // After a variadic, only `:` or end-of-input is legal.
            // If the next token looks like a bare identifier or another sigil, error.
            if !remaining.is_empty() && !remaining.starts_with(':') {
                let next_byte = remaining.as_bytes()[0] as char;
                if next_byte == '+' || next_byte == '*' {
                    // Another variadic sigil follows — MultipleVariadics.
                    // Peek past the sigil to get the second variadic's name.
                    let after_sigil = remaining[1..].trim_start_matches(' ');
                    let name2_end = after_sigil
                        .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                        .unwrap_or(after_sigil.len());
                    let second = after_sigil[..name2_end].to_string();
                    return Err(LexError::MultipleVariadics {
                        line,
                        chore: chore_name.to_string(),
                        first: param_name,
                        second,
                    });
                } else if is_ident_start(next_byte) {
                    // A non-variadic param after the variadic — VariadicNotLast.
                    return Err(LexError::VariadicNotLast {
                        line,
                        chore: chore_name.to_string(),
                        name: param_name,
                    });
                }
            }

            // Continue loop — the next iteration will see `:` or EOF and break.
            continue;
        }

        // Non-variadic path: if a variadic was already seen, this param comes after it.
        if let Some(ref var_name) = seen_variadic {
            return Err(LexError::VariadicNotLast {
                line,
                chore: chore_name.to_string(),
                name: var_name.clone(),
            });
        }

        // Check for a default value.
        if remaining.starts_with('=') {
            let after_eq = &remaining[1..];
            if after_eq.starts_with('"') {
                // String default: parse up to closing `"`.
                let inner = &after_eq[1..];
                let close = inner
                    .find('"')
                    .ok_or_else(|| LexError::UnclosedChoreParamDefault {
                        line,
                        chore: chore_name.to_string(),
                        name: param_name.clone(),
                    })?;
                let default_val = inner[..close].to_string();
                remaining = inner[close + 1..].trim_start();

                params.push(ChoreParam::DefaultedString {
                    name: param_name,
                    default: default_val,
                    line,
                    col: 0,
                });
                seen_defaulted = true;
            } else if after_eq.starts_with('(') {
                // Lua-expression default: brace-balanced scan.
                let rest = &after_eq[1..];
                let (lua_expr, after_close) =
                    scan_balanced_paren(rest, line, chore_name, &param_name)?;
                params.push(ChoreParam::DefaultedLua {
                    name: param_name,
                    default_lua: lua_expr,
                    line,
                    col: 0,
                });
                seen_defaulted = true;
                remaining = after_close.trim_start();
                continue;
            } else {
                return Err(LexError::BadChoreParamDefault {
                    line,
                    chore: chore_name.to_string(),
                    name: param_name,
                });
            }
        } else {
            // Required param — must precede any defaulted param.
            if seen_defaulted {
                // Find the name of the most-recently-added defaulted param.
                let defaulted_name = params
                    .iter()
                    .rev()
                    .find_map(|p| match p {
                        ChoreParam::DefaultedString { name, .. }
                        | ChoreParam::DefaultedLua { name, .. } => Some(name.clone()),
                        _ => None,
                    })
                    .unwrap_or_default();
                return Err(LexError::RequiredAfterDefaulted {
                    line,
                    chore: chore_name.to_string(),
                    required: param_name,
                    defaulted: defaulted_name,
                });
            }
            params.push(ChoreParam::Required { name: param_name, line, col: 0 });
        }
    }

    Ok((params, remaining))
}

/// Parse a space-separated list of names (quoted or bare).
fn parse_names(text: &str, line: usize) -> Result<Vec<String>, LexError> {
    let mut result = Vec::new();
    let mut remaining = text.trim();
    while !remaining.is_empty() {
        let (name, rest) = parse_name(remaining, line)?;
        result.push(name);
        remaining = rest;
    }
    Ok(result)
}

/// Scan one `probe_ref ::= IDENT (":" IDENT)?` from the front of `s`.
/// Each segment uses the recipe-name char set `is_ident_char`
/// (`[A-Za-z0-9_.-]`, CS-0131). A `:` is consumed into the name ONLY when
/// immediately followed by an ident-start char (no whitespace) — this is the
/// module-prefix colon. A second contiguous `:` is the malformed triple-colon
/// case. Returns `(name, rest_after_name)`.
fn scan_probe_ref(s: &str, line: usize) -> Result<(String, &str), LexError> {
    fn seg_len(s: &str) -> usize {
        s.find(|c: char| !is_ident_char(c)).unwrap_or(s.len())
    }
    let s = s.trim_start();
    let n1 = seg_len(s);
    if n1 == 0 || !is_ident_start(s.chars().next().unwrap_or('\0')) {
        return Err(LexError::MissingProbeName { line });
    }
    let after1 = &s[n1..];
    // module-prefix colon: ':' immediately followed by an ident-start char
    if let Some(rest) = after1.strip_prefix(':') {
        if !rest.is_empty() && is_ident_start(rest.chars().next().unwrap_or('\0')) {
            let n2 = seg_len(rest);
            let name = format!("{}:{}", &s[..n1], &rest[..n2]);
            let after2 = &rest[n2..];
            // a third contiguous ':IDENT' is malformed
            if let Some(more) = after2.strip_prefix(':') {
                if !more.is_empty() && is_ident_start(more.chars().next().unwrap_or('\0')) {
                    return Err(LexError::MalformedProbeName { name: format!("{name}:…"), line });
                }
            }
            return Ok((name, after2));
        }
    }
    Ok((s[..n1].to_string(), after1))
}

/// Parse a probe header's dependency list: whitespace-separated `probe_ref`s.
fn parse_probe_dep_list(text: &str, line: usize) -> Result<Vec<String>, LexError> {
    let mut deps = Vec::new();
    let mut rest = text.trim();
    while !rest.is_empty() {
        // probe_ref ::= BARE_PROBE_KEY | STRING (App. A.3.2). Quoted arm mirrors
        // the probe-NAME dispatch; the STRING form is the escape hatch for keys
        // outside is_ident_char. CS-0131 / COOK-71.
        let (name, after) = if rest.starts_with('"') {
            parse_name(rest, line)?
        } else {
            scan_probe_ref(rest, line)?
        };
        deps.push(name);
        rest = after.trim_start();
    }
    Ok(deps)
}

pub fn tokenize(source: &str) -> Result<Vec<Located<Token>>, LexError> {
    let mut tokens = Vec::new();

    for (idx, line) in source.lines().enumerate() {
        let line_num = idx + 1;
        let trimmed = line.trim();

        let token = if trimmed.is_empty() {
            Token::Blank
        } else if trimmed.starts_with('#') {
            Token::Comment(trimmed[1..].to_string())
        } else if trimmed.starts_with(">>>") {
            // Reserved for future prefixes; reject explicitly so a four-arrow
            // line is a sharp diagnostic rather than a confusing parse error
            // further down (§{lexical.line-prefixes}).
            return Err(LexError::ReservedTripleArrow { line: line_num });
        } else if trimmed.starts_with(">>{") {
            Token::InlineLuaBlockOpen
        } else if trimmed.starts_with(">>") {
            let code = &trimmed[2..];
            let code = code.strip_prefix(' ').unwrap_or(code);
            Token::InlineLuaLine(code.to_string())
        } else if trimmed.starts_with(">{") {
            Token::LuaBlockOpen
        } else if trimmed.starts_with('>') {
            let code = &trimmed[1..];
            let code = code.strip_prefix(' ').unwrap_or(code);
            Token::LuaLine(code.to_string())
        } else if !line.starts_with(|c: char| c.is_whitespace())
            && trimmed.starts_with("recipe")
            && trimmed.len() > 6
            && (trimmed.as_bytes()[6] == b' '
                || trimmed.as_bytes()[6] == b'\t'
                || trimmed.as_bytes()[6] == b'"')
        {
            let rest = trimmed["recipe".len()..].trim();
            let (name, after_name) = parse_name(rest, line_num)?;
            if name.contains('.') {
                // CS-0132: the reserved-segment ban no longer constrains
                // undotted declaration names. A dotted declaration name is
                // rejected outright; run the reserved check first so a dotted
                // reserved reference (env.foo, foo.out) keeps its specific
                // "reserved word" diagnostic rather than the generic dotted one.
                check_reserved_recipe_name(&name, line_num)?;
                return Err(LexError::DottedDeclaredRecipeName { name, line: line_num });
            }

            // Recipes don't take parameters. Reject any token between the name
            // and the `:` (or end of header) that is not a dep list.
            let after_name_trimmed = after_name.trim_start();
            if !after_name_trimmed.is_empty() && !after_name_trimmed.starts_with(':') {
                return Err(LexError::RecipeWithParams { line: line_num, name });
            }

            let deps = if let Some(after_colon) = after_name.strip_prefix(':') {
                parse_names(after_colon.trim(), line_num)?
            } else {
                vec![]
            };

            Token::RecipeHeader { name, deps }
        } else if !line.starts_with(|c: char| c.is_whitespace())
            && trimmed.starts_with("chore")
            && trimmed.len() > 5
            && (trimmed.as_bytes()[5] == b' '
                || trimmed.as_bytes()[5] == b'\t'
                || trimmed.as_bytes()[5] == b'"')
        {
            let rest = trimmed["chore".len()..].trim();
            let (name, after_name) = parse_name(rest, line_num)?;
            if name.contains('.') {
                // CS-0132: see the recipe reduction above — reserved check is
                // gated to the dotted path so undotted `chore name` is legal.
                check_reserved_recipe_name(&name, line_num)?;
                return Err(LexError::DottedDeclaredChoreName { name, line: line_num });
            }

            let (params, after_params) = parse_chore_params(after_name, &name, line_num)?;
            let deps = if let Some(after_colon) = after_params.strip_prefix(':') {
                parse_names(after_colon.trim(), line_num)?
            } else {
                vec![]
            };

            Token::ChoreHeader { name, params, deps }
        } else if !line.starts_with(|c: char| c.is_whitespace()) && trimmed == "config" {
            Token::ConfigHeader { name: None }
        } else if !line.starts_with(|c: char| c.is_whitespace())
            && trimmed.starts_with("config")
            && trimmed.len() > 6
            && (trimmed.as_bytes()[6] == b' '
                || trimmed.as_bytes()[6] == b'\t'
                || trimmed.as_bytes()[6] == b'"')
        {
            let rest = trimmed["config".len()..].trim();
            let (name, _) = parse_name(rest, line_num)?;
            Token::ConfigHeader { name: Some(name) }
        } else if !line.starts_with(|c: char| c.is_whitespace())
            && trimmed.starts_with("register")
            && (trimmed.len() == 8
                || (trimmed.len() > 8
                    && (trimmed.as_bytes()[8] == b' ' || trimmed.as_bytes()[8] == b'\t')))
        {
            Token::RegisterHeader
        } else if !line.starts_with(|c: char| c.is_whitespace())
            && trimmed.starts_with("probe")
            && trimmed.len() > 5
            && (trimmed.as_bytes()[5] == b' '
                || trimmed.as_bytes()[5] == b'\t'
                || trimmed.as_bytes()[5] == b'"')
        {
            let rest = trimmed["probe".len()..].trim();
            let (name, after_name) = if rest.starts_with('"') {
                parse_name(rest, line_num)?
            } else {
                scan_probe_ref(rest, line_num)?
            };
            // scan_probe_ref returns an untrimmed remainder; parse_name already trims. Trim covers both.
            let after_name = after_name.trim_start();
            let deps = if let Some(after_colon) = after_name.strip_prefix(':') {
                parse_probe_dep_list(after_colon.trim(), line_num)?
            } else if after_name.is_empty() {
                vec![]
            } else {
                return Err(LexError::ProbeExtraTokens { name, line: line_num });
            };
            Token::ProbeHeader { name, deps }
        } else if !line.starts_with(|c: char| c.is_whitespace())
            && trimmed.starts_with("use")
            && trimmed.len() > 3
            && (trimmed.as_bytes()[3] == b' '
                || trimmed.as_bytes()[3] == b'\t'
                || trimmed.as_bytes()[3] == b'"')
        {
            let rest = trimmed["use".len()..].trim();
            let (name, _) = parse_name(rest, line_num)?;
            check_use_name(&name, line_num)?;
            Token::UseDecl { name }
        } else if !line.starts_with(|c: char| c.is_whitespace())
            && trimmed.starts_with("import")
            && trimmed.len() > 6
            && (trimmed.as_bytes()[6] == b' ' || trimmed.as_bytes()[6] == b'\t')
        {
            let rest = trimmed["import".len()..].trim();
            let space_pos = rest.find(|c: char| c == ' ' || c == '\t');
            match space_pos {
                Some(pos) => {
                    let name = rest[..pos].to_string();
                    let path = rest[pos..].trim().to_string();
                    if path.is_empty() {
                        return Err(LexError::MissingRecipeName { line: line_num });
                    }
                    Token::ImportDecl { name, path }
                }
                None => {
                    return Err(LexError::MissingRecipeName { line: line_num });
                }
            }
        } else {
            // Anything else: a Content line. Whether it dispatches inside a
            // recipe body (shell_command, interactive_command, module_call,
            // ingredients_step, etc.) or is rejected at top level is the
            // syntactic-layer's concern (§{grammar.overview}, §{grammar.step-dispatch}).
            Token::Content(trimmed.to_string())
        };

        tokens.push(Located {
            value: token,
            line: line_num,
        });
    }

    Ok(tokens)
}

#[cfg(test)]
#[path = "tests/lexer_tests.rs"]
mod tests;
