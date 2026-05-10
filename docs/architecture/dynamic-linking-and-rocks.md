# Dynamic linking, dlopen, and how Cook loads C-extension rocks

A primer on the runtime mechanics that make `require("cjson")` work inside cook's embedded Lua. Written for the Phase 3 design context, but intended to be readable cold.

## The setup

Cook's binary statically links a copy of Lua 5.4 (via mlua's `vendored` feature). When cook starts, there is exactly one Lua state in memory, owned by the cook process. Every recipe body, every chore, every `cook.fs.copy` call — all running inside that one Lua state.

LuaRocks-installed rocks live on disk under `cook_modules/`:

```
cook_modules/
├── share/lua/5.4/argparse.lua          ← pure-Lua rock
└── lib/lua/5.4/cjson.so                ← C-extension rock (a shared library)
```

When a Cookfile says `use cjson` and a recipe body calls `cjson.encode({})`, what happens depends on which kind of rock it is.

## Pure-Lua rocks: boring

`require("argparse")` searches `package.path`, finds `cook_modules/share/lua/5.4/argparse.lua`, reads the file, runs the Lua source through cook's embedded Lua interpreter, and returns whatever the chunk returned. The argparse module's `local` variables and functions live in cook's Lua heap. Calling `argparse.parse(...)` is a normal Lua function call.

No magic. Same as today's `cook_modules/foo.lua` resolution.

## C-extension rocks: the interesting case

`cjson.so` is **a shared library** — a chunk of compiled machine code, not Lua source. It was produced by compiling `lua_cjson.c` (and friends) with `cc -shared -fPIC`. The file format is ELF on Linux, Mach-O on macOS, PE/COFF on Windows.

Inside `cjson.c`, the C source uses Lua's C API:

```c
#include "lua.h"
#include "lauxlib.h"

static int json_encode(lua_State *L) {
    const char *s = lua_tostring(L, 1);
    /* ... build output ... */
    lua_pushstring(L, output);
    return 1;
}

int luaopen_cjson(lua_State *L) {
    lua_newtable(L);
    lua_pushcfunction(L, json_encode);
    lua_setfield(L, -2, "encode");
    return 1;
}
```

When `cjson.c` is compiled, `lua_tostring`, `lua_pushstring`, `lua_newtable`, etc. are **undefined references**. The compiler emits machine code that says "call the function named `lua_tostring`, but I don't know where it is — the linker will figure it out."

If you statically link `cjson.so` against a copy of `liblua5.4.a`, those symbols get resolved at link time and baked into `cjson.so`. But that's *not what LuaRocks does*. LuaRocks builds `cjson.so` with the undefined references **left undefined** — the symbols stay unresolved in the `.so`. The intent is that the *executable hosting the Lua interpreter* will provide them at runtime.

## dlopen: opening a `.so` from a running process

When `require("cjson")` searches `package.cpath` and finds `cook_modules/lib/lua/5.4/cjson.so`, Lua calls a C function called `dlopen()`. Roughly:

```c
void *handle = dlopen("cook_modules/lib/lua/5.4/cjson.so", RTLD_NOW | RTLD_GLOBAL);
int (*luaopen_cjson)(lua_State *) = dlsym(handle, "luaopen_cjson");
luaopen_cjson(L);
```

`dlopen` does three things:

1. **Loads the shared library into the running process's address space.** The machine code in `cjson.so` is now mapped into memory, alongside the cook executable's own machine code.
2. **Resolves undefined references.** This is the magic step. `cjson.so` references `lua_tostring`, but doesn't contain it. The dynamic linker (`ld.so` on Linux, `dyld` on macOS) walks a list of "places to look for symbols" and finds `lua_tostring` defined somewhere — *somewhere* being the key question.
3. **Runs initializers**, then returns a handle.

Once `dlopen` returns, the cook binary calls `dlsym(handle, "luaopen_cjson")` to get a function pointer, then calls that function. Inside that function, every reference to `lua_pushstring` is now a real machine-level function call into wherever the dynamic linker resolved it.

The whole thing happens **in-process**. No new Lua state, no IPC, no serialization. `cjson.so`'s code runs against cook's existing Lua state because that's the `lua_State *` pointer cook passed in.

## The hard question: where does `lua_tostring` get resolved?

Cook's Lua is **statically linked into the cook binary**. There's no `liblua5.4.so` on disk that `cjson.so` could link against in the obvious way — the symbols are *inside cook itself*.

By default, Linux and macOS executables hide their own symbols from dynamically-loaded libraries. The static Lua copy is there, in cook's binary, but `cjson.so` can't see it. `dlopen` would fail with "undefined symbol: lua_tostring".

This is what `-rdynamic` (Linux) and `-Wl,-export_dynamic` (macOS) fix. From Phase 2's `cli/.cargo/config.toml`:

```toml
[target.'cfg(target_os = "linux")']
rustflags = ["-C", "link-arg=-rdynamic"]

[target.'cfg(target_os = "macos")']
rustflags = ["-C", "link-arg=-Wl,-export_dynamic"]
```

These flags tell the linker that builds the cook executable: "export every defined symbol in the dynamic symbol table, even though this is an executable, not a library." After this:

- Cook is built. The `cook` binary contains Lua's machine code (the statically linked copy). On Linux, that machine code is now in cook's `.dynsym` section, visible to the dynamic linker. `nm -D cook` shows `lua_tostring` as a defined symbol of cook.
- Phase 2's `chore "check-exports"` is exactly this verification — it greps `nm -D` output for the sentinel symbols.

When `dlopen("cjson.so")` runs, the dynamic linker resolves `cjson.so`'s undefined `lua_tostring` reference against **the cook executable's own exported symbol** — the static Lua copy that lives inside cook. `cjson.so` and cook now share the same Lua state because they're calling into the same Lua machine code.

`liblua5.4.so` (the file at `~/.cook/lib/liblua5.4.so`) **is not loaded into the cook process at runtime**. It exists purely as a *link target for compiling new C rocks*. When LuaRocks compiles `cjson.c` for the first time, it links `cjson.so` against `liblua5.4.so`'s headers (so the C compiler knows the type signatures) but emits unresolved references — the actual binding happens at dlopen time, against cook.

That's why Phase 2's gate-m2 fixture matters: lua-cjson is the canonical regression catcher. If the linker flags ever break — or if a future luarocks update changes how `.so` files are produced — `require("cjson")` fails with `undefined symbol: lua_pushstring`. Loud, unambiguous, easy to debug.

## macOS adds one more wrinkle: two-level namespacing

By default, macOS records the **library name** that defined each symbol when it builds a `.dylib` or `.so`. So if you naively built `cjson.so` against `liblua5.4.dylib`, the resulting `cjson.so` would request `lua_tostring` *from `liblua5.4.dylib` specifically*. At dlopen time, `dyld` would load `liblua5.4.dylib` to satisfy that request — even though cook itself already has the symbol.

This is a real bug: cook would end up with **two Lua states**. The one statically inside cook, used by every recipe body. And a second one inside `liblua5.4.dylib`, which `cjson.so` would now be calling into. They wouldn't share heap, registry, or anything. `cjson.encode` would push a string into the wrong state, and the recipe body wouldn't see anything come back.

LuaRocks avoids this by passing `-undefined dynamic_lookup` (or, equivalently, building with flat-namespace) when compiling C rocks on macOS. That tells the linker "leave the symbol references generic — at dlopen time, resolve them against whatever provides them in the loading process." Combined with `-Wl,-export_dynamic` on the cook side, the resolution lands on cook's static Lua, which is what we want.

Phase 2's `default-rocks-config.lua` deliberately **does not override LDFLAGS** on macOS — LuaRocks's default `-undefined dynamic_lookup` is exactly what we need. There's a comment in that config explaining why future-us shouldn't be tempted to "tidy it up."

## Why Windows is different

Everything above relied on a property of ELF (Linux) and Mach-O (macOS): symbols can be resolved across the executable/shared-library boundary at load time, treating the executable's exported symbols as a global pool. With the right flags, the OS unifies symbol lookup across an executable and the libraries it dlopens.

Windows PE/COFF doesn't work that way. There is no equivalent of `-rdynamic` for `.exe` files. The reasons go back to how Windows was designed:

- **Windows uses explicit import tables.** Every DLL declares which symbols it imports, and from which DLL by name. There's no "search the whole process for `lua_tostring`" step. `cjson.dll` would have to declare "import `lua_tostring` from cook.exe" — but DLLs don't normally import from executables, only from other DLLs. The toolchain can do it (you can produce `cook.lib` as a stub library and link `cjson.dll` against it), but it's awkward, slow to build, and it bakes the executable name into every rock.
- **Symbol resolution happens at link time, not dlopen time.** When `cjson.dll` is built, every `lua_*` reference must already be bound to a specific DLL by name. The "leave it unresolved, resolve at load time" model that ELF and Mach-O support is not how PE works.
- **`LoadLibrary` (Windows's `dlopen`) doesn't unify symbol lookup.** Once a DLL is loaded, it sees its own imports plus Windows's own DLLs. It does not see the host executable's symbols by default.

The practical consequence: on Windows, statically linking Lua into `cook.exe` makes C-extension rocks fundamentally unable to find `lua_*` symbols.

The parent design doc resolves this by **flipping the model on Windows only**:

- Windows ships `lua54.dll` next to `cook.exe`.
- `cook.exe` is built without mlua's `vendored` feature on Windows; it links dynamically against `lua54.dll`.
- C rocks on Windows are also linked against `lua54.dll`.
- At runtime, both `cook.exe` and any loaded `cjson.dll` share `lua54.dll`'s single Lua state. The same effect as Linux/macOS, achieved differently.

Trade-offs of the Windows model:
- **Two artifacts to ship instead of one.** `cook.exe` plus `lua54.dll`. The MSI installer handles this; users don't notice. But losing "single self-contained executable" is a real cost on Windows specifically.
- **`lua54.dll` becomes part of cook's ABI.** Replacing it with a different patch version of Lua could break loaded rocks. The `liblua5.4` source must come from the same release that mlua's external-Lua feature targets.
- **The mlua feature gate is a `cfg(not(target_os = "windows"))` on `vendored`.** Linux and macOS keep the static-linking, single-binary model. Windows is the one platform where the model bends.

Phase 5 (Windows packaging) is a separate brainstorm. The parent design captures the architecture but the operational details — MSI structure, `lua54.dll` versioning, MSVC documentation for off-curated rocks — are deferred until Phases 1–3 are stable on Unix. The point of mentioning Windows here is just to explain *why* it's a separate phase: it isn't packaging busywork, it's a fundamentally different symbol-resolution model.

## Mental model in one paragraph

Cook is one process with one Lua state. Pure-Lua rocks are just `.lua` files that cook's Lua reads and runs in that state. C-extension rocks are `.so` files (or `.dylib`, or `.dll`) that cook's Lua dlopens at runtime, which loads compiled machine code into cook's address space; that code calls into Lua via `lua_*` C-API functions, which the dynamic linker resolves against cook's own statically-linked Lua copy because cook was built with `-rdynamic` (Linux) or `-Wl,-export_dynamic` (macOS). LuaRocks is the package manager that downloads, compiles, and lays out rocks; once install is done, LuaRocks is gone. Windows can't do this trick because PE/COFF has no equivalent of `-rdynamic`, so on Windows cook links dynamically against a separate `lua54.dll` and so do the rocks; that achieves the same single-Lua-state property by a different mechanism.

## See also

- Parent architectural spec: `docs/superpowers/specs/2026-05-08-luarocks-modules-design.md` §"Rust-side changes" (the linker-flag rationale).
- Phase 2 design: `docs/superpowers/specs/2026-05-09-luarocks-phase-2-design.md` §M2.2 (`check-exports` chore) and §M2.4 (the lua-cjson regression test).
- Phase 2 commit: `ec32ad9`. The two-line `cli/.cargo/config.toml` edit and the gate-m2 chore are where the runtime-side wiring actually lives in the tree.
- LuaRocks's macOS LDFLAGS handling: `share/cook/default-rocks-config.lua` (committed template) — the comment there explains the `-undefined dynamic_lookup` default.
