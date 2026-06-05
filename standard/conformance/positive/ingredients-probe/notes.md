COOK-88: a bare probe key after `ingredients` is a data-member source, parse/codegen-equivalent to `for_each cards`. Binds members as `$<in>`/`$<in.FIELD>`.

Pins the §8.3 surface: `ingredients <probe>` desugars to a `ForEach` step with `source=ProbeKey("cards")`. The recipe is otherwise identical to `for-each-probe` — same recipe name, same `cook` step outputs and shell body — demonstrating that both driver forms produce the same AST.
