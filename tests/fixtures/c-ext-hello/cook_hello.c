/*
 * cook_hello.c — Phase 2 acceptance fixture for `chore gate-m2`.
 *
 * Smallest possible Lua C extension: exposes a single function
 * `cook_hello.value()` that returns the integer 42, plus the standard
 * `luaopen_cook_hello` registration entrypoint that Lua's `require()`
 * looks up after dlopen'ing the resulting .so.
 *
 * The whole point of this fixture is to dlopen against cook's
 * `-rdynamic` / `-Wl,-export_dynamic` symbol table: when the .so loads,
 * the unresolved `lua_*` / `luaL_*` references must resolve against the
 * cook executable's exported symbol set. If that link-arg regresses,
 * `require("cook_hello")` fails at load time.
 */

#include <lua.h>
#include <lauxlib.h>

static int cook_hello_value(lua_State *L) {
    lua_pushinteger(L, 42);
    return 1;
}

static const luaL_Reg cook_hello_funcs[] = {
    {"value", cook_hello_value},
    {NULL, NULL},
};

int luaopen_cook_hello(lua_State *L) {
    luaL_newlib(L, cook_hello_funcs);
    return 1;
}
