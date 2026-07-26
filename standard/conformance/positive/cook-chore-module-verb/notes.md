Pins Standard §{lua.cook-chore} / CS-0176: a module MAY register a chore of its
own, under a name namespaced to that module, and the `use` declaration alone is
what brings it into existence.

Three properties are pinned here, and the fixture is built so that each one
failing produces a distinct failure rather than a silently-passing case:

  - **The registration happens at module evaluation.** `cook.chore` is called
    from the module's own chunk, and the Cookfile calls no module function at
    all. If an implementation deferred registration to a later call, the chore
    would simply not exist and the register pass would report a different
    recipe set.

  - **The stripped prefix is accepted.** Module `cook_demo` registers
    `demo.greet`. §22.11 admits the module name with a leading `cook_` removed
    as well as the full name, because blessed modules are named `cook_*` while
    their verbs read as `cc.*` / `pnpm.*`. An implementation accepting only the
    full name rejects this fixture.

  - **A chore coexists with an ordinary recipe.** `build` is declared normally,
    so the fixture also pins that a module-registered chore joins the same
    name-uniqueness namespace without disturbing surface declarations.

The negative half — an undotted name, and another module's namespace — is
`negative/cook-chore-undotted-rejected/`.

Not pinned here, because they are not observable from a parse or a clean
register pass, and live as executable assertions in `cli/crates/cook-register/`
(`register_tests.rs`, the `cook_chore_*` block):

  - the §{chores.no-caching} bracket around the body (asserted by `cache = true`
    being refused inside a `cook.chore` body — the failure mode is silent
    caching, so only a rejection test can see it);
  - rejection of registration from a function called after the module load
    returned;
  - `origin`-keyed rendering of the parameter diagnostics.
