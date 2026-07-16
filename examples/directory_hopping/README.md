# Directory Hopping — run cook from anywhere in the workspace

Demonstrates workspace ergonomics (Standard §20.2, book §10.1): upward
Cookfile discovery, cwd-scoped bare names, root-anchored cache keys, the
reserved `//` target syntax, and the `.cookroot` workspace boundary.

## Layout

```
directory_hopping/
├── .cookroot            # workspace-boundary marker (zero-content)
├── Cookfile             # root: imports web + tools, bundles their artifacts
├── apps/web/
│   ├── Cookfile         # member: recipe build (has its own import: theme)
│   ├── src/index.html   # nested source dir — no Cookfile here
│   └── theme/
│       ├── Cookfile     # member-of-a-member: recipe build
│       └── palette.txt
└── tools/
    ├── Cookfile         # member: recipe build
    └── main.sh
```

Every recipe is named `build`. Which one a bare `cook build` runs depends
only on where you stand: the nearest Cookfile owns bare names.

## The tour

Run `./walkthrough.sh` to do all of this automatically, or hop around by
hand:

### 1. Build everything from the root

```bash
cd examples/directory_hopping
cook build          # 4 nodes: tools.build, web.theme.build, web.build, build
cat build/bundle.txt
```

### 2. Hop into a member — bare names are the member's

```bash
cd apps/web
cook build          # web's own build — cache hit, nothing re-runs
cook theme.build    # web's own import stays addressable
cook web.build      # ERROR: recipe not found — the root's alias never leaks in
cook menu           # prints: build, theme.build  (the member view)
```

Compare with `cook menu` at the root, which prints the qualified view
(`build`, `web.build`, `web.theme.build`, `tools.build`).

### 3. Hop deeper — upward discovery from a non-Cookfile directory

```bash
cd src              # apps/web/src has no Cookfile
cook build          # walks up to apps/web/Cookfile; same recipe, still cached
```

### 4. Cache keys don't care where you stand

The same unit gets the same cache key from the root, the member dir, or
the nested source dir — cache/log/probe state anchors at the workspace
root (the `.cookroot` dir):

```bash
cook why web.build --json   # from the root
cook why build --json       # from apps/web/
cook why build --json       # from apps/web/src/
```

All three reports carry identical `key` and `cache_key` fields for the
same units; only the display name of the recipe differs (`web.build` at
the root vs bare `build` inside the member).

### 5. Reserved `//` targets

`//`-prefixed targets are reserved for root-anchored resolution
(symmetric with `//` import paths). Cook rejects them today rather than
guessing:

```bash
cook //check
# cook: '//check': root-anchored targets ('//<name>') are reserved syntax
# and not yet supported; run `cook check` from the workspace root instead
```

### 6. The `.cookroot` boundary

The upward walk never escapes the workspace. The `.cookroot` marker in
this directory is the boundary: from any subdirectory in here, discovery
stops at this directory and never selects a Cookfile from an unrelated
enclosing project. To see the boundary refuse a decoy, try it in a
tmpdir:

```bash
mkdir -p /tmp/decoy/project/sub
printf 'recipe build\n    @echo DECOY\n' > /tmp/decoy/Cookfile
touch /tmp/decoy/project/.cookroot
cd /tmp/decoy/project/sub && cook build
# cook: no Cookfile found from .../project/sub up to the workspace
# boundary .../project
```

Without any Cookfile or marker at all, the walk runs to the filesystem
root and fails with `no Cookfile found from <dir> up to the filesystem
root`.

## Cleaning up

```bash
find . -type d \( -name build -o -name .cook \) -exec rm -rf {} +
```
