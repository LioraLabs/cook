Pins CS-0240's extension of §{xref.var-namespace}'s reservation to `import`
aliases.

`var.` resolves ahead of every lookup step, so a recipe in a Cookfile imported
under the alias `var` is addressed as `$<var.helper>` — a spelling §10.7
requires to name a declared variable. The recipe is unreachable by
construction. Before CS-0240 the alias was legal and the reference resolved to
the qualified cross-Cookfile form, because the `var.` strip sat at the BOTTOM
of the cascade; hoisting it (which a probe keyed `var` makes necessary) is what
closes that spelling.

Refused at the declaration rather than at each reference: the remedy is a
one-word rename, and diagnosing it per use site would name a variable the
author never wrote.

Semantic, not syntactic — `import var "sub"` is a well-formed import
declaration and only the reserved spelling makes it wrong, so the tree-sitter
walker carries it on `SEMANTIC_ONLY_NEGATIVES`.
