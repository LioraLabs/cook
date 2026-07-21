//! Rust bindings for the Cook tree-sitter grammar.
//!
//! Scope note: this grammar parses Cookfile *frame* structure — `recipe`,
//! `chore`, `register`, declarations, step kinds, dependency lists. Embedded
//! Lua payloads (`module_call_text`, `lua_code`) are opaque leaf tokens by
//! design; see the scope comments at `grammar.js:161` and `grammar.js:480`.
//!
//! The grammar locates; it does not decide meaning. `cook-lang` remains the
//! sole authority on what a Cookfile means. Consumers here should use node
//! byte ranges to find a span and then verify its contents, never to infer
//! semantics.

use tree_sitter_language::LanguageFn;

extern "C" {
    fn tree_sitter_cook() -> *const ();
}

/// The tree-sitter [`LanguageFn`] for Cook.
pub const LANGUAGE: LanguageFn = unsafe { LanguageFn::from_raw(tree_sitter_cook) };

/// The grammar's generated node-type metadata.
pub const NODE_TYPES: &str = include_str!("../../src/node-types.json");

/// Syntax-highlighting query.
pub const HIGHLIGHTS_QUERY: &str = include_str!("../../queries/highlights.scm");

/// Language-injection query (marks Lua payload spans as `lua`).
pub const INJECTIONS_QUERY: &str = include_str!("../../queries/injections.scm");

#[cfg(test)]
mod tests {
    use tree_sitter::Parser;

    fn parse(source: &str) -> tree_sitter::Tree {
        let mut parser = Parser::new();
        parser
            .set_language(&super::LANGUAGE.into())
            .expect("load Cook grammar");
        parser.parse(source, None).expect("parse succeeded")
    }

    #[test]
    fn grammar_loads() {
        let tree = parse("recipe build\n");
        assert_eq!(tree.root_node().kind(), "source_file");
    }

    #[test]
    fn parses_a_representative_cookfile_without_errors() {
        let source = "use cook_cc\n\
                      \n\
                      cook_cc.uses(\"SDL3\")\n\
                      \n\
                      recipe game\n\
                      \x20   cook_cc.bin({\n\
                      \x20       sources = { \"src/main.c\" },\n\
                      \x20       needs   = { \"SDL3\" },\n\
                      \x20   })\n\
                      \n\
                      chore clean\n\
                      \x20   rm -rf build\n";
        let tree = parse(source);
        assert!(
            !tree.root_node().has_error(),
            "unexpected parse error in:\n{}\ntree: {}",
            source,
            tree.root_node().to_sexp()
        );
    }

    /// The load-bearing capability for Cookfile editing: recover the exact
    /// byte span of a module call so an edit can be spliced into it without
    /// re-rendering anything around it.
    #[test]
    fn recovers_byte_span_of_a_multiline_module_call() {
        let source = "use cook_cc\n\
                      \n\
                      recipe game\n\
                      \x20   cook_cc.bin({\n\
                      \x20       sources = { \"src/main.c\" },\n\
                      \x20   })\n";
        let tree = parse(source);

        let mut cursor = tree.root_node().walk();
        let span = find_kind(&mut cursor, "module_call_text")
            .expect("module_call_text node present in a recipe body");

        let text = &source[span.0..span.1];
        assert!(
            text.starts_with("cook_cc.bin({") && text.trim_end().ends_with("})"),
            "span should cover the whole multi-line call, got: {text:?}"
        );
    }

    /// Confirms the documented limitation rather than assuming it: the Lua
    /// payload is one opaque token, so `links = { ... }` has no node of its
    /// own. An editor built on this grammar must locate the call and then
    /// scan within it.
    #[test]
    fn lua_payload_is_opaque_no_field_level_nodes() {
        let source = "cook_cc.lib({ links = { \"mathlib\" } })\n";
        let tree = parse(source);

        let mut cursor = tree.root_node().walk();
        let span = find_kind(&mut cursor, "module_call_text").expect("module_call_text present");

        // The whole call is a single childless leaf — no `links` node exists.
        let mut c2 = tree.root_node().walk();
        assert!(
            find_kind(&mut c2, "links").is_none(),
            "grammar unexpectedly grew a `links` node; revisit the editing strategy"
        );
        assert!(source[span.0..span.1].contains("links"));
    }

    /// End-to-end contract test for the editing strategy: locate the call
    /// structurally, scan within its span for the field, splice a single
    /// entry. Everything outside the inserted bytes must survive verbatim —
    /// comments, indentation, and non-literal Lua alike. This is the property
    /// a decode/re-encode round-trip cannot offer.
    #[test]
    fn splice_into_links_preserves_comments_and_non_literal_lua() {
        let source = "use cook_cc\n\
                      \n\
                      recipe app\n\
                      \x20   cook_cc.bin({\n\
                      \x20       sources  = { \"src/main.cpp\" },  -- entry point\n\
                      \x20       links    = { \"mathlib\" },\n\
                      \x20       standard = cxx_std,\n\
                      \x20   })\n";
        let tree = parse(source);

        let mut cursor = tree.root_node().walk();
        let (start, end) = find_kind(&mut cursor, "module_call_text").expect("call located");

        // Scan within the located span only. `links` closing brace is the
        // first `}` after the field name.
        let call = &source[start..end];
        let field = call.find("links").expect("links field in call");
        let close = call[field..].find('}').expect("closing brace") + field;

        // Anchor the insert to the last non-whitespace byte before the closing
        // brace, so the existing interior padding stays where the author put it.
        let anchor = call[..close].trim_end().len();

        let mut edited = String::with_capacity(source.len() + 16);
        edited.push_str(&source[..start + anchor]);
        edited.push_str(", \"physlib\"");
        edited.push_str(&source[start + anchor..]);

        // The inserted text is present...
        assert!(edited.contains("{ \"mathlib\", \"physlib\" }"));
        // ...and nothing else moved: comment, variable reference, and the
        // original spacing are all byte-identical.
        assert!(edited.contains("-- entry point"));
        assert!(edited.contains("standard = cxx_std,"));
        assert!(edited.contains("sources  = { \"src/main.cpp\" },"));

        // The result still parses cleanly.
        assert!(!parse(&edited).root_node().has_error());
    }

    /// Depth-first search for the first node of `kind`, returning its byte range.
    fn find_kind(cursor: &mut tree_sitter::TreeCursor, kind: &str) -> Option<(usize, usize)> {
        let node = cursor.node();
        if node.kind() == kind {
            return Some((node.start_byte(), node.end_byte()));
        }
        if cursor.goto_first_child() {
            loop {
                if let Some(found) = find_kind(cursor, kind) {
                    cursor.goto_parent();
                    return Some(found);
                }
                if !cursor.goto_next_sibling() {
                    break;
                }
            }
            cursor.goto_parent();
        }
        None
    }
}
