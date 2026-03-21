# Configuration System Design

## Goal

Add build configuration support to Cook — bare variables, named config presets (debug/release), `.env` overrides, and CLI flags — all resolved into a single `cook.env` table and available via `{VAR}` template expansion in `using` clauses.

## Design

### Cookfile Syntax

**Bare vars** — top-level defaults, before recipes:

```
CC "gcc"
AR "ar"
CFLAGS "-Wall -Wextra"
```

**Config blocks** — named presets that override bare vars:

```
config "debug"
    CFLAGS "-g -O0 -Wall -Wextra"
end

config "release"
    CFLAGS "-O2 -DNDEBUG -Wall -Wextra"
    LDFLAGS "-s"
end
```

Vars and config blocks must appear before any recipe.

### Resolution Pipeline

All sources merge into a single `HashMap<String, String>`, each layer overwriting previous keys:

```
System env vars           (std::env::vars — lowest priority)
  + Cookfile bare vars     (project defaults)
  + Cookfile config block  (selected via -c, if any)
  + .env file              (local developer overrides)
  + CLI --set flags        (one-off overrides — highest priority)
  = cook.env               (single resolved table)
```

If no `-c` flag is given and no config blocks exist, the result is just system env + bare vars + .env. If config blocks exist but no `-c` is given, no config block is applied (just bare vars layer). Config blocks with no var entries are valid (empty override — no effect). Bare vars are optional — a Cookfile can have only config blocks, only recipes, or any combination.

### CLI Interface

```bash
cook build                             # recipe only, bare var defaults
cook build debug                       # recipe + config preset
cook build release                     # recipe + config preset
cook --set CC=clang build              # recipe + var override
cook --set CC=clang build release      # recipe + config + override
cook serve build release               # subcommand + recipe + config
```

Usage: `cook [OPTIONS] <recipe> [config]`

- `<recipe>` — required first positional argument (defaults to `build` when omitted).
- `[config]` — optional second positional argument. Selects a named config block from the Cookfile. If the name does not match any config block, Cook exits with an error: `cook: unknown config '<name>'`. Available config names are listed in the error message.
- `--set KEY=VALUE` — overrides any var, highest priority, repeatable. Splits on the first `=` only, so `--set CFLAGS=-DFOO=1` sets `CFLAGS` to `-DFOO=1`. Must appear before positional arguments (standard flag ordering).

**Implementation note:** The existing CLI uses clap's `external_subcommand` to capture positional args into a `Vec<String>`. The first element is the recipe name, the optional second element is the config name. `--set` is a global arg parsed by clap before the external subcommand captures remaining positionals.

### Accessing Variables

**In shell templates (`using` clauses):**

```
cook "build/obj/{stem}.o" using "{CC} {CFLAGS} -c {in} -o {out}"
```

Config variables use the same `{VAR}` curly-brace syntax as built-in placeholders. Resolution: built-in placeholders (`{in}`, `{out}`, `{stem}`, `{name}`, `{ext}`, `{dir}`, `{all}`) always take priority — these names are reserved and cannot be used as config variable names. If a `{VAR}` is not a built-in and is not found in `cook.env`, Cook exits with an error at recipe registration time: `cook: undefined variable '{VAR}' in template at line N`.

**In Lua code:**

```
> print("Compiler: " .. cook.env.CC)
```

`cook.env` is the same table as today, just populated with the fully resolved config instead of only `.env` file values.

### Template Expansion (Codegen)

When codegen encounters `{CC}` in a `using` clause and it's not a built-in placeholder, it generates a `cook.env.VAR` lookup:

```
using "{CC} {CFLAGS} -c {in} -o {out}"
```

Becomes:

```lua
cook.exec(cook.env.CC .. " " .. cook.env.CFLAGS .. " -c " .. _cook_in .. " -o " .. _cook_out, 3)
```

Config var expansion happens at Lua execution time from the resolved `cook.env` table.

**Implementation note:** The current `expand_template_with_vars` processes all placeholders in a single pass using a `&[(&str, &str)]` slice. For config vars, the codegen should first expand built-in placeholders (the existing behavior), then do a second pass over remaining `{...}` tokens to generate `cook.env.VAR` lookups. This two-pass approach keeps built-in priority clean and avoids mixing the two sets of variables into a single data structure.

### Parser Changes

New AST nodes:

```rust
pub struct Cookfile {
    pub vars: Vec<(String, String)>,
    pub configs: HashMap<String, Vec<(String, String)>>,
    pub recipes: Vec<Recipe>,
}
```

New tokens:
- `Token::Var { name: String, value: String }` — bare var declaration
- `Token::ConfigHeader { name: String }` — opens a config block
- Config blocks reuse `Token::RecipeEnd` for closing `end`

Ordering: vars and config blocks before recipes.

### Env Resolution (Rust)

```rust
fn resolve_env(
    system_env: HashMap<String, String>,
    cookfile_vars: Vec<(String, String)>,
    config_block: Option<Vec<(String, String)>>,
    dotenv_vars: HashMap<String, String>,
    cli_sets: Vec<(String, String)>,
) -> HashMap<String, String> {
    let mut env = system_env;
    for (k, v) in cookfile_vars { env.insert(k, v); }
    if let Some(config) = config_block {
        for (k, v) in config { env.insert(k, v); }
    }
    for (k, v) in dotenv_vars { env.insert(k, v); }
    for (k, v) in cli_sets { env.insert(k, v); }
    env
}
```

Called in `cli/mod.rs` before `Runtime::new()`. The result replaces the current `load_env()` return value — everything downstream (`cook.env`, shell child processes, template expansion) consumes the same HashMap.

**Behavioral change from today:** Currently `cook.env` only contains values from the `.env` file. After this change, `cook.env` contains system environment variables as the base layer, with Cookfile vars, config blocks, `.env`, and CLI overrides layered on top. Shell commands launched by `cook.exec()` inherit the full resolved env as their child process environment (same as today — `run_shell_command` merges `env_vars` into the child process).

### What Changes Where

- `src/parser/lexer.rs` — new token types for bare vars and config headers
- `src/parser/mod.rs` — parse bare vars and config blocks before recipes
- `src/parser/ast.rs` — add `vars` and `configs` fields to `Cookfile`
- `src/cli/mod.rs` — add `--set` global clap arg, extract optional config from `External` vec's second element, call `std::env::vars()` for system env base layer, `resolve_env()` pipeline replaces current `load_env()` call. Add optional `config` positional to `Serve` subcommand.
- `src/codegen/mod.rs` — two-pass template expansion: built-in placeholders first, then `cook.env.VAR` fallback for remaining `{...}` tokens
- `src/env/mod.rs` — unchanged (still loads `.env` file, returns HashMap)
- `src/runtime/` — unchanged (already consumes `HashMap<String, String>`)

### Example Cookfile

```
CC "gcc"
AR "ar"
CFLAGS "-Wall -Wextra"

config "debug"
    CFLAGS "-g -O0 -Wall -Wextra"
end

config "release"
    CFLAGS "-O2 -DNDEBUG -Wall -Wextra"
    LDFLAGS "-s"
end

recipe "setup"
    mkdir -p build/obj bin
end

recipe "lib": "setup"
    ingredients "lib/*.c" "include/*.h"
    cook "build/obj/{stem}.o" using "{CC} {CFLAGS} -Iinclude -c {in} -o {out}"
    cook "build/libmath.a" using "{AR} rcs {out} {all}"
end

recipe "build": "lib"
    ingredients "src/*.c"
    cook "bin/app" using "{CC} {CFLAGS} {all} -Iinclude -Lbuild -lmath -lm -o {out}"
end

recipe "clean"
    rm -rf build bin
end
```

```bash
cook build                             # uses bare var defaults
cook build debug                       # debug config
cook build release                     # release config
cook --set CC=clang build release      # release with clang
```

## Future Roadmap

The configuration system is the foundation. Future phases build on it:

1. **Platform detection** — `cook.platform` table (os, arch, compiler, compiler_version) populated by Rust at startup. Enables platform-aware Cookfiles and Lua modules.

2. **`use` statement and Lua module loading** — `use "cook.c"` loads a Lua module that can read `cook.env`, `cook.platform`, and call `cook.layer()` to emit work units. Cook ships standard modules; users write their own.

3. **Modules as build graph generators** — Language modules (`cook.cpp`) scan source files for dependencies (e.g., C++20 `import` declarations), build dependency graphs, and emit `cook.layer()` calls programmatically during registration. Native Rust helpers (`cook.native.scan_imports`, `cook.native.which`) handle performance-critical operations.

4. **`include` for nested Cookfiles** — `include "lib"` loads `lib/Cookfile`, namespaces its recipes, paths resolve relative to child directory. Enables multi-directory projects.

5. **Package management** — Cook.toml introduced when needed for dependency declarations, versioning, and a module registry for sharing `cook.*` modules.

Each phase is a separate spec → plan → implement cycle.
