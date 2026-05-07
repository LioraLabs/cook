# Cook

A modern build system with Lua. The Cookfile language is defined by the [Cook Standard](standard/); see [`CONTRIBUTING.md`](CONTRIBUTING.md) for development guidelines.

The reference implementation in [`cli/crates/cook-lang/`](cli/crates/cook-lang/) claims **Cook Standard v0.7**.

First-time setup: `cargo install --locked --path cli/crates/cook-cli`. After that, `cook install` updates in place; `cook check` runs the full verification suite.

## Pulling community modules

Cook ships with a small built-in registry of `cook_modules` you can drop into your project:

```sh
cook pull --list           # see what's available
cook pull cpp              # pull the cpp module into ./cook_modules/cpp
cook pull cpp rust         # pull multiple
```

The first time you pull from a given registry, cook prints a one-time disclaimer (the modules are Lua code that `cook` will execute) and records your consent in `~/.config/cook/trust.toml`. Pulled modules are written into your project's `cook_modules/` directory; from then on they're tracked by your project's git, just like any other source file. To update a module, re-run `cook pull <name>`.

To use a different registry: `cook pull --registry https://my.registry/r ...` or set `COOK_REGISTRY_URL` / `[registry].url` in `~/.config/cook/cook.toml`.
