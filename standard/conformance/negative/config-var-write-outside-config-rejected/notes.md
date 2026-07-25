# config-var-write-outside-config-rejected

CS-0172. A declared value is a cache determinant of every unit that consulted
it (§17.1), so it cannot be reassigned once recipes are registering against it.
`var` is the writable sink only inside a `config_body`; everywhere else it is a
read-only proxy whose store is unreachable from Lua.

The pre-CS-0172 surface exposed the live table as `cook.env`, so a register
block could rewrite a value that units had already been keyed on.
