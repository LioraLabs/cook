# cook

**Build artifacts just like grandma used make.**

cook is a build system that remembers what it has already done. It takes the
friendly recipe ergonomics of [`just`](https://github.com/casey/just), keeps the
dependency graph of [`make`](https://www.gnu.org/software/make/), trades
tab-sensitive macros for real [Lua](https://www.lua.org/), and is frankly
obsessive about not building the same thing twice. You describe artifacts in a
`Cookfile`; cook turns those declarations into a dependency graph and runs only
the parts whose inputs changed.

```cook
recipe assets
    ingredients "images/*.png"
    cook "build/$<in.stem>.webp" { cwebp -q 80 $<in> -o $<out> }
    cook "build/manifest.txt"    { printf '%s\n' $<in> > $<out> }
```

```console
$ cook assets                  # encodes every image, then writes the manifest
$ cook assets                  # everything is cached — nothing re-encodes
$ mogrify -resize 50% images/hero.png
$ cook assets                  # re-encodes hero.webp, rebuilds the manifest, nothing else
```

That last line is the point. A recipe is a small pipeline: the first `cook` step
turns each image into a WebP, and the second consumes those results—its `$<in>`
is the collected output of the step before it—to write a manifest. cook
fingerprints every step from the inputs you declared, so an identical build costs
nothing, and changing one image re-encodes exactly that file and refreshes only
the manifest that depends on it.

The same model scales from a handful of assets to a game engine and its asset
tools, or a polyglot monorepo built as one graph.

## The shape of a Cookfile

A **recipe** is a named bundle of build work. Its body is declarative: it lists
ingredients, outputs, tests, cache determinants, and module calls—not arbitrary
run-every-time shell commands.

In the example above:

- `ingredients` selects every PNG input.
- The first `cook` step's output pattern contains `$<in.stem>`, so cook creates
  one independent work unit per image; `$<in>` and `$<out>` are that unit's
  resolved input and output.
- The second `cook` step has no accessor in its output, so it runs once over the
  whole set—its `$<in>` is the collected list of the first step's outputs. That
  is how steps chain into a pipeline: cook tracks the edge between them and
  caches each step on its own.
- A command runs only when its fingerprint misses the cache.

With no recipe name, cook runs the recipe named `build`. `cook menu` shows every
recipe and chore available in the workspace.

```console
$ cook
$ cook notes
$ cook menu
```

Cookfiles are plans rather than scripts. cook first **registers** their declared
work, builds a directed acyclic graph, and then **executes** the graph. Cycles
and unknown dependencies fail before build work begins; independent units run
in parallel.

## Recipes form a graph

Recipes can name ordering dependencies after a colon:

```cook
recipe compile
    ingredients "src/*.c"
    cook "build/$<in.stem>.o" { cc -c $<in> -o $<out> }

recipe link: compile
    cook "build/app" { cc $<compile> -o $<out> }
```

`$<compile>` expands to `compile`'s outputs. It also creates a data dependency:
`link` waits for those outputs and folds their contents into its own key. Since
the reference already records the edge, the explicit `: compile` is optional
here; use header dependencies when ordering matters but no artifact is read.

Edit one source file and cook recompiles one object, then relinks. If an
upstream unit reruns but produces identical bytes, the change stops propagating.

Recipes may declare multiple outputs, and placeholders can select output
indices or path components:

```cook
recipe bundle
    ingredients "src/app.ts"
    cook "dist/app.js" "dist/app.js.map" {
        esbuild $<in> --bundle --sourcemap --outfile=$<out_1>
    }
```

Useful placeholders include `$<in>`, `$<out>`, `$<out_1>`, and the input path
accessors `$<in.stem>`, `$<in.name>`, `$<in.ext>`, and `$<in.dir>`. Ordinary
shell syntax—`$VAR`, `${VAR}`, brace expansion, and awk expressions—passes
through untouched.

## Tests belong to the build

A `test` step checks artifacts without producing another one:

```cook
recipe check
    ingredients "tests/*.c"
    cook "build/tests/$<in.stem>" { cc $<in> -o $<out> }
    test { $<in> }
```

Test results are content-keyed to what they consume. Unchanged code does not
need to be rebuilt or retested. `cook test` runs tests across the workspace and
collects their results; failures do not hide unrelated failures that ran in
parallel.

```console
$ cook test
$ cook test apps.web
$ cook test --affected --since=origin/main
```

The last command restricts testing to the graph slice affected by a Git diff.

## Side effects are chores

Deploying, publishing, cleaning, opening an interactive tool—these actions do
not produce reproducible artifacts. They are **chores**, and chores deliberately
run every time:

```cook
recipe assets
    ingredients "images/*.png"
    cook "build/$<in.stem>.webp" { cwebp -q 80 $<in> -o $<out> }

chore deploy target: assets
    rsync -a build/ "$<target>/"

chore clean
    rm -rf build .cook
```

```console
$ cook deploy staging
```

The assets stay cached; deployment does not. Chores can accept required,
defaulted, and variadic parameters, depend on recipes or other chores, and run
interactive commands with a real terminal.

## Data can shape the build

File globs are only one source of fan-out. A **probe** produces a named, cached
value that recipes and other probes can see:

```cook
probe services
    ingredients "data/services.json"
    json { cat data/services.json }

recipe configs
    ingredients services
    cook "build/$<in.name>.conf" {
        printf 'url=%s\n' "$<in.url>" > $<out>
    }
```

If `services.json` contains an array of records, `configs` runs once per record.
Add one service and cook creates one new unit; existing services remain cache
hits. A downstream recipe iterating the same probe can join the matching output
with `$<configs[in]>`.

Probes can produce strings, lines, JSON values, or Lua values. Native probe
forms can also observe environment variables and executable identities. This
makes external determinants visible to the graph instead of hiding them in
ambient machine state.

## The cache is explicit

Every cacheable unit has one content-addressed key derived from what its author
declared: input contents, command text or Lua chunk, output paths, consulted
configuration, dependency artifacts, and sealed probes.

cook does **not** quietly include the machine, locale, or toolchain in every
key. That keeps artifacts portable by default, but it also makes authors and
modules responsible for declaring real determinants.

```cook
probe compiler
    tools { cc }

recipe app
    ingredients "src/*.c"
    seal compiler
    cook "build/$<in.stem>.o" { cc -c $<in> -o $<out> }
```

`seal compiler` deliberately folds the resolved compiler identity into each
unit. Other per-step dispositions control storage and reproducibility:

- `local` caches on this machine but never shares the artifact.
- `pinned` is fetch-only; a cold miss is an error rather than a rebuild.
- `nondet` records work whose output bytes are intrinsically non-reproducible,
  such as an LLM response.

The local cache and a shared store use the same key. A teammate or CI runner can
reuse an artifact only when they independently compute that same key.

Two commands make this model inspectable:

- `cook why [recipe]` attributes every part of each key and reports hit or miss.
- `cook cache verify [recipe]` reruns cached work and reports byte divergence,
  helping find an undeclared input.

Restored bytes are fingerprinted again before use, so a corrupt or tampered
artifact is treated as a miss.

## Configuration without invisible inputs

A `config` block declares build knobs. Named blocks overlay the base when
selected with `@name`:

```cook
config
    env.MODE = os.getenv("MODE") or "debug"

config release
    env.MODE = "release"

recipe app
    ingredients "src/*.c"
    cook "build/$<in.stem>.o" {
        cc -DMODE=$<MODE> -c $<in> -o $<out>
    }
```

```console
$ cook app
$ cook app @release
$ cook --set MODE=tiny app
```

Only units that consult `$<MODE>` fold it into their key. An undeclared config
placeholder is an error rather than an accidental read from the shell.

## Shell when it is enough, Lua when it is not

Most Cookfiles remain close to the commands they organize. When build logic
needs tables, functions, loops, or libraries, cook exposes real
[Lua](https://www.lua.org/) rather than growing a second programming language:

```cook
recipe upper
    ingredients "src/*.txt"
    cook "build/$<in.stem>.txt" >{
        local text = fs.read(input)
        fs.write(output, text:upper())
    }
```

This Lua runs during execution with the unit's resolved inputs and outputs.
Register-phase Lua can generate work directly through `cook.add_unit`,
`cook.recipe`, `cook.probe`, and related APIs. `cook emit-lua` shows exactly how
a Cookfile lowers to Lua.

Large repositories can also split declarations across directories:

```cook
import api ./services/api
import web ./apps/web

recipe build: api.build web.build
```

Each subtree owns its Cookfile; imports preserve namespaces while joining one
workspace-wide graph. cook can discover the nearest Cookfile from a nested
directory without splitting the workspace cache.

## Extending Cook

cook's core is deliberately language-agnostic. It registers work, connects
dependencies, fingerprints inputs, schedules units, and materializes artifacts.
Knowledge of a compiler, package manager, or framework belongs in a **Cook
module**.

Modules are Lua packages distributed through
[LuaRocks](https://luarocks.org/). Projects declare them in `cook.toml`:

```toml
[modules]
cook_cc = "*"
```

```console
$ cook modules install
```

cook installs the resolved tree under `cook_modules/` and pins exact versions
in `cook.lock`, which should be committed. The release bundles Lua 5.4 and
LuaRocks, so users do not need to coordinate a system Lua installation.

A module can expose **target makers**: Lua functions that translate domain
vocabulary into cook's underlying graph.

```cook
use cook_cc

cook_cc.bin("game", {
    sources = { "src/*.c" },
    needs = { "SDL3" },
})
```

`cook_cc` supplies C and C++ knowledge—compile and link units, toolchain probes,
package discovery, transitive usage requirements, and
`compile_commands.json`—while the core retains responsibility for graph and
cache semantics. `cook_pnpm` applies the same model to pnpm workspaces and their
topological task pipelines. `cook_ai` provides provider and prompt targets for
non-reproducible LLM work.

Projects can keep a module local or publish it as a rock. The
[module-authoring contract](standard/src/content/docs/12-modules.mdx) defines
how target makers declare work without compromising cache correctness.

## What this looks like at scale

The numbered [examples](examples/) grow from a single recipe through fan-out,
tests, imports, cache policy, and deployment—each small and runnable, including
a [data-driven fan-out](examples/05-data-fanout/) that builds one unit per record
(the shape behind eval suites and per-scene rendering). Two repository-sized
builds show why the ideas are worth composing:

### A polyglot monorepo — cook-dogfood

[cook-dogfood](https://github.com/LioraLabs/cook-dogfood) builds a .NET API, a
TypeScript/pnpm web app, a Rust command-line tool, and generated cross-language
contracts as one graph. Each subtree owns its own Cookfile and `import` connects
them; a `stack:versions` probe folds the toolchain versions into the build, and
lockfiles invalidate only the parts of the stack that actually consult them, so a
change in one package doesn't rebuild the rest.

### Doom 3 — dhewm3

[dhewm3](https://github.com/LioraLabs/dhewm3) is a Cookfile for the dhewm3 Doom 3
source port. It uses `cook_cc` to describe the engine, its static libraries, and
generated configuration. Compiling a tool and running it are ordinary upstream
and downstream nodes—not separate build systems joined by scripts.

These are demonstrations, not special modes. They use the same recipes,
ingredients, probes, references, and modules introduced above.

## Install and try it

cook currently supports Linux and macOS. Install the latest release with:

```sh
curl -fsSL https://getcook.sh | sh
```

This creates a self-contained tree under `~/.cook/` containing the binary, Lua
5.4, and LuaRocks. Re-run the command to update.

```console
$ cook --version
```

To build this checkout from source:

```sh
cargo install --locked --path cli/crates/cook-cli
```

Then make a scratch project:

```sh
mkdir hello-cook
cd hello-cook
cook init
cook
```

## Keep exploring

- [The Cook Manual](document.md) is the complete, read-top-to-bottom guide to the
  language and the CLI.
- The runnable [examples](examples/) are arranged in learning order.
- The [Cook Standard](standard/) is the authoritative language and execution
  specification.
- The [architecture notes](docs/architecture/) explain the reference
  implementation.
- [CONTRIBUTING.md](CONTRIBUTING.md) covers building and testing cook itself.

A few more surfaces worth knowing about:

- `cook serve <recipe>` watches declared inputs and rebuilds what changes.
- `cook affected --since=<ref>` lists the recipes affected by a Git diff.
- `cook dag [recipe]` opens an interactive graph viewer.
- `cook logs` browses archived output from parallel builds.
- `cook list` prints recipe and chore names for shell pipelines.
- `cook modules install|remove|update|list|search` manages LuaRocks modules.
- Tree-sitter queries provide Cookfile highlighting and editor support.

The reference implementation currently claims **Cook Standard v0.14**. cook is
pre-1.0 software: if this friendly tour and the Standard disagree, the Standard
wins—and the README has a bug.
