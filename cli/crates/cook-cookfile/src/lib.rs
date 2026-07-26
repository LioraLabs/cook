//! Structure-preserving Cookfile edits (Standard §22.12, CS-0179).
//!
//! The shared layer under every `cc.*` project-management verb: locate a
//! module call, splice an entry into one of its fields, append a declaration.
//!
//! # Why not decode and re-encode
//!
//! A Cookfile is a program, not a data file. Evaluating `cook_cc.bin({...})`
//! to a Lua table and re-rendering it destroys `standard = cxx_std` into
//! whatever that variable happened to hold, drops every comment, and reorders
//! fields by `pairs()` iteration order. None of that is recoverable, and none
//! of it announces itself — the author gets a file that still works and no
//! longer looks like anything they wrote.
//!
//! So nothing here re-renders. An edit is an insertion of bytes at one offset;
//! everything outside the inserted range is preserved by construction rather
//! than by care.
//!
//! # Locate, then scan, then splice
//!
//! The grammar locates; it never decides meaning. `cook-lang` remains the sole
//! authority on what a Cookfile means (see the scope note in
//! `tree-sitter-cook/bindings/rust/lib.rs`).
//!
//! tree-sitter gives us the two structural facts we need and cannot easily get
//! otherwise: which byte range is `recipe game`'s body, and which byte range is
//! the module call inside it. Both are genuinely hard to recover by scanning —
//! the call is multi-line, its braces nest, and its strings may contain braces.
//!
//! What tree-sitter deliberately does NOT give us is the field. Every Lua
//! payload is one opaque leaf by design (`grammar.js:161`, `:480`), so there is
//! no `links` node to find. Fields are therefore located by a targeted scan
//! *within* the call's span, which is sound precisely because the span
//! boundaries came from the parser.

use std::ops::Range;

use thiserror::Error;
use tree_sitter::{Node, Parser, Tree};

/// A Cookfile edit that could not be performed.
///
/// Every variant names something the caller can act on. This is the reason
/// the locate-then-scan strategy is worth its complexity: a lossy
/// decode/re-encode cannot fail this way. It cannot fail at all — it just
/// writes a different file and reports success.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum EditError {
    #[error("Cookfile does not parse; fix the syntax error before editing it")]
    Unparseable,

    #[error("no recipe named '{recipe}' in the Cookfile")]
    RecipeNotFound { recipe: String },

    #[error(
        "recipe '{recipe}' contains no module call to edit — expected something like \
         `cook_cc.bin({{ ... }})` in its body"
    )]
    NoModuleCall { recipe: String },

    #[error(
        "couldn't find '{field}' in the {callee} call in recipe '{recipe}' — \
         add {entry} to it manually"
    )]
    FieldNotFound {
        recipe: String,
        callee: String,
        field: String,
        entry: String,
    },

    #[error(
        "'{field}' in the {callee} call in recipe '{recipe}' is not a `{{ ... }}` list, \
         so {entry} cannot be added to it automatically"
    )]
    FieldNotAList {
        recipe: String,
        callee: String,
        field: String,
        entry: String,
    },
}

/// A located module call: its byte span and the callee that opens it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleCall {
    /// Byte range of the whole call, e.g. all of `cook_cc.bin({ ... })`.
    pub span: Range<usize>,
    /// The dotted callee, e.g. `cook_cc.bin`. Empty if the call text does not
    /// open with one (the grammar admits the node before we inspect it).
    pub callee: String,
}

fn parse(source: &str) -> Result<Tree, EditError> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_cook::LANGUAGE.into())
        .expect("load Cook grammar");
    let tree = parser.parse(source, None).ok_or(EditError::Unparseable)?;
    if tree.root_node().has_error() {
        return Err(EditError::Unparseable);
    }
    Ok(tree)
}

/// Depth-first walk yielding every node of `kind` within `within`.
fn nodes_of_kind<'t>(root: Node<'t>, kind: &str, out: &mut Vec<Node<'t>>) {
    if root.kind() == kind {
        out.push(root);
    }
    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        nodes_of_kind(child, kind, out);
    }
}

/// Byte range of the `recipe` node named `recipe`, header included.
fn locate_recipe(tree: &Tree, source: &str, recipe: &str) -> Option<Range<usize>> {
    let mut recipes = Vec::new();
    nodes_of_kind(tree.root_node(), "recipe", &mut recipes);
    for node in recipes {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() != "explicit_recipe_header" {
                continue;
            }
            let Some(name_node) = child.child_by_field_name("name") else {
                continue;
            };
            // A declaration name is either bare or double-quoted; the quoted
            // form is an exact equivalent, so compare on the unquoted text.
            let raw = &source[name_node.byte_range()];
            let name = raw.trim().trim_matches('"');
            if name == recipe {
                return Some(node.byte_range());
            }
        }
    }
    None
}

/// The first module call inside `within`.
///
/// "First" is the right rule for the `cc.*` verbs: a target maker is a step
/// contributor deriving its identity from the enclosing recipe
/// (`cook.recipe_name()`), so a recipe that registers a target holds exactly
/// one such call. A recipe holding several is not a target recipe, and
/// editing its first call is no more arbitrary than any other choice — the
/// caller gets the callee back and can reject what it did not expect.
fn locate_call_within<'t>(tree: &'t Tree, source: &str, within: Range<usize>) -> Option<ModuleCall> {
    let mut calls = Vec::new();
    nodes_of_kind(tree.root_node(), "module_call_text", &mut calls);
    for node in calls {
        let span = node.byte_range();
        if span.start < within.start || span.end > within.end {
            continue;
        }
        let text = &source[span.clone()];
        let callee = text
            .split(['(', ' ', '\t', '\n'])
            .next()
            .unwrap_or("")
            .to_string();
        return Some(ModuleCall { span, callee });
    }
    None
}

/// Find `field`'s `{ ... }` value inside an already-located call span.
///
/// Returns the byte range of the braces' interior, exclusive of both braces.
/// Scoped to the call span by the caller, so the scan cannot run off into a
/// neighbouring construct.
///
/// Brace matching is quote- and comment-aware: a `}` inside a Lua string
/// literal or a `--` comment does not close the list. It is the reason this is
/// a scan rather than a `find('}')`. `sources = { "a}b.c" }` is rare but
/// legal, and a comment mentioning a brace inside a multi-line list is not
/// rare at all — this layer exists to preserve comments, so miscounting on one
/// would be a particularly poor failure.
fn locate_field_interior(call: &str, field: &str) -> Option<Result<Range<usize>, ()>> {
    let at = find_field_key(call, field)?;

    // Step past `field` and its `=`.
    let rest = &call[at + field.len()..];
    let eq = rest.find('=')?;
    let after_eq = at + field.len() + eq + 1;

    // The value must open with `{` to be a list we can append to.
    let value_start = after_eq + call[after_eq..].len() - call[after_eq..].trim_start().len();
    if !call[value_start..].starts_with('{') {
        return Some(Err(()));
    }

    let open = value_start;
    let mut depth = 0usize;
    let mut in_string: Option<char> = None;
    let mut escaped = false;
    let mut in_comment = false;
    let mut prev_dash = false;
    for (i, ch) in call[open..].char_indices() {
        if in_comment {
            // A `--` comment runs to end of line. Long-bracket comments
            // (`--[[ ... ]]`) are not handled: they cannot appear in a
            // single-line field value, and a multi-line one would have to sit
            // inside the list to matter. If that ever shows up, the depth
            // count fails closed — the field reads as unterminated and the
            // caller gets FieldNotAList rather than a bad splice.
            if ch == '\n' {
                in_comment = false;
            }
            continue;
        }
        if let Some(quote) = in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == quote {
                in_string = None;
            }
            continue;
        }
        if ch == '-' {
            if prev_dash {
                in_comment = true;
                prev_dash = false;
                continue;
            }
            prev_dash = true;
            continue;
        }
        prev_dash = false;
        match ch {
            '"' | '\'' => in_string = Some(ch),
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(Ok(open + 1..open + i));
                }
            }
            _ => {}
        }
    }
    Some(Err(()))
}

/// Locate `field` as a table KEY, not as a substring.
///
/// `call.find("links")` would match the `links` inside `"mathlinks"` or a
/// comment, and splice into it. A key is preceded by a delimiter and followed
/// by optional whitespace then `=`.
fn find_field_key(call: &str, field: &str) -> Option<usize> {
    let bytes = call.as_bytes();
    let mut from = 0usize;
    while let Some(rel) = call[from..].find(field) {
        let at = from + rel;
        from = at + field.len();

        let before_ok = at == 0
            || !(bytes[at - 1].is_ascii_alphanumeric() || bytes[at - 1] == b'_');
        let after = &call[at + field.len()..];
        let after_ok = after.trim_start().starts_with('=');
        if before_ok && after_ok {
            return Some(at);
        }
    }
    None
}

/// Offset just past the last byte of code in a list's interior, skipping
/// trailing whitespace and any `--` comments.
///
/// `str::trim_end` is not enough. In
///
/// ```text
/// links = {
///     "mathlib",   -- see docs/build.md {section 2}
/// }
/// ```
///
/// the last non-whitespace byte is the `}` of `{section 2}`, so anchoring
/// there splices the new entry into the middle of the author's comment. The
/// comment is exactly what this layer exists to preserve, which makes that a
/// particularly bad way to be wrong.
///
/// Returns `None` for an interior holding no code at all (empty, or only
/// comments).
fn last_code_end(inner: &str) -> Option<usize> {
    let mut last: Option<usize> = None;
    let mut in_string: Option<char> = None;
    let mut escaped = false;
    let mut in_comment = false;
    let mut prev_dash = false;
    // `last` as it stood before the first `-` of a possible `--` was
    // provisionally counted as code. A single `-` is legal Lua (`n-1`), so it
    // has to count until a second one proves it was a comment opener; this is
    // what makes that retraction exact rather than recomputed.
    let mut last_before_dash: Option<usize> = None;

    for (i, ch) in inner.char_indices() {
        if in_comment {
            if ch == '\n' {
                in_comment = false;
            }
            continue;
        }
        if let Some(quote) = in_string {
            last = Some(i + ch.len_utf8());
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == quote {
                in_string = None;
            }
            continue;
        }
        if ch == '-' {
            if prev_dash {
                in_comment = true;
                prev_dash = false;
                last = last_before_dash;
                continue;
            }
            prev_dash = true;
            last_before_dash = last;
            last = Some(i + 1);
            continue;
        }
        prev_dash = false;
        if ch == '"' || ch == '\'' {
            in_string = Some(ch);
        }
        if !ch.is_whitespace() {
            last = Some(i + ch.len_utf8());
        }
    }
    last
}

/// Splice `entry` into `field`'s list, in the module call inside `recipe`.
///
/// Returns the edited source. Everything outside the inserted bytes is
/// byte-identical to the input — comments, spacing, and non-literal Lua alike.
///
/// `entry` is inserted verbatim, so the caller renders its own quoting. The
/// insert is anchored to the last non-whitespace byte before the list's close,
/// which keeps the author's interior padding where they put it: inserting
/// immediately before `}` turns `{ "a" }` into `{ "a", "b" }` rather than
/// `{ "a", "b"}`.
pub fn splice_into_field(
    source: &str,
    recipe: &str,
    field: &str,
    entry: &str,
) -> Result<String, EditError> {
    let tree = parse(source)?;
    let recipe_span = locate_recipe(&tree, source, recipe).ok_or_else(|| {
        EditError::RecipeNotFound {
            recipe: recipe.to_string(),
        }
    })?;
    let call = locate_call_within(&tree, source, recipe_span).ok_or_else(|| {
        EditError::NoModuleCall {
            recipe: recipe.to_string(),
        }
    })?;

    let call_text = &source[call.span.clone()];
    let interior = match locate_field_interior(call_text, field) {
        None => {
            return Err(EditError::FieldNotFound {
                recipe: recipe.to_string(),
                callee: call.callee,
                field: field.to_string(),
                entry: entry.to_string(),
            })
        }
        Some(Err(())) => {
            return Err(EditError::FieldNotAList {
                recipe: recipe.to_string(),
                callee: call.callee,
                field: field.to_string(),
                entry: entry.to_string(),
            })
        }
        Some(Ok(range)) => range,
    };

    let inner = &call_text[interior.clone()];

    // Anchor after the last byte of actual CODE in the list — not merely the
    // last non-whitespace byte, which may sit inside a trailing comment and
    // would splice the entry into the comment text.
    let (anchor_in_call, insertion) = match last_code_end(inner) {
        None => (interior.start, entry.to_string()),
        Some(end) => {
            // A trailing comma is already the separator, so adding another
            // would produce `{ "a",, "b" }`.
            let sep = if inner[..end].trim_end().ends_with(',') {
                format!(" {entry}")
            } else {
                format!(", {entry}")
            };
            (interior.start + end, sep)
        }
    };
    let at = call.span.start + anchor_in_call;

    let mut edited = String::with_capacity(source.len() + insertion.len());
    edited.push_str(&source[..at]);
    edited.push_str(&insertion);
    edited.push_str(&source[at..]);
    Ok(edited)
}

/// Append `text` at end of file, guaranteeing exactly one blank line before it
/// and a trailing newline after.
///
/// The scaffolding verbs use this before any field splicing exists to edit:
/// `cc.add` writes a whole new `recipe` declaration, which has no enclosing
/// structure to preserve.
pub fn append_declaration(source: &str, text: &str) -> String {
    let trimmed = source.trim_end();
    let body = text.trim_end();
    if trimmed.is_empty() {
        return format!("{body}\n");
    }
    format!("{trimmed}\n\n{body}\n")
}

/// Locate the module call in `recipe`, for a caller that wants to inspect
/// before editing.
pub fn find_call(source: &str, recipe: &str) -> Result<ModuleCall, EditError> {
    let tree = parse(source)?;
    let recipe_span = locate_recipe(&tree, source, recipe).ok_or_else(|| {
        EditError::RecipeNotFound {
            recipe: recipe.to_string(),
        }
    })?;
    locate_call_within(&tree, source, recipe_span).ok_or_else(|| EditError::NoModuleCall {
        recipe: recipe.to_string(),
    })
}
