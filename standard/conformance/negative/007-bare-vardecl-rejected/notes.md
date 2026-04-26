Asserts that top-level `NAME "value"` is rejected at v0.2 of the Cook Standard. Per CS-0011 (App. D), the `variable_declaration` form was removed from §2 (lexical) and §3 (syntactic grammar) in favor of config-block-only variables (§3.6.1). The parser MUST therefore treat top-level non-keyword content as an error.

Exercises the negative side of §3.1 (top-level production list) and the absence of §3.3 (formerly variable_declaration).
