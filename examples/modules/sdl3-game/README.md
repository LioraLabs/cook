# sdl3-game example

A minimal SDL3 demo built via Cook + `cook_cc`. Exercises the `cmake-compat`
finder strategy: SDL3 ships `SDL3Config.cmake` but no `.pc` file, so this is
the canonical M2 case.

## Install SDL3

- **Debian Trixie (testing) / Arch / CachyOS:** `sudo apt install libsdl3-dev` / `sudo pacman -S sdl3`
- **Fedora 41:** `sudo dnf install SDL3-devel`
- **macOS (Homebrew):** `brew install sdl3`
- **Debian Bookworm / Ubuntu 24.04 LTS:** SDL3 is not yet packaged. Build from
  source (see <https://github.com/libsdl-org/SDL/releases>) or wait for the
  next LTS.

`cc.find_or_error` will raise with the install hint if SDL3 is not present.

## Build

```bash
cook game
```

The resulting binary is at `build/bin/game`. Run it; you should see an
800x600 dark window. Press Escape or close the window to exit.

## Notes

- The Cookfile pipes `sdl3.libs` (absolute path to `libSDL3.so` from
  cmake's LINK-mode output) through `extra_ldflags`. This is the documented
  v0.3 channel for cmake-compat's raw-path output (Standard §9.2.3.8.3 +
  spec §7.2 FindResult→BinOpts mapping).
- Case matters: `cook_cc.find("SDL3")` probes `SDL3Config.cmake`; lowercase
  `"sdl3"` would probe `sdl3-config.cmake` (which does not exist). The case
  convention follows cmake's own resolver.
- The `cook.toml [registry] indexes` single-entry line is the same workaround
  for the bundled-luarocks dual-server bug used by `examples/raylib-game/`
  (SHI-211).
