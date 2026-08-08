Pins CS-0206: the path form of `use`. All four accepted spellings in one
Cookfile, each binding an alias a recipe-body `module_call` then names, so the
fixture fails if any of them does not bind:

- `use ./build/helpers.lua` — bare path, alias derived from the basename.
- `use "./build/quoted.lua"` — quoted path, same derivation.
- `use fmt ./tools/lua/formatting.lua` — explicit alias, which the basename
  would not have produced.
- `use ./build/my-helpers.lua` — a hyphenated basename, binding `my_helpers`.
  This is the first reachable input §12.1's hyphen-to-underscore rewrite has
  ever had: CS-0035 rejects a hyphen in a `use` NAME, so before CS-0206 the
  rule described a spelling no conforming Cookfile could write (COOK-436). A
  basename passes through no name production, so it can carry one.

The `parse.txt` records the AST shape. The path form renders as
`UseStatement path=… alias=…`, and the name form is deliberately left
rendering as `UseStatement module_name=…` so the corpus's existing entries
stay byte-identical.

Note the recorded paths have lost their `./` prefix: a `use` path is
normalised before it is stored, so `./lua/helpers.lua` and
`lua/helpers.lua` key as one module rather than evaluating one file twice
(§12.3.2). The `.lua` files exist because the fixture describes a real
project; this corpus entry pins the parser, and the runtime halves are pinned
where they execute — see the CS-0206 entry's conformance section.

The helper files are under `lua/` rather than the `build/` a real project might
use, and deliberately: this repository's own `.gitignore` ignores `build/`, and
the first draft of this fixture had two of its four modules silently untracked.
The suite passes off the working tree either way; CI would have run it against a
tree missing the files (COOK-412, COOK-430). Checked with
`git archive HEAD | tar -t`, which is what CI actually sees.
