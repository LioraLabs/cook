# config-var-ambient-env-rejected

CS-0172. `$<HOME>` resolves nothing: the declared-variable namespace is exactly
what the config blocks export, and this one exports only `MODE`.

Before CS-0172 the namespace WAS the process environment table, pre-seeded from
`std::env::vars()`, so this Cookfile built happily and keyed the unit on an
ambient value it never declared — and the config sandbox's `host.env` gate, whose
entire purpose is that a config body's inputs are declared, was bypassable by
simply not declaring them.

A step still inherits the ambient environment as ordinary shell variables, so
`echo $HOME` in the body works. Making a host value a *determinant* is what the
`envs { }` probe (§22.5.2) is for.

The rejection is register-time (the frozen keyset is consulted when the
placeholder lowers), so the parser-only harness SKIPS this fixture.
