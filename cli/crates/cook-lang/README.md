# cook-lang

`cook-lang` turns one Cookfile's text into one Cookfile AST.

It is the current reference implementation of the [Cook Standard](../../../standard/).

## How it does that well

- Its whole public surface is `parse(&str) -> Result<Cookfile, ParseError>`.
  The input is a string, not a path: `std::fs`, `std::env`, and `Path` appear
  nowhere in the crate's source. A parse is reproducible from its argument
  alone, which is what lets the conformance corpus be a directory of text files
  and an expected AST dump rather than a fixture harness with a working
  directory.
- **It resolves nothing.** `seal never_declared`, a dependency naming a recipe
  that does not exist, and `$<nosuchsigil>` all parse clean. Resolution needs
  the *other* Cookfiles, the import graph, and the module search path, none of
  which a function taking one `&str` can see. Refusing to half-answer here is
  why there is no second, weaker name resolver competing with `cook-register`'s.
- **It does not own the placeholder grammar.** `$<...>` spans survive into the
  AST as ordinary text. The strict scanner lives in `cook-contracts::sigil`,
  where `cook-luagen` and the execute-phase worker both read it (CS-0188). A
  parser that pre-tokenised sigils would be a third opinion on a grammar whose
  entire point is that every phase reads a placeholder identically.
- **A block body is the character span between the braces** (§3.9, CS-0154,
  COOK-267/268). The collectors used to drop the opening line's remainder and
  the closing line's prefix, and shell quote tracking was line-local. A heredoc
  or a multi-line quoted string opened beside the `{` broke the block, and
  `>{ return {` silently lost its first statement. `brace_scan.rs` now carries
  the interior language's state across lines: Lua long brackets and block
  comments at any `=`-level, POSIX heredocs including `<<-` and the quoted-tag
  forms. The same fix retired a latent miscount on the inline `{ echo '}' }`.
- **Trailers are read from the exact closing brace**, not from `rfind('}')`.
  A `cook` or `test` step's modifier tail begins at the byte after the brace the
  scanner matched, so a `}` inside the body can never be mistaken for the one
  that ends it. Contexts with no trailer (probe producers, chore Lua blocks)
  reject stray text instead of ignoring it.
- **Removed syntax gets a diagnostic naming the CS that removed it.** `>>`,
  `>>{`, and `@` (CS-0134), `as`, `should_fail`, and `timeout` (CS-0135), and
  `record` (CS-0115) each produce a sentence saying what to write instead. A
  language that cuts surface this aggressively pays for it in "unexpected
  token" errors unless the parser keeps the gravestones.
- **The AST speaks `cook-contracts` where the value is shared law.**
  `Disposition.sharing` is a `cook_contracts::Sharing`, not a private
  `(local, pinned)` pair, so `(true, true)` is unrepresentable and the cache
  reads the same enum the parser wrote. That is the crate's only dependency
  edge into contracts, and it is the right one.
- **The Standard claim is a constant and a corpus, not a paragraph.**
  `COOK_STANDARD_VERSION` is checked by `tests/conformance.rs`, which walks
  every positive case for AST shape and every negative case for its rejection
  class. See below.

## What it does not do

It does not read, resolve, or open a file. Import paths are *classified*
(tree-relative versus `//` workspace-anchored) and checked for `..` and
absolute forms per §7.2; turning either into a location on disk belongs to the
caller that knows where the workspace root is.

It does not run, or even parse, Lua. A `>{ … }` body, a config block, and a
`register` block are collected as opaque text with their whitespace and
comments intact, and handed on. The brace scanner knows just enough Lua
lexical structure to find the matching `}` and no more.

It does not generate anything. `cook-luagen` owns emission; this crate owns the
shape that gets emitted from.

The validation it *does* perform is confined to what one text can decide alone:
duplicate declaration names across the shared recipe/chore namespace (App. A.2),
declaration ordering, duplicate imports and config blocks, and chore parameter
well-formedness. These are properties of the file, not of the program, and each
one is a rejection rather than an inference.

**Known divergence.** One grammar is spelled four ways here. A hyphenated or
dotted probe key is declarable (`probe cc-version`, CS-0131) and referenceable
through a sigil, but `seal cc-version` is rejected as a "malformed probe ref"
by `disposition.rs`, and `ingredients cc-version` is rejected as "unexpected
trailing content" by `cook_line.rs`. The `ingredients` half is a residual the
Standard records (App. E, CS-0131); the `seal` half is not recorded anywhere,
and the two rejections blame different things for the same cause. Consolidating
onto one `PROBE_SEG` predicate is the fix; by the `cook-contracts` admission
bar, that predicate is pure shared law and wants a single home.

## Cook Standard claim

This crate claims **Cook Standard v0.18**.

The claim lives in `src/lib.rs`:

```rust
pub const COOK_STANDARD_VERSION: &str = "0.18";
```

To verify the claim, run the conformance harness:

```bash
cargo test -p cook-lang --test conformance
```

To verify backwards conformance against a previously-cut version:

```bash
standard/scripts/check-conformance-against-tag.sh v0.1
```

See `CONFORMANCE.md` for details and pending CSes.
