# Cook

A modern build system with Lua. The Cookfile language is defined by the [Cook Standard](standard/); see [`CONTRIBUTING.md`](CONTRIBUTING.md) for development guidelines.

The reference implementation in [`cli/crates/cook-lang/`](cli/crates/cook-lang/) claims **Cook Standard v0.7**.

First-time setup: `cargo install --locked --path cli/crates/cook-cli`. After that, `cook install` updates in place; `cook check` runs the full verification suite.
