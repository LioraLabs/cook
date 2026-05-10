# Vendored Lua 5.4.7 sources

Verbatim copy of the `lua-src` crate's `lua-5.4.7/` subtree at version
`547.0.0`, the Lua release that mlua statically links into the cook
binary via the `vendored` feature, **plus** `lua.c` and `luac.c` taken
verbatim from the upstream Lua 5.4.7 release tarball
(https://www.lua.org/ftp/lua-5.4.7.tar.gz). The lua-src crate strips the
two driver entry points because it only embeds the library; cook ships
both the embeddable library AND the standalone `bin/lua` and `bin/luac`
artifacts, so it needs them.

Cook uses these sources to build the bundled
`liblua5.4.{so,dylib}`, `bin/lua`, `bin/luac`, and `include/lua5.4/`
artifacts that ship in the `~/.cook/` install layout, so C extension
rocks compiled against the bundled headers are ABI-compatible with the
embedded Lua state.

## Pin

- Upstream crate: `lua-src` v547.0.0 (encodes Lua 5.4.7).
- Pinned via: `mlua = "0.10"` (`vendored` feature) in
  `cli/crates/cook-register/Cargo.toml`,
  `cli/crates/cook-luaotp/Cargo.toml`,
  `cli/crates/cook-lua-stdlib/Cargo.toml`.
- Recorded in `cli/Cargo.lock`.
- `lua.c` and `luac.c` taken from
  https://www.lua.org/ftp/lua-5.4.7.tar.gz `lua-5.4.7/src/`.

## Drift guard

The cargo test `cli/crates/cook-cli/tests/lua_source_drift_test.rs`
asserts every file under this directory (except this README, `lua.c`,
and `luac.c`) hashes identically to the corresponding file in the
resolved `lua-src` crate source tree. The test runs as part of
`cargo test` and will fail if:

- mlua updates `lua-src` to a newer Lua release without a matching refresh
  of this directory, or
- a file here is edited out-of-band.

`lua.c` and `luac.c` are excluded from the guard because they are not
in the lua-src crate. They are version-pinned to Lua 5.4.7 — the same
release lua-src embeds — so ABI alignment holds.

## Refresh procedure

When mlua bumps `lua-src` (visible in `cli/Cargo.lock`):

1. `cargo metadata --manifest-path cli/Cargo.toml --format-version 1 \
    | jq -r '.packages[] | select(.name=="lua-src") | .manifest_path' \
    | xargs dirname` — locate the new crate's `lua-X.Y.Z/` directory.
2. `rm -rf cli/vendored/lua-5.4.7/` (or replace the version segment if the
   Lua MAJOR.MINOR also moved — see step 6).
3. `cp -a <new-lua-src>/lua-X.Y.Z/. cli/vendored/lua-5.4.7/`.
4. Download the matching upstream Lua tarball from www.lua.org/ftp/ and
   copy `src/lua.c` and `src/luac.c` into `cli/vendored/lua-5.4.7/`.
5. Update this README's pin section.
6. If Lua MAJOR.MINOR moved (e.g., 5.4 → 5.5), rename the directory and
   update every reference in `cook_modules/dist.lua` and the
   default-rocks-config.lua template.
7. `cargo test -p cook-cli --test lua_source_drift_test` — must pass.
8. Run `cook package` on Linux x86_64 and darwin-arm64 (mini) — the
   bundled lib + headers must still pass the C-extension and lua-cjson
   tests (run automatically as the final `verify` step of `dist.package`).
