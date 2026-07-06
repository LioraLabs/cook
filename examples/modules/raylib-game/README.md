# raylib-game example

A Cook M3 demonstration of `cc.config_header` + `cc.checks.*`. The Cookfile
runs feature tests (`has_header`, `has_function`) and substitutes the results
plus literal toggles into `raylib-src/src/config.h.in`, producing
`build/raylib-src/config.h`. A tiny `src/main.c` includes the generated header
and prints its values so the effect is observable.

A full raylib library build (~30 sources + GLFW + OpenGL/GLAD on Linux) is
intentionally out of scope. The vendored sources are raysan5/raylib 5.5
(zlib/libpng license) and exist solely so the template + include chain is
authentic.

## Build

```bash
cook game
```

The resulting binary is at `build/bin/game`. Run it to see the generated
config values reflected in stdout.
