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
- Every question it asks of the call's bytes is asked of one lexical reading of
  them. "Where does a Lua string or comment begin and end" is decided once, by
  `cook_contracts::lua_scan`, and the three things this crate needs to know —
  which `}` closes the list, which `links` is the field, where the last byte of
  code is — are consumers of that reading rather than three scanners of their
  own. They were three, at three fidelities, which is how the field-key scan
  came to splice into a commented-out field while the two scanners beside it
  would have refused to (COOK-403).
- The brace matcher counts depth through strings and comments instead of
  calling `find('}')`, and "string" means all four of Lua's spellings.
  `sources = { "src/a}b.cpp" }` is rare but legal, `[[src/a}b.cpp]]` no less
  so, and a comment mentioning a brace inside a multi-line list is not rare at
  all. This layer exists to preserve comments, which makes miscounting on one a
  particularly poor way to be wrong.
- A field is a table key at the top level of the call's argument. `links` does
  not match inside `"mathlinks.cpp"`, inside a `-- links = { "old" }` the
  author commented out, or as the key of a table nested in the call. The last
  of those is the one worth the check on its own: editing it leaves a file that
  parses, reads correctly, and links against a list nobody edited.
- The insert anchors on the last byte of *code*, not the last non-whitespace
  byte. In a list ending `"mathlib",   -- see docs/build.md {section 2}` the
  last non-whitespace byte sits inside the author's comment. A single `-` still
  counts as code — `{ n-1 }` is not a comment — while a `--` inside a string
  literal is not one either, so a list ending `[[note -- x]]` anchors after the
  bracket rather than inside the literal.
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
  algebra be pinned by 27 string-in/string-out tests with no Lua VM and no
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

## Known limits

Recorded here rather than in a comment nobody greps.

- **A recipe with several module calls edits its first**, by the rule above. A
  caller that cares reads the returned `callee` before writing.
- **`["links"] = { … }` is not matched.** A bracketed key is a table key and
  the scan looks for the bare identifier form, so the edit is refused by name
  rather than mis-aimed. Failing is the correct half of the bargain; the
  spelling is simply not supported yet.

The defects this file used to list — long-bracket literals spliced into, the
field-key scan matching inside a comment or a nested table — are fixed
(COOK-403, CS-0208), and the tests that pin them are named after the shape they
refuse rather than after the bug.

## Relationship to the rest of the workspace

This is the workspace's only Rust consumer of `tree-sitter-cook`. It sits in
the *mechanism* stratum: what it needs a grammar for stays here, and the one
rule it does not own — where a Lua string or comment begins and ends — went
down to `cook_contracts::lua_scan`, where `cook-luagen` already needed the same
answer. Nothing depends back on it from its own stratum or below; its consumers
are the `cook.cookfile.*` binding in `cook-lua-stdlib` and, since CS-0220,
`cook-cli`, which calls `ensure_use` when a named `cook modules install` has to
declare what it installed.

That second consumer is in the *surface* crate for a reason worth recording,
because the obvious home for it is `cook-modules`. Both that crate and this one
are *mechanism*, so the edge would be sideways and the constitution refuses it.
The refusal turns out to name the seam correctly: `cook-modules` knows what a
rock is, and writing the author's project files is a job `cook-cli` already
owns for `cook init`.

Its dependency on the grammar is closer than the module boundary suggests, and
in one direction only: the call span it splices within is a `module_call_text`
token produced by `tree-sitter-cook`'s external scanner, so a construct that
scanner cannot span is a construct this crate cannot edit. That coupling has
already produced one defect — the scanner read a long bracket as code, and this
crate reported a syntax error in files `cook-lang` accepts (fixed under
CS-0208). `cook-lang` remains the authority on what a Cookfile means, so a
disagreement between the two parsers is always the grammar's bug.

The splicing half of the surface still has no shipped module consumer. CS-0179
was written for the `cc.add` / `cc.link` / `cc.need` verbs of CS-0176, and none
of those exist yet; the only callers of `splice_into_field` outside this
crate's own tests are the synthetic `cook_edit` modules in
`standard/conformance/positive/cookfile-splice-preserves-comments/` and
`.../cookfile-splice-skips-strings-and-comments/`. `ensure_use` is the first
piece of the algebra with a caller in a shipped verb (CS-0220), which is also
the first time the preservation claim is made against a file a user did not opt
in to having edited.
