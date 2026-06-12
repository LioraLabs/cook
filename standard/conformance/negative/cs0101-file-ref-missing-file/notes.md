CS-0101: a literal `$<file:PATH>` reference whose file does not exist is a register-phase error naming the missing path.

The Cookfile parses and codegens cleanly — the hoisted `cook.file_ref("missing.css")` local fails when the recipe body runs during registration. Fixtures carrying a `register_error.txt` are consumed by the register-phase harness in `cli/crates/cook-register/tests/conformance.rs`, which runs with the fixture directory as the working directory (the sibling `src/page.md` exists; only `missing.css` is absent).
