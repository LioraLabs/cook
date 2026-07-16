§22.3, CS-0127. `cook.recipe(name, {..., origin = "..."}, fn)` accepts an
optional `origin` string on the metadata table and the register pass must
succeed with it present.

Exercises both documented carve-outs for module-minted recipes in one
fixture, per the ticket that introduced `origin`:

- **Data-driven fan-out**: the `for` loop mints one recipe per package
  (`web:build`, `api:build`) tagged `origin = "cook_pnpm.workspace"` — the
  `cook_pnpm.workspace` shape, one recipe per workspace member discovered
  at register time.
- **Support recipe**: `config_header` is a single recipe tagged
  `origin = "cook_cc.config_header"` — the `cook_cc.config_header` shape, a
  fixed helper recipe a module mints alongside the ones it fans out.

This corpus case is parse- and register-phase only. It confirms
`cook.recipe` accepts a string `origin` and the register pass returns Ok;
it does NOT and cannot assert that `cook list` renders `(from
cook_pnpm.workspace)` / `(from cook_cc.config_header)` next to these
recipes — that observation crosses into `cook-cli`'s stdout rendering,
past the `cook-register` library boundary this corpus tests up to. That
assertion belongs to a separate e2e fixture.
