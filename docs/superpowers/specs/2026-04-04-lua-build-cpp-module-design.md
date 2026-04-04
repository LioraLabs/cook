# Lua-Build cpp Module Integration

## Goal

Refactor the lua-build example from imperative shell commands to use the existing `cpp.lua` cook_module, showcasing header dependency tracking via `.d` files, automatic compiler detection, transitive linking, platform-conditional defines, and compile_commands.json generation.

## Ingredients as Lua Variable

The existing `ingredients` line already resolves globs and feeds the `{in}/{out}/{stem}/{all}` template chain. This feature makes the same resolved file list *also* available as a `local ingredients` Lua variable in the recipe body — no new syntax required.

### Example

```
recipe liblua
    ingredients "lua-5.4.7/src/*.c" !"lua-5.4.7/src/lua.c" !"lua-5.4.7/src/luac.c"
    cpp.static_library("liblua", {
        sources = ingredients,   -- <-- resolved file list as a Lua table
        ...
    })
end
```

### Implementation

- **Codegen:** When a recipe has `ingredients`, emit `local ingredients = cook.resolve_ingredients({...}, {...})` at the top of the recipe function body.
- **Runtime:** Register `cook.resolve_ingredients(includes, excludes)` on the `cook` Lua table. Performs the same glob+exclude resolution the engine already does, returns a flat Lua table of relative file paths.
- **No parser or AST changes needed.**

### Scope

The `ingredients` variable is `local` to the recipe body. It holds a flat Lua table of strings (file paths). The existing `{in}/{out}/{stem}/{all}` template system continues to work unchanged alongside this.

## Refactored Cookfile

```
use cpp

recipe liblua
    ingredients "lua-5.4.7/src/*.c" !"lua-5.4.7/src/lua.c" !"lua-5.4.7/src/luac.c"
    cpp.static_library("liblua", {
        sources = ingredients,
        defines = cook.platform.os == "linux" and { "LUA_USE_LINUX" } or {},
        system_libs = { "m", "dl" },
    })
end

recipe lua: liblua
    cpp.executable("lua", {
        sources = { "lua-5.4.7/src/lua.c" },
        links = { "liblua" },
        defines = cook.platform.os == "linux" and { "LUA_USE_LINUX" } or {},
        system_libs = { "m", "dl", "readline" },
    })
end

recipe luac: liblua
    cpp.executable("luac", {
        sources = { "lua-5.4.7/src/luac.c" },
        links = { "liblua" },
        defines = cook.platform.os == "linux" and { "LUA_USE_LINUX" } or {},
        system_libs = { "m", "dl" },
    })
end

recipe build: lua luac
end

recipe test: lua
    test "build/bin/lua -e 'print(\"hello from lua built by cook\")'"
end

recipe compile-commands: liblua lua luac
    cpp.compile_commands()
end

recipe clean
    rm -rf build .cook
end
```

## What This Showcases Over the Old Version

- **Header dependency tracking** via `.d` files — change a header, only affected files recompile
- **Automatic compiler detection** — no hardcoded `CC "gcc"`
- **Platform-conditional defines** — `LUA_USE_LINUX` only on Linux
- **Transitive linking** — `lua` and `luac` get the library path automatically from `links = { "liblua" }`
- **compile_commands.json** — IDE integration via `cook compile-commands`
- **Ingredients as Lua variable** — Cook DSL does glob+exclude, module consumes the result as a Lua table

## Changes Required

1. **Codegen** — emit `local ingredients = cook.resolve_ingredients({...}, {...})` in recipe body
2. **Runtime** — register `cook.resolve_ingredients()` Lua function
3. **cpp.lua** — copy into `examples/lua-build/cook_modules/`
4. **Cookfile** — rewrite as shown above

## Decisions

- Use existing `cpp.lua` module as-is (no modifications to the module)
- Compiler auto-detection preferred over hardcoding (module picks what's available)
- Platform-conditional defines via `cook.platform.os` Lua expression
- Keep both `lua` and `luac` executables as separate recipes
- Include `compile-commands` recipe to showcase IDE integration
- Ingredients variable is always named `ingredients` — no custom naming syntax
- The variable is a flat table (not nested); multiple ingredient groups can be supported later if needed
