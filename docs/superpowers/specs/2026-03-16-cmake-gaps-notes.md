# "Are We CMake Yet?" — Working Notes

Captured during brainstorming session 2026-03-16. These are rough notes to revisit after the testing feature is complete.

## Gap Summary

| Area | Status | Priority | Notes |
|---|---|---|---|
| Testing | Missing | P0 | **Designing now — see testing design doc** |
| Package management | Minimal (pkg-config only) | P0 | Extend `cpp.find()` to support conan/vcpkg. Same return shape. Add `dependencies` block to Cookfile for auto-install. Don't build our own registry. |
| Install / packaging | Missing | P1 | `cpp.install()` — module already knows artifacts. Headers→include/, libs→lib/, bins→bin/. CPack-style packaging is stretch. |
| CI/CD ergonomics | Partial | P1 | Need: `--output-format json`, `NO_COLOR`, `--quiet`, build timing summary, documented exit codes. This is the Cook Cloud data pipeline. |
| Cross-platform (Windows/MSVC) | Missing | P2 | Massive effort. Defer until Linux/macOS adoption proves the tool. |
| Reproducible builds | Not discussed | P2 | Hermetic/pinned toolchains. |
| Cross-compilation | Missing | P2 | Ties into Windows support. |
| IDE support | Partial (compile_commands.json) | P2 | Module can auto-register project init recipe. |

## Strategic Framing

- Prioritization: **developer experience depth first** (testing, packages, CI) on Linux/macOS, with **cloud-readiness woven in** (structured output everywhere).
- Windows comes later — plenty of enterprise teams build on Linux/macOS only (servers, embedded, cloud infra, game servers).
- Every CLI feature should support the Cook Cloud SaaS goal: structured output, machine-readable results, cache that can go remote.

## cpp Module Specific Changes Needed

- `cpp.find()` → extend beyond pkg-config to conan/vcpkg
- `cpp.install()` → new function for install targets
- `cpp.test()` → new function that compiles test binary + registers with Cook's test API (see testing design)
- Modules adding their own recipes without explicit Cookfile declaration (e.g., auto-register "compile-commands" recipe)
