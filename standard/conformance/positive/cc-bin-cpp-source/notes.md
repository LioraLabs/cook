Pins §9.2.3.1: `cook_cc.bin(name, opts)` accepts a `sources` list of
C++ source paths plus a `standard` option, and parses cleanly. The
extension-dispatch behaviour (`.cpp` → cxx compiler) is verified by
the busted suite in ~/dev/cook-modules/cook_cc/spec/cc_spec.lua's
"emits a g++ command for a .cpp source" test.

Parse-only verification. Runtime is out of scope for the conformance
suite per Task 20's harness limitation.
