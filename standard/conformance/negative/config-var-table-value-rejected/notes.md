# config-var-table-value-rejected

CS-0172. A declared variable is a string, a number, or a boolean — the three
types that have an unambiguous string form for `$<NAME>` interpolation and for
the cache key. A table has neither, and silently rendering one (`table:
0x55f3...`) would put an address in a cache key.

Lists are composed in a `register` block, which has the full Lua library, from
scalar variables the config declared.
