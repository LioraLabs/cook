# README Design Spec

## Overview

A hype-style landing page README for Cook that reflects the author's personality, sells the project, and progressively reveals Cook's capabilities through examples.

## Voice & Tone

- Developer-casual with landing-page energy
- Personal and authentic — the author's own words where possible
- Opinionated and specific, not generic marketing fluff
- "Vibe coded prototype" framing upfront and proud

## Structure

### 1. Header + Tagline

- `# Cook` title
- Bold one-liner: modern build system + power of Make + clarity of Just + embedded Lua
- Subtitle: "My vibe coded prototype of my dream build system."

### 2. Progressive Cookfile Examples

Build excitement by layering capabilities:

**Tier 1: Simple task runner** — shell commands only

```cookfile
recipe "setup"
    mkdir -p build bin
end

recipe "clean"
    rm -rf build bin
end
```

**Tier 2: Build system** — ingredients, serves, dependencies

```cookfile
recipe "build"
    ingredients = {"src/*.c"}
    serves = "bin/app"
    requires = {"setup"}
    gcc src/main.c -Iinclude -Lbuild -lmath -O2 -o bin/app
end
```

**Tier 3: Embedded Lua** — loop over sources, compile, archive, use filesystem API and `cook.sh()`

```cookfile
recipe "lib"
    ingredients = {"lib/*.c", "include/*.h"}
    serves = "build/libmath.a"
    requires = {"setup"}

    >{
        local objects = {}
        for _, src in ipairs(recipe.ingredients[1]) do
            local name = src:match("([^/]+)%.c$")
            local obj = "build/obj/" .. name .. ".o"
            cook.sh("gcc -c " .. src .. " -Iinclude -O2 -o " .. obj)
            table.insert(objects, obj)
        end
        cook.sh("ar rcs " .. recipe.serves .. " " .. table.concat(objects, " "))
    }
end
```

Each example adds a new capability so the reader sees Cook's range.

### 3. "Why Cook?" — Personal Framing

The author's own words:

> I love Make; all of my projects start with a Makefile - even this one. However, archaic syntax, significant whitespace, environment variable declaration 1976 style, quickly becomes a headache in the modern era.
>
> I tried Just and while the syntax is nice, it's missing a lot of the features that make Makefiles so powerful - time stamps, automatic variables, and parallel execution.

Followed by a line positioning Cook as the hybrid task runner + build system answer.

### 4. Feature Highlights

Punchy scannable bullet list:

- Readable syntax (no tabs-vs-spaces, no `$@` cryptography)
- File watching (`cook serve` watches ingredients, re-runs recipes)
- Embedded Lua (drop into Lua right in your Cookfile)
- Dependency resolution (implicit file-based + explicit, cycle detection)
- Filesystem API (`fs.glob()`, `fs.exists()`, `fs.size()`, `fs.read()`, `fs.mtime()`)
- Environment variables (`.env` file support built in)

### 5. CLI Commands

Quick reference table:

- `cook [recipe]` — run a recipe (defaults to "build")
- `cook serve [recipe]` — watch files, re-run on change
- `cook menu` — list all recipes
- `cook init` — create a starter Cookfile
- `cook --plate` — print the generated Lua (debug)

### 6. The Dream (Roadmap)

Short forward-looking section:

- REPL debugger (taste breakpoints already stubbed)
- Parallel recipe execution
- Plugin system ("imagine CMake as a Cook plugin")

### 7. Makefile vs Cook Comparison (The Closer)

The mic drop moment. Show the `$@` `$<` `$(wildcard ...)` `.PHONY` noise vs Cook's clean readable syntax.

**The Makefile:**

```makefile
CC      = gcc
CFLAGS  = -Wall -Wextra -O2
SRC     = $(wildcard lib/*.c)
OBJ     = $(patsubst lib/%.c,build/obj/%.o,$(SRC))
TESTS   = $(wildcard tests/test_*.c)
TEST_BIN = $(patsubst tests/%.c,build/%,$(TESTS))

.PHONY: all clean test

all: bin/app

build/obj/%.o: lib/%.c include/*.h | build/obj
	$(CC) $(CFLAGS) -Iinclude -c $< -o $@

build/libmath.a: $(OBJ)
	ar rcs $@ $^

bin/app: src/main.c build/libmath.a | bin
	$(CC) $(CFLAGS) $< -Iinclude -Lbuild -lmath -lm -o $@

build/%: tests/%.c build/libmath.a
	$(CC) $< -Iinclude -Lbuild -lmath -lm -o $@

test: $(TEST_BIN)
	@for t in $^; do echo "running $$t"; ./$$t; done

build/obj bin:
	mkdir -p $@

clean:
	rm -rf build bin
```

**The Cookfile (same project):**

```cookfile
recipe "setup"
    mkdir -p build/obj bin
end

recipe "lib"
    ingredients = {"lib/*.c", "include/*.h"}
    serves = "build/libmath.a"
    requires = {"setup"}

    >{
        local objects = {}
        for _, src in ipairs(recipe.ingredients[1]) do
            local name = src:match("([^/]+)%.c$")
            local obj = "build/obj/" .. name .. ".o"
            cook.sh("gcc -c " .. src .. " -Iinclude -Wall -Wextra -O2 -o " .. obj)
            table.insert(objects, obj)
        end
        cook.sh("ar rcs " .. recipe.serves .. " " .. table.concat(objects, " "))
    }
end

recipe "build"
    ingredients = {"src/*.c"}
    serves = "bin/app"
    requires = {"lib"}
    gcc src/main.c -Iinclude -Lbuild -lmath -lm -Wall -Wextra -O2 -o bin/app
end

recipe "test"
    ingredients = {"tests/test_*.c"}
    requires = {"lib"}

    >{
        for _, src in ipairs(recipe.ingredients[1]) do
            local name = src:match("([^/]+)%.c$")
            local bin = "build/" .. name
            cook.sh("gcc " .. src .. " -Iinclude -Lbuild -lmath -lm -o " .. bin)
            cook.sh("./" .. bin)
        end
    }
end

recipe "clean"
    rm -rf build bin
end
```

Key contrasts to highlight: tab-sensitivity vs none, `$@`/`$<`/`$^` vs named variables, `$(wildcard ...)`/`$(patsubst ...)` vs glob ingredients, `.PHONY` boilerplate vs implicit, readability at a glance.

## Audience

- Open source community
- Fellow developers / friends
- Portfolio visitors

All three. The README should work as both a project intro and a reflection of who built it.
