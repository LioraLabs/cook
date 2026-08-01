# cook-cookfile

`cook-cookfile` edits a Cookfile by inserting bytes into it, never by
re-rendering it.

## How it does that well

- An edit is one insertion at one byte offset, so everything outside the
  inserted range survives by construction rather than by fidelity effort. The
  tempting implementation is the lossy one: evaluating `cook_cc.bin({ standard
  = cxx_std })` to a table and printing it back writes `cxx_std` out as
  whatever it held, drops every comment, and reorders fields by `pairs()`
  order, none of it recoverable and none of it announced. Standard §22.13
  makes preservation normative and prohibits decode/re-encode by name
  (CS-0179).
- It locates structurally, then scans locally. tree-sitter supplies the two
  facts that are genuinely hard to recover by hand: which byte range is
  `recipe game`, and which range is the module call inside it. It deliberately
  does not supply the field, because every embedded Lua payload is one opaque
  leaf (`grammar.js:161`, `:480`), so a field is found by a scan bounded by the
  call's span. That scan is sound only because its boundaries came from the
  parser.
- The brace matcher counts depth through quotes and `--` comments instead of
  calling `find('}')`. `sources = { "src/a}b.cpp" }` is rare but legal, and a
  comment mentioning a brace inside a multi-line list is not rare at all. This
  layer exists to preserve comments, which makes miscounting on one a
  particularly poor way to be wrong.
- The insert anchors on the last byte of *code*, not the last non-whitespace
  byte. In a list ending `"mathlib",   -- see docs/build.md {section 2}` the
  last non-whitespace byte sits inside the author's comment. A single `-` still
  counts as code until a second one proves it opened a comment, so `{ n-1 }` is
  not mistaken for one; the retraction is exact rather than recomputed.
- Every failure names what it looked for and leaves the file byte-identical:
  `RecipeNotFound`, `NoModuleCall`, `FieldNotFound`, `FieldNotAList`,
  `Unparseable`. This is the property the splice is bought with. A re-rendering
  implementation cannot fail this way because it cannot tell that anything was
  unusual; it writes a plausible file and reports success. Being told to make
  the edit by hand is worse than the edit working and much better than the edit
  appearing to work.
- An unparseable file is refused before any edit, so a syntax error the author
  already has is never compounded by an insertion landing somewhere arbitrary.
- It is pure: `&str` in, `String` out. No filesystem, no environment, no VM.
  Reading, writing, and the CS-0045 sandbox gate stay in the caller
  (`cook-lua-stdlib/src/cookfile_api.rs`), which is what lets the whole editing
  algebra be pinned by 18 string-in/string-out tests with no Lua VM and no
  tempdir.
- The selector is the recipe name, which is exact rather than convenient. A
  target maker is a step contributor deriving its identity from
  `cook.recipe_name()`, so a target *is* its enclosing recipe.

## What it does not do

It does not decide what a Cookfile means. The grammar locates; `cook-lang`
remains the sole authority on semantics, per the scope note in
`tree-sitter-cook/bindings/rust/lib.rs`. Nothing here evaluates Lua, resolves a
variable, or knows what `cook_cc.bin` is; `callee` comes back as text so the
caller can reject a call it did not expect.

It does not touch the filesystem, so it cannot be the thing that writes outside
the project root. It does not render entries: `entry` and `text` are inserted
verbatim, because the caller knows whether it is adding a string, an
identifier, or a table, and guessing would be wrong for two of the three
(§22.13).

It does not choose among several module calls in one recipe. The first wins,
on the ground that a recipe holding several is not a target recipe; a caller
that cares reads the returned `callee` first.

## Known defects

Recorded here rather than in a comment nobody greps. Both are cases where this
crate does the exact thing it exists to prevent.

- **Long-bracket literals are not understood.** `links = { [[a}b]], "c" }`
  splices *inside* the string, yielding `[[a, "d"}b]]`. §22.13 states that a
  `}` inside a string literal MUST NOT close the list, so this is a normative
  violation, not a boundary. `locate_field_interior` (`src/lib.rs:201`) and
  `last_code_end` (`src/lib.rs:302`) both track `"` and `'` and neither tracks
  `[[`.
- **The field-key scan is neither quote- nor comment-aware**, although the two
  scanners beside it are. `find_field_key` (`src/lib.rs:254`) checks only for a
  delimiter before and an `=` after, so a commented-out `-- links = { "old" }`
  above the real field is matched as the key and spliced into, and
  `defines = { "links=1" }` earlier in the same call yields a spurious
  `FieldNotAList`. It is also nesting-blind: given
  `{ opts = { links = {…} }, links = {…} }` it edits the nested list.

The shared root cause is that "where does a Lua string or comment begin and
end" is answered three times in this one file, at three different fidelity
levels. One answer, used by all three scanners, closes all of the above.

## Relationship to the rest of the workspace

This is the workspace's only Rust consumer of `tree-sitter-cook`. It occupies
the stratum `cook-contracts` cannot reach: the rules here are pure, and would
belong there on that count alone, but the crate's dependency budget is serde
and this needs a grammar. Nothing depends back on it; its one consumer is the
`cook.cookfile.*` binding in `cook-lua-stdlib`.

The surface it backs has no shipped module consumer yet. CS-0179 was written
for the `cc.add` / `cc.link` / `cc.need` verbs of CS-0176, and none of those
exist in `cook-modules` today; the only caller outside this crate's own tests
is the synthetic `cook_edit` module in
`standard/conformance/positive/cookfile-splice-preserves-comments/`. The
editing algebra is finished and the verbs that would use it are not.
