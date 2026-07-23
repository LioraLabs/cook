# config-var-output

CS-0164. A `config` body declares its outputs on the `var` sink (`var.CC`,
`var.CFLAGS`), replacing the pre-CS-0164 `env.*` form. A base block plus a
`release` overlay establish values; the `build` recipe consumes them as
`$<CC>` / `$<CFLAGS>`, proving placeholder resolution routes to the declared
`var.*` value (§5.3, §9.2, §10.2 step 3).

The register-positive harness selects no overlay, so the base alone runs and
both `CC` and `CFLAGS` are declared before the `cook` step substitutes them.
The parser-only harness baselines the AST shape (the `var.*` body is opaque
`LUA_SOURCE`, so the config block still parses as raw text — the surface change
is in what the sandboxed runtime accepts, not in the grammar).
