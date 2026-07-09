Pins CS-0134's purely-declarative recipe body: a bare, unprefixed shell
command line (`echo hi`) inside a `recipe` body is now a parse error. Recipe
bodies are register-phase only — imperative shell work belongs in a
`cook "out" { … }` body or a chore, never loose in the recipe itself.
