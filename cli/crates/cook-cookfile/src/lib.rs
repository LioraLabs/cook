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

use cook_contracts::lua_scan::{is_ident_cont, opens_comment, skip_non_code, Skip};
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

/// What a byte range of a Lua fragment is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Region {
    /// Ordinary code. A brace here nests the table and an `=` here binds a key.
    Code,
    /// One whole string literal, in any of Lua's four spellings.
    Str,
    /// One whole comment, `-- …` to end of line or `--[==[ … ]==]` across them.
    Comment,
}

/// Split `src` into lexical regions, in order and covering every byte.
///
/// This is the crate's one answer to "where does a Lua string or comment begin
/// and end", and it is not answered here: [`cook_contracts::lua_scan`] owns it,
/// because `cook-luagen` asks the same question of the Lua it lowers. Three
/// questions below consume this — which `}` closes the list, which `links` is
/// the field, and where the last byte of code is — and they used to be three
/// state machines at three fidelities, which is how a `[[a}b]]` entry came to
/// be spliced into (COOK-403, CS-0208).
fn regions(src: &str) -> Vec<(Region, Range<usize>)> {
    let bytes = src.as_bytes();
    let mut out: Vec<(Region, Range<usize>)> = Vec::new();
    let mut code_from = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        let kind = if opens_comment(bytes, i) {
            Region::Comment
        } else {
            Region::Str
        };
        let end = match skip_non_code(src, i) {
            Skip::Code => {
                i += 1;
                continue;
            }
            Skip::Ended(end) => end.max(i + 1),
            // Past a literal nobody closed there is no honest reading, so the
            // rest of the fragment is that literal. Every caller then fails to
            // find what it was looking for and the edit is refused, which is
            // the correct answer to a file the author has already broken.
            Skip::Unterminated => bytes.len(),
        };
        if code_from < i {
            out.push((Region::Code, code_from..i));
        }
        out.push((kind, i..end));
        i = end;
        code_from = end;
    }
    if code_from < bytes.len() {
        out.push((Region::Code, code_from..bytes.len()));
    }
    out
}

/// A string that stands in for a whole literal, and a space that stands in for
/// a whole comment.
///
/// Both are bytes that can neither continue an identifier nor nest a table, so
/// a name written across one — `link--x\ns` — cannot join up into `links`.
const STRING_ATOM: u8 = b'"';
const COMMENT_ATOM: u8 = b' ';

/// The call text as the grammar-relevant bytes alone: comments removed, string
/// literals collapsed to one opaque byte, and every remaining byte paired with
/// the offset it came from.
///
/// Nesting depth, key positions and the `=` that binds a key are all decided
/// over this view, which is what makes each of them blind by construction to a
/// brace, a field name or an `=` the author wrote inside a literal or a
/// comment. The offsets are the original ones, so anything found here can be
/// spliced without translating back.
struct CodeView {
    bytes: Vec<u8>,
    at: Vec<usize>,
}

impl CodeView {
    fn of(src: &str) -> Self {
        let raw = src.as_bytes();
        let mut view = CodeView {
            bytes: Vec::new(),
            at: Vec::new(),
        };
        for (kind, range) in regions(src) {
            match kind {
                Region::Code => {
                    for i in range {
                        view.bytes.push(raw[i]);
                        view.at.push(i);
                    }
                }
                Region::Str => {
                    view.bytes.push(STRING_ATOM);
                    view.at.push(range.start);
                }
                Region::Comment => {
                    view.bytes.push(COMMENT_ATOM);
                    view.at.push(range.start);
                }
            }
        }
        view
    }

    /// Index of the next non-whitespace byte at or after `from`.
    fn significant_from(&self, from: usize) -> Option<usize> {
        (from..self.bytes.len()).find(|&i| !self.bytes[i].is_ascii_whitespace())
    }

    /// Index of the last non-whitespace byte before `before`.
    fn significant_before(&self, before: usize) -> Option<usize> {
        (0..before).rev().find(|&i| !self.bytes[i].is_ascii_whitespace())
    }
}

/// Find `field`'s `{ ... }` value inside an already-located call span.
///
/// Returns the byte range of the braces' interior, exclusive of both braces.
/// Scoped to the call span by the caller, so the scan cannot run off into a
/// neighbouring construct.
///
/// `field` is matched as a table KEY at the top level of the call's argument,
/// never as a substring. `call.find("links")` would match inside
/// `"mathlinks.cpp"`, inside a commented-out `-- links = { "old" }`, or inside
/// the nested `opts = { links = … }` of a different table, and splice into
/// whichever came first. Each is a way of editing bytes the author never
/// pointed at, and the nested one is the worst, because the file still looks
/// right afterwards (§22.13, CS-0208).
///
/// Brace matching is blind to strings and comments for the same reason: a `}`
/// inside `"src/a}b.cpp"` or `[[a}b]]` or a comment does not close the list.
fn locate_field_interior(call: &str, field: &str) -> Option<Result<Range<usize>, ()>> {
    let view = CodeView::of(call);
    let eq = find_field_key(&view, field)?;

    // The value is whatever follows the `=`. It must open with `{` to be a
    // list an entry can be added to.
    let after_eq = view.significant_from(eq + 1);
    let open = match after_eq {
        Some(i) if view.bytes[i] == b'{' => i,
        _ => return Some(Err(())),
    };

    let mut depth = 0usize;
    for i in open..view.bytes.len() {
        match view.bytes[i] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(Ok(view.at[open] + 1..view.at[i]));
                }
            }
            _ => {}
        }
    }
    // Unterminated: the author's file, or a literal this scan could not close.
    // Either way the honest answer is that no list was found here.
    Some(Err(()))
}

/// Locate `field` as a table key at the top level of the call's argument, and
/// return the [`CodeView`] index of the `=` that binds it.
///
/// A key qualifies when all four hold: it is whole (no identifier byte on
/// either side), it sits at brace depth 1 — inside the call's own table and
/// not a table nested in it — the previous significant byte opens or separates
/// a table entry, and the next one is a lone `=` rather than the `==` of a
/// comparison.
fn find_field_key(view: &CodeView, field: &str) -> Option<usize> {
    // An empty name matches at every position, which is a way of editing an
    // arbitrary field rather than of finding none.
    if field.is_empty() {
        return None;
    }
    let needle = field.as_bytes();
    let mut depth = 0usize;
    for i in 0..view.bytes.len() {
        match view.bytes[i] {
            b'{' => {
                depth += 1;
                continue;
            }
            b'}' => {
                depth = depth.saturating_sub(1);
                continue;
            }
            _ => {}
        }
        if depth != 1 || !view.bytes[i..].starts_with(needle) {
            continue;
        }
        let after = i + needle.len();
        if after < view.bytes.len() && is_ident_cont(view.bytes[after]) {
            continue;
        }
        let opens_entry = match view.significant_before(i) {
            Some(p) => matches!(view.bytes[p], b'{' | b',' | b';'),
            None => false,
        };
        if !opens_entry {
            continue;
        }
        let Some(eq) = view.significant_from(after) else {
            continue;
        };
        if view.bytes[eq] != b'=' || view.bytes.get(eq + 1) == Some(&b'=') {
            continue;
        }
        return Some(eq);
    }
    None
}

/// Offset just past the last byte of code in a list's interior, skipping
/// trailing whitespace and any comment.
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
/// A string literal counts as code, and counts to its last byte: a list ending
/// `[[note -- x]]` anchors after the closing bracket, not at the `--` inside
/// it, where a comment-aware scan that cannot see long brackets would put it.
///
/// Returns `None` for an interior holding no code at all (empty, or only
/// comments).
fn last_code_end(inner: &str) -> Option<usize> {
    let mut last: Option<usize> = None;
    for (kind, range) in regions(inner) {
        match kind {
            Region::Comment => {}
            Region::Str => last = Some(range.end),
            Region::Code => {
                for (i, ch) in inner[range.clone()].char_indices() {
                    if !ch.is_whitespace() {
                        last = Some(range.start + i + ch.len_utf8());
                    }
                }
            }
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

/// What [`ensure_use`] did.
///
/// The two are distinguished rather than collapsed into a `String` because the
/// caller writes the file. Handing back the source unchanged would leave it
/// with no way to tell "already correct" from "edited", so it would write
/// either way: an mtime bump, a rebuild of everything downstream of the
/// Cookfile, and a line in `git status` for an edit nobody made. `ensure_` is
/// only honestly named if the second run is free.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UseEdit {
    /// The declaration is already there; the file is not rewritten.
    AlreadyPresent,
    /// The edited source, with exactly the inserted bytes added.
    Inserted(String),
}

/// The module a `use_declaration` binds by name, unquoted.
///
/// `None` for the path form `use ALIAS "./x.lua"`, whose identifier the
/// grammar records as `alias` and not as `module` (`grammar.js`,
/// `use_declaration`). The distinction matters: the path form binds that name
/// to a file the author chose, so treating it as `use cook_cc` would report a
/// module nobody named and skip the declaration that actually loads it.
fn declared_module<'s>(node: Node<'_>, source: &'s str) -> Option<&'s str> {
    let field = node.child_by_field_name("module")?;
    // Bare or double-quoted, exactly as recipe names are; the quoted form is
    // an exact equivalent, so compare on the unquoted text.
    Some(source[field.byte_range()].trim().trim_matches('"'))
}

/// Offset just past the newline terminating the line that `end` closes.
///
/// Node spans disagree about the newline and both spellings occur among the
/// nodes the `use` run walks: a `use_declaration` swallows its terminator (the
/// grammar requires one), while a `comment` stops at the last byte of its
/// text. Scanning forward unconditionally would step over an entire extra line
/// in the first case, which is how an insert lands below the recipe header it
/// was meant to precede.
fn past_line_end(source: &str, end: usize) -> usize {
    if end > 0 && source.as_bytes()[end - 1] == b'\n' {
        return end;
    }
    match source[end..].find('\n') {
        Some(i) => end + i + 1,
        None => source.len(),
    }
}

/// Whether a blank line separates `from` — always the start of a line — from
/// the node beginning at `to`.
///
/// `from` is `past_line_end`'s output, so it sits immediately after a newline
/// or at offset 0; any newline in the gap is therefore a line with nothing on
/// it. Blank lines are the hidden `_newline` rule and never appear among named
/// children, so a walk over the tree alone cannot see the one thing that tells
/// a file's header apart from a comment introducing the next declaration.
fn opens_a_new_block(source: &str, from: usize, to: usize) -> bool {
    source[from..to].contains('\n')
}

/// Add `use <module>` to the file unless a top-level `use` already binds it.
///
/// Returns [`UseEdit::AlreadyPresent`] when it does, and otherwise the edited
/// source: everything outside the inserted run is byte-identical, per §22.13.
///
/// # Presence is structural
///
/// A match is a top-level `use_declaration` node whose `module` field names
/// `module`, so `use cook_cc` and `use "cook_cc"` both count and the path form
/// `use cook_cc "./vendor/cc.lua"` does not. `source.contains("use cook_cc")`
/// would agree with all three and would also agree with a `use cook_cc` line
/// written inside a step body, where the text is shell content and binds
/// nothing. That last one is the case worth the parser: the chore reports the
/// module available, writes a call to it, and the Cookfile fails to load.
///
/// # Where the line goes
///
/// After the file's leading comment block, then after any `use` declarations
/// following it; at offset 0 when there are neither. The opening comment block
/// of a Cookfile is its header — what this file builds, who owns it, what it
/// is licensed under — and burying that under machinery is a worse edit than
/// the one being asked for. Only a *contiguous* leading run is skipped, so a
/// `use` the author put below a recipe is not joined; it is still found by the
/// presence check above, which reads the whole file.
///
/// A blank line ends the header and does not end the `use` run, which is not
/// an inconsistency: it is what a blank line means in each position. A comment
/// under one is attached to what follows it —
///
/// ```text
/// # the project
///
/// # build the game
/// recipe game
/// ```
///
/// — so continuing the header through it would put the declaration between
/// that comment and the recipe it describes, which is the same class of damage
/// as splicing into a comment (§22.13). A blank line between `use` lines
/// separates nothing in the same way: they are one group however the author
/// spaced them, and joining the group is what keeps a second install from
/// landing above the first.
///
/// The inserted text is `use <module>\n`, plus a blank line when the line it
/// lands above is neither empty nor absent — `use cook_cc` flush against
/// `recipe app` is not how anyone writes a Cookfile, and a blank line at end
/// of file is trailing whitespace nobody asked for. The one other adjustment
/// is a leading newline when the file does not end in one and the insert goes
/// at its end: without it the author's unterminated last line swallows the
/// declaration.
///
/// # What it does not check
///
/// `module` is inserted verbatim and is NOT validated as a LUA_IDENT. The
/// grammar constrains a `use` name to a strict Lua identifier (CS-0035)
/// because the name is bound as a Lua local under that spelling, but the
/// refusal belongs to the caller that knows what it is naming and can say so
/// in its own terms. This layer stays a pure editor, as it does for `entry`
/// and `text` (§22.13, "entries are rendered by the caller").
pub fn ensure_use(source: &str, module: &str) -> Result<UseEdit, EditError> {
    let tree = parse(source)?;
    let root = tree.root_node();

    let mut uses = Vec::new();
    nodes_of_kind(root, "use_declaration", &mut uses);
    if uses
        .iter()
        .any(|node| declared_module(*node, source) == Some(module))
    {
        return Ok(UseEdit::AlreadyPresent);
    }

    // The leading run: the header comment block, then the `use` declarations
    // after it. A blank line closes the header (`opens_a_new_block`) and does
    // not close the `use` group, per the rule argued above.
    let mut at = 0usize;
    let mut in_header = true;
    let mut cursor = root.walk();
    for child in root.named_children(&mut cursor) {
        match child.kind() {
            "comment" if in_header && !opens_a_new_block(source, at, child.start_byte()) => {
                at = past_line_end(source, child.end_byte());
            }
            "use_declaration" => {
                in_header = false;
                at = past_line_end(source, child.end_byte());
            }
            _ => break,
        }
    }

    let mut insertion = format!("use {module}\n");
    match source.as_bytes().get(at) {
        // A blank line already separates the run from what follows.
        Some(b'\n') => {}
        Some(_) => insertion.push('\n'),
        // End of file. `past_line_end` returns an offset that either follows a
        // newline or is the file's end, so an unterminated final line can only
        // show up here, and only here does the insert need to open one.
        None => {
            if !source.is_empty() && !source.ends_with('\n') {
                insertion.insert(0, '\n');
            }
        }
    }

    let mut edited = String::with_capacity(source.len() + insertion.len());
    edited.push_str(&source[..at]);
    edited.push_str(&insertion);
    edited.push_str(&source[at..]);
    Ok(UseEdit::Inserted(edited))
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
