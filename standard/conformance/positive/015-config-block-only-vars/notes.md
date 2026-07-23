Demonstrates the canonical replacement for the removed top-level `variable_declaration` form (CS-0011). Named values are declared on the `var` sink (`var.CC`, `var.CFLAGS`) from inside an unnamed (base) `config` block, exercising §5.3 (config-block declaration and composition) and §5.3.1 (the `var` output namespace, CS-0164).

Migrated from the pre-CS-0164 `cook.env.X` output form; the base-only shape (no overlay, paired with a `chore`) complements `config-var-output`, which adds a `release` overlay and a consuming recipe.

Replaces the now-rejected pattern that the negative case `007-bare-vardecl-rejected` covers.
