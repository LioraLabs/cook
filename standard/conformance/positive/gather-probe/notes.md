COOK-88: a bare probe key after `gather` is a data-member source. Binds members as `$<in>`/`$<in.FIELD>`.

Pins the §8.2 surface: `gather <probe>` desugars to a `MemberSource` step with `source=GatherKey("cards")`. COOK-372 renamed the desugar node off the retired `for_each` keyword; the `for-each-probe` fixture this note once compared against went with the keyword itself (CS-0131), so `gather <probe>` is now the only driver form.
