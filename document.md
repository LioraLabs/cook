# The Cook Manual

The complete guide to the Cookfile language and the `cook` CLI. This is the
friendly, read-top-to-bottom reference; the [Cook Standard](standard/) is the
authoritative formal specification, and where the two disagree, the Standard
wins.

Every Cookfile snippet below is real v1.0 syntax.

## Contents

1. [Installation](#installation)
2. [Your first recipe](#your-first-recipe)
3. [Ingredients and the cook step](#ingredients-and-the-cook-step)
4. [Connecting recipes](#connecting-recipes)
5. [Tests](#tests)
6. [Chores](#chores)
7. [Configuration](#configuration)
8. [Probes and data-driven fan-out](#probes-and-data-driven-fan-out)
9. [Caching and cache trust](#caching-and-cache-trust)
10. [Dropping into Lua](#dropping-into-lua)
11. [Modules and composition](#modules-and-composition)
12. [The cook CLI](#the-cook-cli)

---

## Installation

`cook` is a single Rust binary with a bundled Lua runtime and a bundled LuaRocks.
No package manager to fight, no system Lua to match, no separate server.

Linux and macOS:

```sh
curl -fsSL https://getcook.sh | sh
```

This installs a self-contained tree under `~/.cook/` (the `cook` binary plus a
bundled Lua 5.4 and LuaRocks) and puts `cook` on your `PATH` via `~/.cook/bin`.
Point it elsewhere with `COOK_INSTALL_DIR=/opt/cook`. To update, re-run the
same command; the script is idempotent and only replaces the tree if you're
behind.

From source:

```sh
cd cli
cargo install --locked --path crates/cook-cli
```

Running the command from `cli/` lets Cargo load `cli/.cargo/config.toml`, which
exports the embedded Lua symbols required by native Lua rocks.

### Source-build linking

cook statically embeds its own Lua 5.4; no system Lua is consulted. A native
(C) rock resolves the Lua C API from the `cook` executable when it loads, so a
source-built binary must export those symbols. Builds run from `cli/` pick up
the correct linker flag from `cli/.cargo/config.toml` automatically.

Cargo reads configuration from the directory where you invoke it, not from a
package fetched by `cargo install`. When installing directly from Git, pass
the flag explicitly:

```sh
# Linux
RUSTFLAGS="-C link-arg=-rdynamic" \
    cargo install --locked --git https://github.com/LioraLabs/cook cook-cli

# macOS
RUSTFLAGS="-C link-arg=-Wl,-export_dynamic" \
    cargo install --locked --git https://github.com/LioraLabs/cook cook-cli
```

Without the flag, pure-Lua modules still work. The failure appears only when a
native rock loads: `dlopen` succeeds, then `lua_*` symbol lookup fails with an
error that does not look like a linker problem.

Verify:

```sh
cook --version
```

Per-project state lives in a `.cook/` directory (the content-addressed cache and
build logs). It's safe to delete (the next build repopulates it), and `cook
init` writes a `.gitignore` that keeps it out of version control. To remove cook
entirely: `rm -rf ~/.cook`.

## Your first recipe

```sh
mkdir hello-cook && cd hello-cook
cook init
```

`cook init` drops a minimal starter `Cookfile` and a `.gitignore`. The Cookfile
is deliberately tiny:

```
recipe build
    cook "build/hello.txt" { echo "built with cook" > $<out> }

chore clean
    rm -rf build
```

Run it:

```sh
cook build      # or just `cook`: with no name, cook runs the recipe called `build`
cook            # again: cached, zero work
cook clean      # remove the build output
```

A **recipe** is a named bundle of work. It starts with the keyword `recipe`, a
name, and (optionally) a colon-separated list of dependencies. The indented body
below *declares* the work: here, a single `cook` step naming an output
(`build/hello.txt`) and the command that produces it. Add an `ingredients` line
to build from input files, as the next sections show.

A recipe body is **declarative**: a list of inputs and outputs, not a script.
The step kinds it may contain are `ingredients`, `cook`, `test`, `seal`, and
module calls. A loose shell command (`echo hi` on its own line) is *not* one of
them: imperative, run-every-time commands belong in a [chore](#chores). This is
the one thing to unlearn if you're coming from `make` or `npm scripts`.

### Two phases: register, then execute

When you run `cook build`, cook does two distinct things. First it **registers**
the recipe: it runs the body once, on a single Lua VM, in capture mode, to
*record* the work to be done (it doesn't run the `echo` yet, it records a unit
that says "later, run this command"). Then it builds a dependency graph and
**executes** the units. When you read a Cookfile, you're reading a plan, not a
script. This two-phase model is what makes the caching and the dependency graph
possible.

Handy from day one:

```sh
cook menu       # list every recipe and chore in the workspace
```

## Ingredients and the cook step

Real builds have a shape: take input files, transform each, produce outputs,
don't redo the transform if nothing changed. `ingredients` and `cook` describe
exactly that.

**`ingredients`** declares the input file set. Patterns are quoted globs; a
recipe may have at most one `ingredients` line. Combine includes and excludes
(exclude is `!` immediately followed by a quoted pattern):

```
recipe lib
    ingredients "src/*.c" !"src/scratch.c"
    cook "build/libmath.a" { ar rcs $<out> $<in> }
```

**The `cook` step** produces declared outputs from a command. Its shape depends
on the output pattern:

- **One-to-one**: the output pattern contains an input accessor, so cook
  iterates one unit per ingredient (runnable in parallel):

  ```
  recipe compile
      ingredients "src/*.c"
      cook "build/$<in.stem>.o" { gcc -c $<in> -o $<out> }
  ```

- **Many-to-one**: no input accessor in the output, so one unit consumes every
  ingredient (`$<in>` expands to all of them, space-separated):

  ```
  recipe archive
      ingredients "build/*.o"
      cook "build/libmath.a" { ar rcs $<out> $<in> }
  ```

**Chaining steps into a pipeline.** A recipe can hold several `cook` steps, and
each step's `$<in>` is the *previous* step's output. That's how you express a
pipeline in one recipe, with every step cached independently:

```
recipe assets
    ingredients "images/*.png"
    cook "build/$<in.stem>.webp" { cwebp -q 80 $<in> -o $<out> }   # one WebP per image
    cook "build/manifest.txt"    { printf '%s\n' $<in> > $<out> }  # $<in> = those WebPs
```

Edit one image and cook re-encodes exactly that WebP, then rebuilds the manifest
that consumes it; nothing else runs.

### Placeholders

Inside output patterns and `{ ... }` bodies, `$<...>` placeholders expand. They
were chosen so they never collide with shell syntax: `$VAR`, `${VAR}`, brace
expansion, `awk '{print $1}'` all pass through untouched; only `$<...>` is
substituted. A typo'd placeholder is a load-time error, never a silent empty
string.

| Placeholder | Meaning |
|---|---|
| `$<in>` | the current input (one item in a one-to-one step; all of them in many-to-one) |
| `$<out>` | the current output |
| `$<out_1>`, `$<out_2>` | the Nth declared output, for multi-output steps |
| `$<in.stem>` | a path accessor on the input: `stem`, `name`, `ext`, or `dir` |
| `$<other>` | another recipe's outputs, space-joined; also records the dependency edge (see [Connecting recipes](#connecting-recipes)) |

The four **path accessors** on `src/sub/foo.tar.gz`: `stem` → `foo.tar`, `name` →
`foo.tar.gz`, `ext` → `.gz`, `dir` → `src/sub`.

**Multiple outputs** from one command are declared with multiple patterns and
referenced by index:

```
recipe wasm
    ingredients "src/lib.rs"
    cook "out/app.js" "out/app.wasm" {
        wasm-pack build
        cp pkg/app.js $<out_1>
        cp pkg/app.wasm $<out_2>
    }
```

## Connecting recipes

When one recipe's command reads another recipe's artifact, say so in the
command:

```
recipe compile
    ingredients "src/*.c"
    cook "build/$<in.stem>.o" { gcc -c $<in> -o $<out> }

recipe link
    cook "build/app" { gcc $<compile> -o $<out> }
```

`$<compile>` does three jobs in one stroke: it expands to `compile`'s outputs
(space-joined), it records the dependency edge, and it folds those artifacts'
contents into `link`'s cache key. That last one is the important one. Edit a
`.c` file and cook recompiles one object, then relinks; but if a rebuilt
object comes out byte-identical, the change stops propagating and `link` stays
cached. And because the read is declared, `cook why` can attribute a `link`
miss to the exact artifact that changed.

The reference is also precise about *waiting*: only the step that references
`compile` waits for it. Other steps in the recipe proceed.

**The colon is for ordering, not reading.** A header dependency
(`recipe a: b`) is a fence: every unit of `a` waits for all of `b`, and
nothing of `b`'s content enters `a`'s key. That's the right tool exactly when
nothing is read: grouping meta-targets, chores that want a build first,
run-X-before-Y.

```
recipe all: compile link check

chore play: link
    ./build/app
```

If your command reads an artifact, use the reference. If you only need order,
use the colon. Writing both for the same edge is redundant, and cook warns.

**Parallel where it can be.** Independent units run concurrently; the default
parallelism is your CPU count, overridable with `-j`:

```sh
cook -j 1 link     # serial
cook -j 8 link     # cap at 8
```

**Cycles are an error**, caught during graph construction before any work runs.

**The default recipe** is `build`: bare `cook` runs it. If a recipe name
collides with a subcommand (`init`, `menu`, `list`, `modules`, `test`, `dag`,
`logs`, `cache`, `serve`, `emit-lua`, `affected`, `why`), the subcommand wins;
prefix the recipe with `+` to force it: `cook +test` runs the recipe named
`test`, `cook test` runs the test runner.

**Namespacing across Cookfiles** is done with `import`, not dotted names. A recipe
declared in one Cookfile always has a single, undotted name; dots appear only at
*reference* sites and route through an import alias:

```
import backend  ./backend
import frontend ./frontend

recipe ship: backend.build frontend.build
    cook "out/release.tar" { tar cf $<out> dist }
```

`recipe backend.build` at a declaration site is a parse error; put `recipe
build` in `backend/Cookfile` and import it. (More on composition in
[Modules and composition](#modules-and-composition).)

## Tests

A `test` step checks the things a `cook` step produced. It declares no outputs;
its exit code is the verdict. A `test` inherits the preceding `cook` step's
outputs as its inputs (or the recipe's ingredients if no `cook` ran):

```
recipe check
    ingredients "src/*.c"
    cook "build/$<in.stem>" { gcc $<in> -o $<out> }
    test { ./$<in> --selftest }
```

A failure in one test unit doesn't cancel the others: cook collects every
failure and reports them together. There are no test modifiers; to write a check
that's *expected* to fail, invert the command with `!`:

```
recipe lint
    ingredients "src/*.c"
    test { ! grep -q TODO $<in> }
```

`cook test` runs every test in the workspace and aggregates a pass/fail summary.
Because test results are content-keyed to the outputs they consume, unchanged
code doesn't re-run its tests.

```sh
cook test                    # everything
cook test apps.web           # scope to a namespace
cook test --fail-fast        # stop on first failure
cook test --rerun-failed     # re-run only what failed last time
cook test --report-junit results.xml
```

## Chores

Recipes build files. **Chores** are for everything else: clean the tree, run the
formatter, deploy, open a picker. They have no ingredients, no outputs, and no
cache: a chore runs every time you invoke it, by design. Shell commands in a
chore body are interactive by default (they get a real TTY), so `fzf` and friends
just work.

```
chore clean
    rm -rf build .cook

chore fmt
    cargo fmt
```

Chores take **parameters** (required, defaulted, or variadic) which bind as
shell variables and `$<...>` placeholders in the body:

```
chore deploy target host="prod.example.com"
    rsync -a out/ "$<host>:$<target>"
```

```sh
cook deploy /var/www                 # host defaults to prod.example.com
cook deploy /var/www host=staging    # override
```

Chores and recipes depend on each other with the same colon syntax. `cook play`
below builds first, then drops you into the binary:

```
chore play: build
    ./build/app
```

## Configuration

A `config` block declares knobs. The base block is unnamed; **named overlays** are
applied on top with `@name` at invocation. Only values under `env.*` are visible
to `$<...>` placeholders, and every consulted one folds into the cache key.

```
config
    env.MODE = os.getenv("MODE") or "debug"

config release
    env.MODE = "release"

recipe build
    ingredients "src/*.c"
    cook "build/$<in.stem>.o" { gcc -c -DMODE=$<MODE> $<in> -o $<out> }
```

```sh
cook build              # MODE=debug (the base)
cook build @release     # apply the `release` overlay → MODE=release
cook --set MODE=tiny build   # a one-shot override that wins over everything
```

Flipping a declared value cleanly busts only the units that consumed it. A
`$<KEY>` for a variable you never declared is a hard error: no accidental shell
leaks.

## Probes and data-driven fan-out

A **probe** is a named, cached value that the build graph can see: the output of
a shell command, the contents of a JSON file, a recorded tool identity, a Lua
computation. Recipes consume probes three ways (reference, fan out, seal), and
every one of them is visible to the cache.

### The probe shapes

Four producer forms run a body and cache its value:

```
probe greeting
    { echo hello }                        # bare: a string

probe target_list
    ingredients "data/targets.txt"
    lines { cat data/targets.txt }        # an array, one per line

probe services
    ingredients "data/services.json"
    json { cat data/services.json }       # structured data

probe count: services
    >{ return { n = #cook.probes.get("services") } }   # a Lua value
```

An `ingredients` line on a probe declares the files its body reads, so the value
recomputes exactly when they change. Probes depend on other probes with the same
colon syntax as recipes; a `>{ lua }` body reads its upstreams with
`cook.probes.get`.

Three more forms run nothing of yours; they *record a determinant*:

```
probe compiler
    tools { cc }                          # the resolved identity of an executable

probe build_env
    envs { HOSTNAME TERM }                # environment values

probe sources
    files { "src/**/*.c" !"src/gen/**" }  # a file set, hashed per file
```

`tools` records which `cc` resolved and its content hash. `envs` records the
named environment values. `files` records each matched path's content hash
(`ingredients` glob syntax), so editing, adding, or removing any matched file
changes the value.

Probes are demand-driven: one only runs when something scheduled actually
consumes it. Which brings us to the three ways of consuming.

### Using probes

**Reference one in a step.** A `$<...>` sigil with a colon in its name is a
probe reference (the colon is how cook tells it apart from a recipe or config
reference, so name probes `namespace:thing` when you intend to reference them).
One sigil interpolates the value, records the dependency edge, and folds the
value into the unit's cache key:

```
probe stack:node
    { node --version }

recipe banner
    cook "build/banner.txt" {
        echo "built with node $<stack:node>" > $<out>
    }
```

Upgrade node and the banner rebuilds; nothing else does. `$<key>` substitutes a
string value; `$<key.field>` selects a field of a table value. (Execute-phase
Lua bodies have no sigils; they read the same values with
`cook.probes.get("key")`, which cook detects and wires identically.)

**Fan out over one.** Point a recipe's `ingredients` at a probe (bare name, not
a quoted glob) and the recipe runs once per member. Record fields are
addressable as `$<in.FIELD>`:

```
recipe render
    ingredients services
    cook "build/$<in.name>.conf" {
        printf 'url = %s\n' "$<in.url>" > $<out>
    }
```

Add one entry to `services.json` and exactly one new unit builds; remove one and
cook sweeps its orphaned output. A downstream recipe that iterates the *same*
probe can join to a sibling's per-member output with `$<render[in]>`:

```
recipe summary: render
    ingredients services
    cook "build/$<in.name>.txt" { cat $<render[in]> > $<out> }
```

This is the shape behind eval suites, per-package monorepo tasks, and any
"one unit per record" build.

**Seal one into the key.** Some values should invalidate the cache without
appearing in any command: the compiler's identity, the environment, the files a
tool reads implicitly. `seal` folds the probe's value into the key of every
unit in the recipe:

```
recipe app
    ingredients "src/*.c"
    seal compiler build_env
    cook "build/$<in.stem>.o" { cc -c $<in> -o $<out> }
```

Swap `cc` for a different binary, or change a sealed environment value, and
every unit misses, attributably: `cook why` prints the sealed determinant that
changed. A sealed probe is never interpolated; it is a declared determinant,
nothing more. Sealing is the backbone of shared-cache correctness, and
[Caching and cache trust](#caching-and-cache-trust) shows how it composes with
`files` to cover inputs your `ingredients` line can't hold.

## Caching and cache trust

The cache is cook's center of gravity. Every cacheable unit (a step with at
least one declared output) has exactly **one** content-addressed key, computed
from what you declared: the input contents, the command text or Lua chunk, the
output paths, any consulted environment values, and any sealed probe values. The
local cache and the shared, cross-machine store are addressed by that *same* key,
so a teammate or a CI runner reuses your artifact if and only if it recomputes
the same key.

The engine folds in **only what the author declared**. It infers no machine
identity, toolchain, or locale of its own; that keeps keys portable by default,
and means correctness rests on you declaring your real inputs. Under-declaring is
the failure mode, so cook gives you tools to see the key:

- **`cook why [recipe]`**: read-only. Prints each unit's key and every
  determinant behind it, with hit/miss status. On a shared miss it diffs against
  what the cached artifact was built from, so a miss is always attributable.
- **`cook cache verify`**: re-runs cached steps and reports any byte divergence
  under a matching key. A diagnostic for catching undeclared determinants (run it
  in CI on a different host), explicitly *not* a trust gate.

Per-step **dispositions** control keying and sharing. `seal` (the verb from
[Using probes](#using-probes)) folds declared determinants into the key, and a
trailing keyword on a `cook` step tunes how the artifact is stored and shared:

```
recipe misc
    cook "build/scratch.txt" { ./gen > $<out> } local    # cached locally, never shared
    cook "build/pinned.txt"  { ./gen > $<out> } pinned   # fetch-only; a cold miss is an error
    cook "build/blurb.txt"   { llm gen > $<out> } nondet # non-reproducible; reuse the recording
```

Sealing composes with the `files` producer to solve a common bind: a recipe
whose `ingredients` line is already an iteration driver (a probe fan-out) has
no place to declare the *other* files its command reads. Name them as a
`files` probe and seal it: every unit's key now carries each file's hash, and
`cook why` attributes a miss to the exact file that changed:

```
probe sources
    files { "packages/*/src/*.ts" "packages/*/tsconfig.json" }

recipe typecheck
    ingredients packages                # the fan-out driver occupies this slot
    seal sources                        # so the source files ride the key instead
    cook "build/$<in.name>.stamp" { tsc -p $<in.dir> --noEmit && echo ok > $<out> }
```

Unannotated steps publish after a run and fetch by key before running. Restored
bytes are re-fingerprinted before use, so a corrupted or tampered store entry is
treated as a miss and rebuilt. `--no-publish` (or `COOK_NO_PUBLISH=1`) makes a
client read-only: it still fetches, never uploads.

The shared store itself is a content-addressed directory: point `[cache]
cache_dir` in `.cook/cloud.toml` at a path your team can reach and there is no
server to run. See [Sharing a cache across a team](docs/shared-cache.md) for the
setup, the toolchain-sealing step it depends on, and the operational edges.

## Dropping into Lua

Most Cookfiles are pure surface syntax. Underneath, every Cookfile compiles to
Lua, and you can reach for that API when the surface can't express what you need.
`cook emit-lua` prints exactly what a Cookfile lowers to.

Cook runs in two phases, and Lua appears in both:

- **Execute-phase Lua** is a `>{ ... }` body on a `cook`/`test`/`probe` step (or a
  `>` step inside a chore). It runs when the unit runs, with `input`/`output` (or
  `inputs`/`outputs`) bound to the unit's resolved I/O:

  ```
  recipe upper
      ingredients "src/*.txt"
      cook "build/$<in.stem>.txt" >{
          local text = fs.read(input)
          fs.write(output, text:upper())
      }
  ```

- **Register-phase Lua** in a recipe body is a bare module call: `cook.add_unit
  {...}`, `cook.step_group(...)`, or `mymod.fn(...)`. For loops or locals, put the
  code in a top-level `register` block or a module function:

  ```
  register
      for _, src in ipairs(fs.glob("src/*.c")) do
          cook.add_unit({
              inputs  = { src },
              output  = "build/" .. path.stem(src) .. ".o",
              command = "gcc -c " .. src .. " -o build/" .. path.stem(src) .. ".o",
          })
      end
  ```

`cook.add_unit` is the lowest level: the surface `cook` step desugars to it. Its
fields include `inputs`, `output`/`outputs`, `command` (or `lua_code`),
`discovered_inputs` (for compiler `.d` depfiles, folded into the key), and the
cache-trust fields `probes`, `seal`, `sharing`, and `record`. Fields are strictly
typed: a wrong-typed field is a hard error, never coerced. Use `cook.sh(cmd)` when
you need a command's output *now* in your Lua (feature detection, a git tag);
it runs immediately and returns stdout.

The `>>` register prefix and the `@` interactive prefix that older docs mention
were removed; register-phase Lua is a bare module call, and chore shell is
interactive by default.

## Modules and composition

Two complementary tools:

- **`use`** loads a Lua module that defines helper functions in scope.
- **`import`** pulls another Cookfile in under an alias, so its recipes and chores
  become `alias.name`.

### Blessed modules

cook resolves modules through LuaRocks against
[`rocks.usecook.com`](https://rocks.usecook.com). Declare them in a `cook.toml`
next to your Cookfile:

```toml
[modules]
cook_cc = "*"
```

```sh
cook modules install          # everything in cook.toml; pins cook.lock (commit it)
cook modules install cook_cc  # add one
cook modules update           # bump within constraints
```

Then `use` the module and call its target makers (a top-level `use` plus a
column-0 module call is all it takes):

```
use cook_cc

cook_cc.bin("game", {
    sources = { "src/main.c" },
    needs   = { "SDL3" },
})
```

Today's roster:

- **`cook_cc`**: the flagship. C and C++ native builds: `cc.bin` / `cc.lib` /
  `cc.shared` target makers, pkg-config and curated discovery, transitive
  link/include propagation, `compile_commands.json` generation, and
  autoconf-style feature checks. This is the reference for what a real module
  looks like.
- **`cook_pnpm`**: pnpm-driven JS/TS monorepos: Turborepo-style task pipelines
  running on cook's content-addressed graph, so a JS workspace gets the same
  caching and unified graph as everything else.

`cook_rust` currently reserves the name and is not yet a working build
integration.

### Workspaces and directory hopping

For a monorepo, each subproject keeps its own Cookfile and a root Cookfile
composes them with `import`:

```
# /Cookfile
import web    ./apps/web
import api    ./services/api

recipe ship: web.build api.build
    cook "out/release.tar" { tar cf $<out> dist }
```

Then hop. Run `cook` from anywhere in the tree and it walks up to the nearest
Cookfile and behaves exactly as if you'd started there: bare names resolve
against that Cookfile's own recipes, and its imports answer to their aliases.

```sh
cd apps/web && cook build     # web's own build recipe, by its bare name
cd ../..    && cook web.build # the same recipe through the root's alias
```

Same recipe, same work, same cache entry. Cache identity comes from a recipe's
own Cookfile and the workspace root, never from the directory you invoked from
or the alias an importer chose, so hopping around (or renaming an alias) never
splits the cache: the second command above is a cache hit on the first.

The workspace root is inferred by walking upward for the Cookfile that imports
the tree you're standing in. Drop an empty `.cookroot` file at the top to make
the boundary explicit; it also stops the upward walk, so cook will never select
a Cookfile from an unrelated enclosing project. `--root` overrides everything.

### `cook affected`

A monorepo PR usually touches one corner of the graph; CI shouldn't build the
world to find out. `cook affected` maps a git diff onto the graph:

```sh
cook affected --since=origin/main          # affected recipes, one per line
cook affected --since=origin/main --json
cook test --affected --since=origin/main   # run only the affected slice's tests
```

A recipe is affected when the diff touches its declared file inputs, or when it
depends (directly or transitively) on one that is: invalidation flows
downstream, so `ship` is affected whenever `web.build` is. The diff uses
three-dot merge-base semantics against `--since` and includes your working
tree: staged, unstaged, and untracked-but-not-ignored files all count.

`--recipe <name>` narrows the listing to recipes with that bare name across the
workspace (`--recipe=build` lists every `*.build` and `*:build`). The same
`--affected --since` pair on an ordinary run restricts it to the affected
slice; on `cook test` it's the CI pattern from [The cook CLI](#the-cook-cli).

## The cook CLI

`cook --help` is the source of truth; this is the friendly version.

```
cook [OPTIONS] [COMMAND]
```

With no command, cook runs the `build` recipe. A recipe name runs that recipe; a
subcommand name runs the subcommand. Prefix a recipe with `+` when its name
collides with a subcommand.

**Everyday**

| Command | Does |
|---|---|
| `cook` / `cook <recipe> [preset]` | run a recipe (default `build`), optionally with a config `@preset` |
| `cook init` | scaffold a starter Cookfile and `.gitignore` |
| `cook menu` / `cook list` | list recipes and chores, with each chore's parameters |
| `cook test [scope]` | run tests (`--filter`, `--fail-fast`, `--rerun-failed`, `--report-json`, `--report-junit`) |
| `cook serve [recipe]` | watch ingredients and re-run on change |

**Investigation**

| Command | Does |
|---|---|
| `cook why [recipe]` | explain each unit's cache key and hit/miss (`--json`); read-only |
| `cook cache verify [recipe]` | re-run cached steps, fail on byte-divergence (`--json`) |
| `cook dag [recipe]` | open the build-DAG TUI viewer (`--theme mono`) |
| `cook logs [id]` | browse the per-build log archive (`-n N` for the Nth most recent, `--last-failed`) |
| `cook emit-lua` | print the Lua a Cookfile compiles to |
| `cook affected --since=<ref>` | list recipes whose inputs changed since a git ref (`--recipe`, `--json`) |

**Modules**

`cook modules install [names…] | remove <names> | update [name] | list | search
<query>`: with `--registry`, `--non-interactive`, and `--accept-trust` for CI.

**Global flags worth knowing:** `-f/--file`, `--root`, `-j/--jobs`, `-q/--quiet`,
`-v/--verbose`, `--color`, `--output auto|plain|json`, `--set KEY=VALUE`
(repeatable), `--since <ref>` + `--affected`, `--no-prune`, `--no-publish`.

**Tab completion** is served by the `cook` binary itself, so it always matches
the Cookfile in front of you: it completes your recipes and chores (and config
presets after `@`), not just the built-in subcommands. Add one line to your
shell's startup file:

```sh
# ~/.config/fish/config.fish
COMPLETE=fish cook | source

# ~/.bashrc
source <(COMPLETE=bash cook)

# ~/.zshrc
source <(COMPLETE=zsh cook)
```

`elvish` and `powershell` work the same way.

For CI, the pattern is:

```sh
cook test --affected --since=origin/main    # only test the slice the PR touched
```

---

## Where to go next

- The [Cook Standard](standard/) is the authoritative language and execution
  specification (RFC-2119, with a formal grammar appendix).
- The runnable [examples](examples/) are arranged in learning order.
- [`CONTRIBUTING.md`](CONTRIBUTING.md) covers building and testing cook itself.

Some surface is reserved but not yet shipped (`//` root-anchored targets and
cross-probe member joins among them), and cook rejects those with a clear error
rather than guessing. When in doubt, `cook --help`, `cook why`, and `cook
emit-lua` will tell you what cook actually thinks it's doing.
