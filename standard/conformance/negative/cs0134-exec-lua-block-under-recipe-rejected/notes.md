Pins CS-0134's purely-declarative recipe body: a standalone execute-phase
`>{ … }` Lua block is no longer accepted directly under a `recipe` body,
mirroring the single-line `>` rejection. Execute-phase Lua blocks now only
belong inside a `cook "out" >{ … }` body, a `test >{ … }` body, or a chore.
