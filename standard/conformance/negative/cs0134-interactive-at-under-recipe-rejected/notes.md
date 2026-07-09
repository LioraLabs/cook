Pins CS-0134's removal of the `@` interactive-prefix sigil: an `@./run` line
inside a `recipe` body is now a parse error. Recipes are declarative — there
is no interactive-shell escape hatch left in a recipe body (chores remain
interactive by default and never needed the prefix).
