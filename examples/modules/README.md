# module examples

Showcases that require installed cook modules via `cook modules install`.

- `monorepo` — cook_pnpm workspace orchestration

The numbered examples in `examples/` are module-free by design.

## The cook_cc examples were removed

Five cook_cc showcases lived here — `cpp-project`, `lua-build`, `raylib-game`,
`sdl3-game`, `fzf-picker`. The 2026-07-06 overhaul parked them untouched,
intending a pass once the module surface caught up to the v1.0 language cut.
By the time it did, none of them built (COOK-453): three were pinned to
`cook_cc` revisions that predate the engine's current Lua surface, and the two
on `*` had aged past the change that requires target makers to be called inside
a `recipe` block. Repinning did not fix them — it only moved the error — because
their Cookfiles were written against a surface that no longer exists.

They were deleted rather than migrated. A worked example that does not work is
worse than no example, and these had been silently broken for long enough that
nobody was reading them for anything but archaeology.

**The replacement is the tool.** `cook_cc` 0.19.0 ships project-management
verbs, so the canonical C++ starting point is now generated rather than
checked in:

```
mkdir game && cd game
cook modules install cook_cc
cook cc.new
cook build && cook run
```

`cc.add lib math` adds a library, `cc.link game math` wires it, `cc.need sdl2
game` declares an external dependency and probes for it, and `cc.fmt` formats
the tree. What those produce is verified by cook_cc's own suite in the
cook-modules repo, against the published rock, which is a stronger guarantee
than a directory here that only gets exercised when somebody remembers to.

Every example under `examples/` that is *not* module-dependent is now covered
by the `examples` gate in the root `Cookfile`, so this class of rot fails the
build instead of accumulating. `monorepo` is excluded from that gate because it
needs network access and a pnpm toolchain; run it by hand.
