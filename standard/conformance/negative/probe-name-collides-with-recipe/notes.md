Pins CS-0240's one-name-one-kind rule (App. A.2): with probe keys resolving by
name at §{xref.resolution} step 3, a name that is both a probe key and a recipe
gives `$<status>` two readings, so the Cookfile is refused at parse time with a
diagnostic naming both declaration sites.

Declared recipe-first here; `cook-lang`'s `cs0240_recipe_after_probe_of_the_
same_name_is_rejected` covers the reverse order, which is reachable because
probes carry no before-recipes ordering rule.

Semantic, not syntactic: both declarations are well-formed on their own and no
context-free tree holds a uniqueness rule, so the tree-sitter walker carries
this on `SEMANTIC_ONLY_NEGATIVES`.
