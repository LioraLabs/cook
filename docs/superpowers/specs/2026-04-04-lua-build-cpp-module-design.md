# Lua-Build cpp Module Integration

## Goal

Refactor the lua-build example from imperative shell commands to use the existing `cpp.lua` cook_module, showcasing header dependency tracking via `.d` files, automatic compiler detection, transitive linking, platform-conditional defines, and compile_commands.json generation.

## New Language Feature: Named Ingredients

A new form of the `ingredients` statement that binds resolved glob results to a Lua variable instead of feeding the `{in}/{out}/{stem}/{all}` template chain.

### Syntax

```
# Unnamed (existing behavior) — feeds template chain
ingredients "*.c" !"foo.c"

# Named (new) — resolves to Lua table of file paths
ingredients sources "*.c" !"foo.c"
```

### Implementation

- **Parser:** Detect `ingredients <identifier> <patterns>` — when a bare identifier follows `ingredients` before the first quoted pattern, parse it as a named binding.
- **AST:** New node `NamedIngredients { name: String, includes: Vec<Pattern>, excludes: Vec<Pattern> }` stored on the recipe.
- **Codegen:** Emit `local <name> = cook.resolve_ingredients({...}, {...})` where includes and excludes are string literals.
- **Runtime:** Register `cook.resolve_ingredients(includes, excludes)` on the `cook` Lua table. Performs the same glob+exclude resolution the engine already does for standalone ingredients, returns a plain Lua table of resolved file paths.

### Scope

The bound variable is `local` to the recipe body. It holds a Lua table of strings (file paths). It does not participate in the `cook ... using` template system.

## Refactored Cookfile

```
use cpp

recipe liblua
    ingredients sources "lua-5.4.7/src/*.c" !"lua-5.4.7/src/lua.c" !"lua-5.4.7/src/luac.c"
    cpp.static_library("liblua", {
        sources = sources,
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
- **Named ingredients** — Cook DSL does glob+exclude, module consumes the result as a Lua table

## Changes Required

1. **Parser** — detect `ingredients <identifier> <patterns>` as a named ingredient binding
2. **AST** — new node for named ingredients (name + includes + excludes)
3. **Codegen** — emit `local <name> = cook.resolve_ingredients({...}, {...})`
4. **Runtime** — register `cook.resolve_ingredients()` Lua function
5. **cpp.lua** — copy into `examples/lua-build/cook_modules/`
6. **Cookfile** — rewrite as shown above

## Decisions

- Use existing `cpp.lua` module as-is (no modifications to the module)
- Compiler auto-detection preferred over hardcoding (module picks what's available)
- Platform-conditional defines via `cook.platform.os` Lua expression
- Keep both `lua` and `luac` executables as separate recipes
- Include `compile-commands` recipe to showcase IDE integration
- Named ingredients use the existing `ingredients` keyword with an identifier before patterns
