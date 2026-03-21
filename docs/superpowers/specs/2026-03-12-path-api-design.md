# Path API Design Spec

## Overview

A standalone `path` module registered as a global Lua table, providing path manipulation helpers. Follows the same Rust-backed pattern as `fs.*` and `cook.*`. Eliminates repetitive `string:match()` calls for extracting filenames, extensions, and directories from ingredient paths.

## Motivation

The current pattern for working with ingredient paths in Cookfiles is ugly and repetitive:

```lua
local name = src:match("([^/]+)%.c$")
local obj = "build/obj/" .. name .. ".o"
```

With `path.*`:

```lua
local obj = path.join("build/obj", path.stem(src) .. ".o")
```

## API

All functions take and return strings. No special types.

| Function | Input | Output | Description |
|---|---|---|---|
| `path.stem(p)` | `"lib/matrix.c"` | `"matrix"` | Filename without extension |
| `path.name(p)` | `"lib/matrix.c"` | `"matrix.c"` | Filename with extension |
| `path.ext(p)` | `"lib/matrix.c"` | `".c"` | Extension with leading dot (empty string if none) |
| `path.dir(p)` | `"lib/matrix.c"` | `"lib"` | Directory portion (empty string if none) |
| `path.replace_ext(p, new)` | `"lib/matrix.c", ".o"` | `"lib/matrix.o"` | Replace extension |
| `path.join(a, b)` | `"build/obj", "matrix.o"` | `"build/obj/matrix.o"` | Join two path segments |

## Implementation

### Approach

Rust-side registration via `mlua`, identical pattern to `register_fs_api`. Each function uses `std::path::Path` for correct path handling:

- `stem` → `Path::file_stem()`
- `name` → `Path::file_name()`
- `ext` → `Path::extension()` (prepend `.` to result)
- `dir` → `Path::parent()`
- `replace_ext` → `PathBuf::with_extension()` (strip leading `.` from user input if present)
- `join` → `PathBuf::join()`

### Files Changed

- `src/runtime/api.rs` — add `pub fn register_path_api(lua: &Lua) -> Result<(), mlua::Error>` function
- `src/runtime/mod.rs` — call `register_path_api(&lua)?` at both call sites: `execute_recipe` and `list_recipes` (mirrors `register_fs_api`)
- `src/runtime/mod.rs` — add unit tests for all 6 functions
- `examples/Cookfile` — update to use `path.*` instead of `:match()` patterns
- `README.md` — update Cookfile examples and add `path.*` to features list

### Edge Cases

- `path.ext("Makefile")` → `""` (no extension — return empty string, not `"."`)
- `path.dir("file.c")` → `""` (no directory)
- `path.dir("/")` → `""` (root — `parent()` returns `None`, map to empty string)
- `path.stem("archive.tar.gz")` → `"archive.tar"` (Rust `file_stem` behavior)
- `path.stem(".gitignore")` → `".gitignore"` (dotfile — Rust treats as stem with no extension)
- `path.ext(".gitignore")` → `""` (dotfile — no extension)
- `path.stem("")` → `""` (empty input — all functions return empty string for degenerate inputs)
- `path.replace_ext("file.c", ".o")` and `path.replace_ext("file.c", "o")` both work (use `strip_prefix('.')` to remove exactly one leading dot before calling `with_extension`)
