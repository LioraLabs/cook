Pins §9.2.3.1: `cook_cc.bin(name, opts)` accepts a `sources` list of
C source paths and parses cleanly with the standard Lua-step grammar.

Parse-only verification. The harness checks the AST shape against
parse.txt. Runtime behaviour of `cook_cc.bin` (compilation, archive,
link) is covered by the busted suite in ~/dev/cook-modules/cook_cc/
spec/ and by the integration build of examples/cpp-project (which
exercises cc.bin → cc.lib link propagation end-to-end).
