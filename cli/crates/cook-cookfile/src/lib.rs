//! Structure-preserving Cookfile edits (Standard §22.13, CS-0179).
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

use cook_contracts::lua_scan::{
    is_ident_cont, is_ident_start, is_reserved_word, opens_comment, skip_non_code, Skip,
};
use cook_contracts::module_binding::{alias_of, derived_alias};
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

    #[error("'{field}' in the {callee} call in recipe '{recipe}' is not a `{{ ... }}` list")]
    FieldNotAList {
        recipe: String,
        callee: String,
        field: String,
    },

    #[error(
        "the {callee} call in recipe '{recipe}' has no `{{ ... }}` argument table, so '{field}' \
         cannot be added to it — add {field} = {{ {entry} }} to it manually"
    )]
    NoArgumentTable {
        recipe: String,
        callee: String,
        field: String,
        entry: String,
    },

    #[error(
        "the {callee} call in recipe '{recipe}' writes '{field}' as [\"{field}\"], which is the \
         same key and is not a spelling this edit can add to — add {entry} to it manually"
    )]
    BracketedField {
        recipe: String,
        callee: String,
        field: String,
        entry: String,
    },

    #[error(
        "'{field}' cannot be written as a table key, so it cannot be created — a field name is a \
         Lua identifier and not a reserved word"
    )]
    UnspellableField { field: String },
}

/// What to do when the field an edit names is not there (CS-0221).
///
/// [`AbsentField::Refuse`] is §22.13's original and still-default behaviour,
/// and it is the reason this is a parameter rather than a flag someone might
/// forget: total failure is a property callers depend on, so every call site
/// has to say which of the two it wants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbsentField {
    /// Fail with [`EditError::FieldNotFound`], changing nothing.
    Refuse,
    /// Write `field = { entry }` as one more entry of the call's argument
    /// table.
    Create,
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
fn locate_call_within<'t>(
    tree: &'t Tree,
    source: &str,
    within: Range<usize>,
) -> Option<ModuleCall> {
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
        (0..before)
            .rev()
            .find(|&i| !self.bytes[i].is_ascii_whitespace())
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

/// Offset of the first byte of code in a fragment, skipping leading whitespace
/// and any comment. The mirror of [`last_code_end`], and used for the same
/// reason: an entry written as `-- keep this\n"math"` starts at the quote.
fn first_code_start(inner: &str) -> Option<usize> {
    for (kind, range) in regions(inner) {
        match kind {
            Region::Comment => {}
            Region::Str => return Some(range.start),
            Region::Code => {
                for (i, ch) in inner[range.clone()].char_indices() {
                    if !ch.is_whitespace() {
                        return Some(range.start + i);
                    }
                }
            }
        }
    }
    None
}

/// Whether a code byte separates two entries of a table constructor.
///
/// Lua admits both, and [`find_field_key`] has always accepted either as the
/// thing that opens the entry it is looking at. Only the write side treated
/// `,` as the whole story, which is how appending after `{ "a"; }` came to
/// produce `{ "a";, "b" }` — a file that no longer loads.
fn is_entry_separator(b: u8) -> bool {
    b == b',' || b == b';'
}

/// The separator the code up to `end` already ends in, if any.
fn trailing_separator(code: &str) -> Option<char> {
    let last = code.trim_end().chars().next_back()?;
    is_entry_separator(last as u8).then_some(last)
}

/// The offset of the line break that ends the line the code at `from` sits on,
/// paired with the terminator that line uses.
///
/// "The next `\n`" is the wrong answer and is wrong in the damaging direction.
/// Where the author's last entry carries a `--[[ note` comment running over
/// several lines, the next `\n` is INSIDE that comment, and a field inserted
/// there lands inside it: the call is unchanged, the edit reports success, and
/// a second run stacks another line in the same comment. So the search runs
/// over [`regions`] and only a newline in a code region ends a line.
///
/// The terminator comes back with it so a CRLF file keeps its convention.
/// Inserting a bare `\n` line into one is a change to the file's line endings
/// that nobody asked for, which is the kind of unannounced restyle §22.13
/// exists to forbid.
fn line_end_after(inner: &str, from: usize) -> (usize, &'static str) {
    for (kind, range) in regions(inner) {
        if kind != Region::Code || range.end <= from {
            continue;
        }
        let start = range.start.max(from);
        if let Some(i) = inner[start..range.end].find('\n') {
            let at = start + i;
            if at > 0 && inner.as_bytes()[at - 1] == b'\r' {
                return (at - 1, "\r\n");
            }
            return (at, "\n");
        }
    }
    (inner.len(), "\n")
}

/// The interior of the `{ ... }` a module call passes as its argument.
///
/// The same brace-matching the field locator does, one level out: the first
/// `{` of the call and its match. Located by brace rather than by paren so
/// that `f{ ... }`, Lua's sugar for `f({ ... })`, is the same shape here that
/// it is to an evaluator — and `find_field_key`'s depth-1 rule already means
/// the same table either way.
///
/// `None` for a call passing no table at all, which is the one case where a
/// created field has nowhere to go.
fn locate_argument_table(call: &str) -> Option<Range<usize>> {
    let view = CodeView::of(call);
    let open = view.bytes.iter().position(|&b| b == b'{')?;
    let mut depth = 0usize;
    for i in open..view.bytes.len() {
        match view.bytes[i] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(view.at[open] + 1..view.at[i]);
                }
            }
            _ => {}
        }
    }
    None
}

/// Whether the call's argument table already binds `field` under the bracketed
/// spelling `["field"] = …`.
///
/// §22.13 leaves that spelling outside what [`find_field_key`] must match, and
/// answering "not found" is a fine reply when the consequence is a refusal. It
/// stops being a fine reply when the consequence is writing a field: `["links"]`
/// and `links` are one key to an evaluator, so creating the second silently
/// discards the author's first. So this question is asked only on the create
/// path, and only to refuse.
///
/// Short strings only. A long-bracket key is not a spelling anyone writes, and
/// deciding whether `["li\nks"]` names `links` would mean unescaping, which is
/// evaluating the author's Lua by another name.
fn bracketed_key_present(call: &str, field: &str) -> bool {
    let view = CodeView::of(call);
    let literals: Vec<Range<usize>> = regions(call)
        .into_iter()
        .filter(|(kind, _)| *kind == Region::Str)
        .map(|(_, range)| range)
        .collect();

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
        if depth != 1 || view.bytes[i] != STRING_ATOM {
            continue;
        }
        let bracketed = view
            .significant_before(i)
            .is_some_and(|p| view.bytes[p] == b'[')
            && view
                .significant_from(i + 1)
                .and_then(|close| {
                    (view.bytes[close] == b']').then(|| view.significant_from(close + 1))?
                })
                .is_some_and(|eq| view.bytes[eq] == b'=' && view.bytes.get(eq + 1) != Some(&b'='));
        if !bracketed {
            continue;
        }
        let Some(range) = literals.iter().find(|r| r.start == view.at[i]) else {
            continue;
        };
        let raw = &call[range.clone()];
        let unquoted = raw
            .strip_prefix('"')
            .and_then(|s| s.strip_suffix('"'))
            .or_else(|| raw.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')));
        if unquoted == Some(field) {
            return true;
        }
    }
    false
}

/// One insertion: the offset it lands at, and the bytes that land there.
///
/// Creating a field can need two of these, so the shape is a list rather than
/// a pair.
type Insertion = (usize, String);

/// Apply insertions given in document order, at offsets relative to `at_base`.
///
/// Back to front, which is the whole reason the offsets never need adjusting
/// for each other. Reversing the caller's order rather than sorting by offset
/// is what makes two insertions AT THE SAME offset land in the order they were
/// written: sorting leaves that tie to the sort's stability and then inserts
/// the first-listed one first, which puts it after the second — a comma
/// written before a new line ends up after it.
fn apply(source: &str, at_base: usize, edits: Vec<Insertion>) -> String {
    debug_assert!(
        edits.windows(2).all(|w| w[0].0 <= w[1].0),
        "insertions must be given in document order"
    );
    let mut out = source.to_string();
    for (at, text) in edits.into_iter().rev() {
        out.insert_str(at_base + at, &text);
    }
    out
}

/// Where `field = { entry }` goes in an argument table that has no such field,
/// and how it is spelled, as offsets relative to the table's interior.
///
/// The anchor is [`last_code_end`], exactly as it is for an entry inside a
/// list. What is added is layout matching, and it is not decoration: a call
/// whose entries sit one per line is the common shape a scaffolded Cookfile
/// grows into, and appending `, links = { … }` after the last one puts two
/// fields on a line in a file that has none. §22.13 exists to keep an edit
/// from restyling the author's file, and quietly changing its layout
/// convention is a restyle by another name.
///
/// Two subtleties, both about a trailing comment:
///
/// - In the one-per-line layout the new line goes after the END of the anchor
///   line, not after the last code byte, so a comment the author wrote against
///   the previous field stays against that field rather than migrating onto
///   the new one.
/// - When that previous field carries no trailing comma, the comma still has
///   to go at the code, which is before the comment. Hence two insertions.
fn create_field_edits(inner: &str, field: &str, entry: &str) -> Vec<Insertion> {
    let rendered = format!("{field} = {{ {entry} }}");
    let Some(end) = last_code_end(inner) else {
        // An argument table holding no code takes the field with no separator
        // and no invented padding, which is what an empty LIST already does.
        // `{}` has no layout to preserve, and inventing one would be this
        // layer having an opinion about a file it did not write.
        return vec![(0, rendered)];
    };

    let separator = trailing_separator(&inner[..end]);
    // One field per line is decided on the run up to the anchor: if the
    // author put a newline between the opening brace and the last field, they
    // are writing a block, not a one-liner.
    if !inner[..end].contains('\n') {
        let sep = match separator {
            Some(_) => " ".to_string(),
            None => ", ".to_string(),
        };
        return vec![(end, format!("{sep}{rendered}"))];
    }

    let line_start = inner[..end].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let indent: String = inner[line_start..end]
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect();
    let (line_end, newline) = line_end_after(inner, end);

    // The author's own separator is repeated rather than normalised to a
    // comma: a table written with `;` stays written with `;`.
    match separator {
        Some(sep) => vec![(line_end, format!("{newline}{indent}{rendered}{sep}"))],
        None => vec![
            (end, ",".to_string()),
            (line_end, format!("{newline}{indent}{rendered}")),
        ],
    }
}

/// Whether `field` can be WRITTEN as a bare table key.
///
/// Only the create path asks. Locating a field never needs this — a name that
/// cannot be spelled simply is not found — but writing one does, and the two
/// policies diverging on the same input is exactly the asymmetry to avoid:
/// `find_field_key` already refuses an empty name outright, so without this an
/// empty `field` would fail cleanly under [`AbsentField::Refuse`] and silently
/// write ` = { "math" }` under [`AbsentField::Create`].
///
/// This is the same posture CS-0220 took for a `use` name and the opposite of
/// the one `entry` is under: §22.13 delegates `entry` to the caller because
/// only the caller knows whether it is adding a string, an identifier or a
/// table. `field` admits no such variation — it is a key, in one spelling —
/// so the layer that writes it is the one that can check it.
fn is_writable_key(field: &str) -> bool {
    let mut bytes = field.bytes();
    let ok = matches!(bytes.next(), Some(b) if is_ident_start(b)) && bytes.all(is_ident_cont);
    ok && !is_reserved_word(field)
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
///
/// `absent` decides the one case where the field is not there at all
/// (CS-0221). [`AbsentField::Create`] writes `field = { entry }` into the
/// call's argument table; the caller still renders `entry`, and this layer
/// renders only the frame it was asked to create, so the two cannot come to
/// different opinions about the same entry. Nothing else is relaxed by it: a
/// field that IS there and is not a list, a recipe that does not exist, a
/// recipe holding no module call, and a file that does not parse are refused
/// under either policy, because in each of those the thing to create either
/// already exists or has nowhere to go.
pub fn splice_into_field(
    source: &str,
    recipe: &str,
    field: &str,
    entry: &str,
    absent: AbsentField,
) -> Result<String, EditError> {
    let tree = parse(source)?;
    let recipe_span =
        locate_recipe(&tree, source, recipe).ok_or_else(|| EditError::RecipeNotFound {
            recipe: recipe.to_string(),
        })?;
    let call =
        locate_call_within(&tree, source, recipe_span).ok_or_else(|| EditError::NoModuleCall {
            recipe: recipe.to_string(),
        })?;

    let call_text = &source[call.span.clone()];
    let interior = match locate_field_interior(call_text, field) {
        None if absent == AbsentField::Create => {
            if !is_writable_key(field) {
                return Err(EditError::UnspellableField {
                    field: field.to_string(),
                });
            }
            if bracketed_key_present(call_text, field) {
                return Err(EditError::BracketedField {
                    recipe: recipe.to_string(),
                    callee: call.callee,
                    field: field.to_string(),
                    entry: entry.to_string(),
                });
            }
            let table =
                locate_argument_table(call_text).ok_or_else(|| EditError::NoArgumentTable {
                    recipe: recipe.to_string(),
                    callee: call.callee.clone(),
                    field: field.to_string(),
                    entry: entry.to_string(),
                })?;
            let edits = create_field_edits(&call_text[table.clone()], field, entry);
            return Ok(apply(source, call.span.start + table.start, edits));
        }
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
            // A trailing separator is already the separator, so adding another
            // would produce `{ "a",, "b" }` — or, for the `;` spelling Lua
            // equally admits, the `{ "a";, "b" }` that does not load at all.
            let sep = match trailing_separator(&inner[..end]) {
                Some(_) => format!(" {entry}"),
                None => format!(", {entry}"),
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

/// The entries of `field`'s list in the module call inside `recipe`, as
/// written (CS-0221).
///
/// `Ok(None)` where there is nothing to read — no such recipe, no module call
/// in it, or no such field — which is [`find_call`]'s rule and the same
/// question [`AbsentField::Create`] answers by writing. A field that IS there
/// and is not a list is an error rather than `None`, because a caller told
/// "nothing here" would go on to create a second key of that name.
///
/// The entries come back verbatim and in order, trimmed of surrounding
/// whitespace and comments but otherwise untouched: `"math"` keeps its quotes,
/// because the caller compares against the entry it would pass to
/// [`splice_into_field`], and that is also written verbatim.
///
/// # Why this is a blessed read
///
/// The question it answers — is `math` already in `links` — has an obvious
/// wrong implementation in every consuming module: `call.text:find('"math"')`
/// matches inside `sources = { "src/math/main.cpp" }`. Answering it correctly
/// means locating the field as a top-level key and splitting a list without
/// tripping over a comma inside a nested table, a call, a string or a comment;
/// that is this crate's whole subject, and two modules writing it again in Lua
/// is the second opinion about the hard part that §22.13 exists to prevent.
pub fn field_entries(
    source: &str,
    recipe: &str,
    field: &str,
) -> Result<Option<Vec<String>>, EditError> {
    let tree = parse(source)?;
    let Some(recipe_span) = locate_recipe(&tree, source, recipe) else {
        return Ok(None);
    };
    let Some(call) = locate_call_within(&tree, source, recipe_span) else {
        return Ok(None);
    };
    let call_text = &source[call.span.clone()];
    match locate_field_interior(call_text, field) {
        None => Ok(None),
        Some(Err(())) => Err(EditError::FieldNotAList {
            recipe: recipe.to_string(),
            callee: call.callee,
            field: field.to_string(),
        }),
        Some(Ok(range)) => Ok(Some(split_entries(&call_text[range]))),
    }
}

/// Split a list's interior into its entries.
///
/// A comma separates entries only at the interior's own nesting level: the one
/// in `{ "a", "b" }` and the one in `f(1, 2)` belong to a table and a call
/// written *inside* an entry, and splitting on them would hand back two halves
/// of one thing. Brackets of all three kinds are counted, over the
/// [`CodeView`], so a comma inside a string or a comment is not a separator
/// either.
fn split_entries(inner: &str) -> Vec<String> {
    let view = CodeView::of(inner);
    let mut cuts: Vec<usize> = Vec::new();
    let mut depth = 0usize;
    for i in 0..view.bytes.len() {
        match view.bytes[i] {
            b'{' | b'(' | b'[' => depth += 1,
            b'}' | b')' | b']' => depth = depth.saturating_sub(1),
            b if is_entry_separator(b) && depth == 0 => cuts.push(view.at[i]),
            _ => {}
        }
    }

    let mut out = Vec::new();
    let mut from = 0usize;
    for cut in cuts.into_iter().chain(std::iter::once(inner.len())) {
        let slice = &inner[from..cut];
        from = cut + 1;
        // An empty run is the author's trailing comma, or a comment-only line
        // between two entries. Neither is an entry.
        if let (Some(start), Some(end)) = (first_code_start(slice), last_code_end(slice)) {
            out.push(slice[start..end].to_string());
        }
    }
    out
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

/// The Lua local a `use_declaration` binds, in any of its three spellings.
///
/// The question is the ALIAS and not the module's identity, because that is
/// what a second declaration would collide with. All three of `use cook_cc`,
/// `use cook_cc ./vendor/cc.lua` and `use ./cook_cc.lua` bind `cook_cc`
/// (§12.1), and adding a fourth line binding it again is not an addition: the
/// two phases pick different winners — the register chunk emits one `local`
/// per declaration in order, so the last wins, while the execute prelude skips
/// an alias it has already bound, so the first does. A build whose two phases
/// silently load different module code is the failure mode this crate exists
/// to avoid, and §27.1.2 makes the path form the sanctioned way to patch a
/// blessed module, so it is a shape a `cook modules install` will meet.
///
/// `None` only for a node the grammar admits with neither field.
fn bound_alias(node: Node<'_>, source: &str) -> Option<String> {
    // Bare or double-quoted, exactly as recipe names are; the quoted form is
    // an exact equivalent, so compare on the unquoted text.
    let text = |field: Node<'_>| source[field.byte_range()].trim().trim_matches('"');
    if let Some(module) = node.child_by_field_name("module") {
        return Some(alias_of(text(module)));
    }
    if let Some(alias) = node.child_by_field_name("alias") {
        return Some(alias_of(text(alias)));
    }
    // A path with no explicit alias derives one from its basename, by the same
    // rule the loader applies — spelled once, in `cook-contracts`, so this
    // crate cannot come to a second opinion about what `./build/my-cc.lua`
    // binds.
    let path = node.child_by_field_name("path")?;
    Some(derived_alias(text(path)))
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
/// # Presence is structural, and it is about the name that gets bound
///
/// A match is a `use_declaration` node already binding `module` as its alias,
/// by [`bound_alias`] — so `use cook_cc`, `use "cook_cc"`,
/// `use cook_cc ./vendor/cc.lua` and `use ./cook_cc.lua` all count, because
/// adding a second declaration of that name is not an addition but a conflict
/// the two phases resolve differently.
///
/// `source.contains("use cook_cc")` would agree with the first three and would
/// also agree with a `use cook_cc` line written inside a step body, where the
/// text is shell content and binds nothing. That last one is the case worth the
/// parser: the caller reports the module available, writes a call to it, and
/// the Cookfile fails to load.
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
    // Compared against the alias the line WOULD bind rather than against
    // `module` itself: the two differ only for a name the grammar rejects, and
    // asking the question in the units the collision happens in keeps this
    // honest for the name production CS-0206 left room for.
    let binds = alias_of(module);
    if uses
        .iter()
        .any(|node| bound_alias(*node, source).as_deref() == Some(binds.as_str()))
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
    let recipe_span =
        locate_recipe(&tree, source, recipe).ok_or_else(|| EditError::RecipeNotFound {
            recipe: recipe.to_string(),
        })?;
    locate_call_within(&tree, source, recipe_span).ok_or_else(|| EditError::NoModuleCall {
        recipe: recipe.to_string(),
    })
}
