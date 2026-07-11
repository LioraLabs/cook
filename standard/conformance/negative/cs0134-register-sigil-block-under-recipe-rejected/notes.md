Pins CS-0134's removal of the `>>{ … }` register-phase sigil block: a
`>>{ … }` block inside a `recipe` body is now a parse error. Multi-line
register-phase work moves into a top-level `register` block instead of
being sigil-fenced inside the recipe.
