CS-0101: a file reference is an input, not an iteration driver — rejected in cook output patterns at codegen.

The Cookfile parses cleanly (the sigil scanner admits `$<file:tokens.css>` as a well-formed span); the rejection lives in `cook-luagen::generate_with_names_checked`, which refuses `$<file:PATH>` inside a `cook` output pattern. Fixtures carrying a `codegen_error.txt` are consumed by the codegen-phase harness in `cli/crates/cook-luagen/tests/conformance.rs`; the parser-only harness skips them.
