# config-var-typed-toplevel-read

CS-0172. Two properties the pre-CS-0172 surface could not express, in one
fixture.

**Ordering.** The `register` block reads `var.optimize` / `var.symbols` /
`var.jobs`. Before CS-0172 config blocks were dispatched *after* the whole
register-phase program had already run, so every one of those reads saw nil —
which is why a top-level `cc.toolchain({ optimize = var.optimize })`, the
place a toolchain is actually configured, could not be config-driven at all.
Codegen now wraps everything after the config function in `__cook_main` and
the engine calls it after dispatch.

**Typing.** `var.symbols` is a boolean and drives an `if`; `var.jobs` is a
number concatenated into a string. Values were string-only when the namespace
was the process environment, so `if var.symbols` would have been true for the
string `"false"`. `$<optimize>` in the recipe body interpolates the same
variable's string form.
