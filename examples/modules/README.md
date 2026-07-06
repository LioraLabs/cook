# module examples

Showcases that require installed cook modules (`cook_cc`, `cook_pnpm`, …)
via `cook modules install`. Parked here untouched during the 2026-07-06
examples overhaul; they get their own pass when the module surface
catches up to the v1.0 language cut (see the doom3 / cook_cc milestone).

- `cpp-project` — cook_cc lib/bin/link, compile_commands, tests
- `lua-build` — cook_cc building real Lua 5.4
- `raylib-game` — cook_cc feature checks + config_header
- `sdl3-game` — cook_cc pkg-config discovery (`needs = {"SDL3"}`)
- `fzf-picker` — cook_cc multi-bin + interactive chore
- `monorepo` — cook_pnpm workspace orchestration

The numbered examples in `examples/` are module-free by design.
