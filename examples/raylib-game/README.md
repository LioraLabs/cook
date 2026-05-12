# raylib-game example

A minimal raylib demo built via Cook + `cook_cc`. Exercises `cc.find_or_error`,
the curated raylib finder, and macOS framework propagation through `cc.bin`.

## Install raylib

- **Debian/Ubuntu:** `sudo apt install libraylib-dev`
- **macOS (Homebrew):** `brew install raylib`

`cc.find_or_error` will raise with the install hint if raylib is not present.

## Build

```bash
cook game
```

The resulting binary is at `build/bin/game`.

## Notes

- macOS will warn about deprecated `OpenGL.framework` at link time — expected,
  not actionable in M1. raylib still links and runs correctly.
- The `cook.toml [registry] indexes` single-entry line is a workaround for
  the bundled-luarocks dual-server bug (SHI-211).
