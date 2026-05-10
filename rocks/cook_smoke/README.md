# cook_smoke

Phase 3 acceptance fixture for SHI-176. Published to `rocks.usecook.com`
so cook's modules-install pipeline has a real rock to exercise end-to-end.

This rock is **not stable**. It exposes one function (`cook_smoke.value()`
returns 42) and exists solely to validate that:

- `cook modules install cook_smoke` resolves against `rocks.usecook.com`.
- The resulting `cook_modules/share/lua/5.4/cook_smoke.lua` loads via
  the §7 (CS-0062) runtime resolution.
- `cook.lock` round-trips with `cook_smoke` pinned at the published version.

Do not import `cook_smoke` from a real Cookfile. It will be deleted or
rewritten without notice.

## Publish procedure

See SHI-180 for the rocks.usecook.com upload mechanism. Quick reference:

```sh
~/.cook/bin/luarocks pack rocks/cook_smoke/cook_smoke-0.1.0-1.rockspec
# upload cook_smoke-0.1.0-1.src.rock per SHI-180
```
