Pins CS-0134's purely-declarative recipe body: a standalone execute-phase
`> print("x")` Lua line is no longer accepted directly under a `recipe`
body. Execute-phase Lua now only belongs inside a `cook "out" >{ … }` body,
a `test >{ … }` body, or a chore.
