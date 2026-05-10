# Cook

A modern build system with Lua. The Cookfile language is defined by the [Cook Standard](standard/); see [`CONTRIBUTING.md`](CONTRIBUTING.md) for development guidelines.

The reference implementation in [`cli/crates/cook-lang/`](cli/crates/cook-lang/) claims **Cook Standard v0.8**.

First-time setup: `cargo install --locked --path cli/crates/cook-cli`. After that, `cook install` updates in place; `cook check` runs the full verification suite.

## Installing community modules

Cook resolves `cook_modules` through LuaRocks against [`rocks.usecook.com`](https://rocks.usecook.com) (and `luarocks.org` as fallback). Declare what you want in `cook.toml`:

```toml
[modules]
cook_cpp = "*"
cook_rust = "*"
```

then realise them with:

```sh
cook modules install                  # install everything declared in cook.toml; pins cook.lock
cook modules install cook_cpp         # add a single dependency
cook modules install cook_cpp cook_rust  # add multiple
cook modules update                   # bump within manifest constraints
cook modules remove cook_cpp          # drop a dependency
```

Modules land in `./cook_modules/` (the local LuaRocks tree). `cook.lock` pins exact versions for reproducible installs and should be committed. To use a different rocks index, edit `cook.toml`'s `[registry].indexes` list — see Cook Standard §7 for resolution order.
