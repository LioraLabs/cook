# Vendored LuaRocks 3.11.0 sources

Verbatim copy of the upstream `luarocks-3.11.0` release tarball
(https://luarocks.org/releases/luarocks-3.11.0.tar.gz). The
`chore bundle-luarocks` step in `cook_modules/luarocks_phase2.lua` stages
`src/luarocks/` into `target/cook-stage/share/luarocks/` so the bundled
`bin/luarocks` shell launcher can `require()` against it.

LuaRocks itself is a pure-Lua program that runs against the bundled
`bin/lua` interpreter (built from `cli/vendored/lua-5.4.7/`). No upstream
build or configure step is invoked at cook build time — the only artifact
copied into the staged tree is the `src/luarocks/` library directory.
The relocatable launcher script and the default config template are
authored cook-side and live in `cook_modules/luarocks_phase2.lua` and
`cli/crates/cook-cli/templates/default-rocks-config.lua`.

## Pin

- Upstream tarball: `luarocks-3.11.0.tar.gz`.
- Source URL: https://luarocks.org/releases/luarocks-3.11.0.tar.gz
- SHA-256: `25f56b3c7272fb35b869049371d649a1bbe668a56d24df0a66e3712e35dd44a6`
- Vendored at: 2026-05-09.

## What ships, what doesn't

`chore bundle-luarocks` copies `src/luarocks/` (the pure-Lua library
tree) into `target/cook-stage/share/luarocks/`. The upstream `Makefile`,
`configure`, `GNUmakefile`, `binary/` (Win32 builder), `spec/`,
`smoke_test.sh`, and `test_regression.sh` are kept here for reference
but are NOT staged into the release tarball — cook does not invoke
upstream's autoconf-style build pipeline.

The original upstream README (badges, build instructions, etc.) is
replaced by this file; the canonical reference is the upstream tarball
identified by the SHA-256 above.

## Refresh procedure

When LuaRocks cuts a new release:

1. Download `https://luarocks.org/releases/luarocks-X.Y.Z.tar.gz` and
   record its SHA-256.
2. `rm -rf cli/vendored/luarocks-3.11.0/` and replace the version
   segment if LuaRocks MAJOR.MINOR.PATCH moved —
   `cp -a <extracted>/. cli/vendored/luarocks-X.Y.Z/`.
3. Update this README's pin section (URL, SHA-256, vendor date).
4. Update `LUAROCKS_SRC_DIR` in `cook_modules/luarocks_phase2.lua`
   and the version string the bundled `bin/luarocks --version` is
   expected to print (referenced in M2.4 gate tests).
5. Run `cook build-lua && cook bundle-luarocks` and confirm
   `target/cook-stage/bin/luarocks --version` prints `LuaRocks X.Y.Z`.
6. Run the relocation smoke test:
   `cp -a target/cook-stage/. /tmp/cook-relocate-test/ && /tmp/cook-relocate-test/bin/luarocks --version`.
7. Run `cook gate-m2` on Linux x86_64 and darwin-arm64 — the bundled
   luarocks must still install lua-cjson against the bundled lib + headers.
