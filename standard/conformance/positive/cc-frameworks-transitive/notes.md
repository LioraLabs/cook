# cc-frameworks-transitive

Locks §9.2.4 propagation of `frameworks` from a `cc.lib` target into a
downstream `cc.bin` via the `links` chain. Parse-only: this fixture
confirms the Cookfile shape parses; the resolve-and-emit behaviour is
verified by cook_cc/spec/transitive_spec.lua and targets_spec.lua.
