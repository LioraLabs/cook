<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/readme/logo-dark.svg">
    <img src="assets/readme/logo.svg" width="440" alt="cook, a build system">
  </picture>
</p>

<p align="center"><b>Build artifacts just like grandma used to make.</b></p>

<p align="center">
  <a href="https://github.com/LioraLabs/cook/actions/workflows/ci.yml"><img src="https://img.shields.io/github/actions/workflow/status/LioraLabs/cook/ci.yml?branch=main&style=flat-square&label=ci" alt="ci status"></a>
  <a href="https://github.com/LioraLabs/cook/releases/latest"><img src="https://img.shields.io/github/v/release/LioraLabs/cook?style=flat-square&label=release" alt="latest release"></a>
  <a href="document.md"><img src="https://img.shields.io/badge/docs-the%20manual-4c8dae?style=flat-square" alt="the cook manual"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-Apache--2.0-lightgrey?style=flat-square" alt="Apache-2.0 license"></a>
</p>

Got a lot of chefs complicating your build? cargo, CMake, pnpm, codegen, a
`scripts/` directory nobody will admit to writing... each having its own idea
of what's up to date. cook is one kitchen for the whole crew: a declarative
language that runs them as one dependency graph with one content-addressed
cache. It keeps the recipe ergonomics of
[`just`](https://github.com/casey/just) and the dependency graph of
[`make`](https://www.gnu.org/software/make/), and swaps macro-heavy
configuration for a Lua-backed DSL. You describe artifacts in a `Cookfile`;
cook runs exactly the work whose inputs changed.

Heard enough?

```sh
curl -fsSL https://getcook.sh | sh
```

Still curious? Don't take our word for any of that. Taste it.

## The receipts

Two repository-sized builds. Same recipes, same probes, same modules as
the five-minute tour below; no special modes.

### Cap: Turborepo, replaced

[Cap](https://github.com/CapSoftware/Cap) is a 20k-star open-source screen
recorder: a pnpm monorepo with eleven Turbo tasks, and a Rust workspace
Turbo can't see at all. [cook-cap](https://github.com/LioraLabs/cook-cap)
swaps Turborepo for `cook_pnpm` and benchmarks the swap. The experiment:
two different edits to the same file in `@cap/env`, a package almost
everything depends on.

| Edit to `@cap/env` | Turborepo | cook |
| --- | --- | --- |
| Add a comment | rebuilds 7 of 11 tasks, **~47s** | rebuilds 1 task, **~1s** |
| Add a real export | rebuilds the same 7 tasks, ~47s | rebuilds `@cap/env` plus the one app that ingests it, ~30s |

Turbo hashes inputs and cascades: touch a file and everything downstream is
dirty, whatever you touched. cook keys each task on what it actually
consumes, so a comment that compiles away is a cache hit everywhere
downstream. The Turbo config becomes this:

```cook
use cook_pnpm

cook_pnpm.workspace({
    packages = { "packages/*", "apps/web", "apps/desktop" },
    pm       = "pnpm@10",
    tasks    = {
        build = { outputs = { "dist/**" }, depends_on = { "^build" } },
        ["@cap/web#build"] = { outputs = { ".next/**" }, depends_on = { "^build" } },
    },
})

probe rust:sources
    files { "Cargo.toml", "Cargo.lock", "crates/**/*" }

# The Rust workspace Turbo can't see: more recipes in the same graph.
recipe rust-bins
    seal rust:sources
    cook "target/debug/scap-targets" { cargo build -q -p scap-targets }
```

Every number above is reproduced by
[`bench/compare.sh`](https://github.com/LioraLabs/cook-cap/blob/main/bench/compare.sh)
in that repo. Run it yourself.

### Doom 3: one graph from compiler to demo map

[cook-dhewm3](https://github.com/LioraLabs/cook-dhewm3) builds the Doom 3
source port with `cook_cc`: a 427-node graph producing the engine binary,
the gameplay `.so` plugins, and the asset tools (`dmap`, the AAS compiler),
then feeds those tools their own output: a playable demo map compiled by
the `dmap` the graph just built. Clone, `cook`, play.
`compile_commands.json` for clangd falls out of the same graph.

Straight talk: on a raw header-touch rebuild sweep, ninja is still faster,
1.7x on our bench. What ninja can't do is hold the map compiler, the maps,
and the engine in one cached graph; CMake can bolt that on with custom
commands, but it can't cache it, share it, or explain a miss.

## The five-minute tour

Say you're handed a pile of SVGs and you need a PNG sprite sheet.

```cook
recipe sprite-sheet
    ingredients "images/*.svg"
    cook "build/sprites/$<in.stem>.png" { inkscape -z -e $<out> $<in> }
    cook "build/spritesheet.json" "build/spritesheet.png" {
        printf '%s\n' $<in> | texpack -o build/spritesheet
    }
```

Each `cook` step declares a target. `$<in.stem>` varies per input, so the
first step fans out: one unit per SVG, parallel across your cores, each
cached under its own key. The second step's targets are static, so it
gathers: every PNG in, one sheet out. You never write the loop or the join.
The shape of your outputs tells cook the shape of the work.

```console
$ cook sprite-sheet      # rasterizes every SVG in parallel, then packs the sheet
$ cook sprite-sheet      # cache hit across the board, the kitchen doesn't even turn on
$ nvim images/enemy.svg  # give the enemy a bigger sword
$ cook sprite-sheet      # re-rasterizes enemy.png, repacks the sheet, nothing else
```

Globs aren't the only source of fan-out. Data can shape the build too:

```cook
probe platforms
    json { echo '[ {"name":"web","level":"9"}, {"name":"desktop","level":"0"} ]' }

recipe ship
    ingredients platforms
    cook "build/$<in.name>/game.zip" {
        zip -$<in.level> -j $<out> $<sprite-sheet>
    }
```

`ingredients platforms` points at a **probe**: a named, cached value the
graph can see. The recipe runs once per record, fields addressable as
`$<in.name>`; add a record and exactly one new unit builds, delete one and
cook sweeps the orphaned bundle. And `$<sprite-sheet>` reaches across
recipes: it expands to the sheet's outputs and records the dependency edge.
Eval suites, per-package monorepo tasks, one-job-per-row builds of any
kind: the shape of your data is the shape of your build.

That's the language. Tests, chores (run-every-time side effects like
deploys, with a real TTY), and configuration overlays are in
[the manual](document.md).

## Why not ___?

**make or just?** make decides staleness by mtime and makes you hand-write
every edge in macro language; just has lovely ergonomics and no dependency
graph or cache at all. cook keeps just's ergonomics and make's graph, keys
work by content instead of clock, and gives you Lua where you'd otherwise
be fighting `$(shell ...)`.

**Turborepo or Nx?** Package-level hash cascades: any changed byte in a
dependency dirties every dependent, and the graph ends at the edge of the
JavaScript world. cook keys on what tasks actually consume (that's the Cap
table above) and holds the Rust crates, codegen, and assets in the same
graph as the JS.

**Bazel?** If you can afford to adopt Bazel's world, adopt it: it's the
strongest hermeticity story there is. The price is rewriting your build in
its terms: BUILD files for every target, wrapped toolchains, ecosystem
tools swapped for rules_*. cook wraps the tools you already run (pnpm,
cargo, cc) instead of replacing them, and the shared store is a directory,
not a service. You trade enforced hermeticity for a key you own and can
audit; the next section is how cook keeps that trade honest.

**CMake?** CMake generates build systems for C and C++ and stops at the
language border. `cook_cc` is a module, not a fork: C and C++ land in the
same graph as everything else in the repo. That's the Doom 3 build above.

## The part cook was built for: the cache

### You own the key

Every unit of work is cached under one content-addressed key, and the key is
built from what you declared: nothing else. cook does not quietly fold your
machine, locale, or toolchain into it. That keeps artifacts portable by
default, and it makes *you* responsible for naming your real determinants.

When the compiler matters, say so:
```cook
probe compiler
    tools { cc }

recipe app
    ingredients "src/*.c"
    seal compiler
    cook "build/$<in.stem>.o" { cc -c $<in> -o $<out> }
```

`seal` folds the resolved compiler identity into each unit's key. The local
cache and the shared store are addressed by that same key, so a teammate or
a CI runner reuses your artifact exactly when they'd have computed the same
one. The store is a directory: point everyone at one path, no server to
run.

### No mystery misses

Power over the key comes with a promise: cook treats an unexplainable miss
as a bug in the tool. `cook why` prints every determinant behind every
unit's key with its hit or miss status; on a shared-store miss it diffs your
key against what the cached artifact was actually built from. `cook cache
verify` re-runs cached work and reports byte divergence... how you catch a
determinant you forgot to declare. Work that legitimately diverges (an LLM
response, say) is marked `nondet`, and verify leaves it alone.

### Lua when shell isn't enough

Need imperative scripting? Here it is. A `>{ ... }` body runs Lua instead of
shell, with the unit's resolved I/O in scope:

```cook
recipe upper
    ingredients "src/*.txt"
    cook "build/$<in.stem>.txt" >{
        local text = fs.read(input)
        fs.write(output, text:upper())
    }
```

This is no embedded afterthought: every Cookfile lowers to Lua before
execution, and `cook emit-lua` shows you exactly what yours compiles to.
Modules distributed through LuaRocks use the same interface to teach cook
entire ecosystems; `cook_cc` and `cook_pnpm` above are exactly that.

## Install

On Linux and macOS install cook with a single command:
```sh
curl -fsSL https://getcook.sh | sh


```

This installs a single Rust binary with
Lua 5.4 and LuaRocks bundled.

Or build from source:
```sh
cargo install --locked --git https://github.com/LioraLabs/cook cook-cli
```

One linking note for source builds. cook statically embeds its own Lua 5.4;
no system Lua is consulted. But a native (C) rock resolves the Lua C API
out of the `cook` executable itself when it loads, so the binary has to
*export* those symbols. A checkout build (`cargo build` under `cli/`) picks
the right linker flag up from `cli/.cargo/config.toml` automatically;
`cargo install` reads config from the directory you run it in, not from the
package, so pass the flag yourself:

```sh
# Linux
RUSTFLAGS="-C link-arg=-rdynamic" \
    cargo install --locked --git https://github.com/LioraLabs/cook cook-cli

# macOS
RUSTFLAGS="-C link-arg=-Wl,-export_dynamic" \
    cargo install --locked --git https://github.com/LioraLabs/cook cook-cli
```

Skip it and everything pure-Lua still works; the failure only shows up when
a C rock loads: dlopen succeeds, then `lua_*` symbol lookup fails with an
error that looks nothing like a linker problem. Sanity-check any build with
`nm -D $(command -v cook) | grep lua_newstate`: exactly one line means the
symbols are exported.

Then:
```sh
mkdir hello-cook && cd hello-cook
cook init
cook
```

## Learn cook

**Start here**

- [Installation](document.md#installation): the one-liner, building from
  source, `cook init`.
- [Your first recipe](document.md#your-first-recipe): a Cookfile from zero;
  register, then execute.
- [Ingredients and the cook step](document.md#ingredients-and-the-cook-step):
  globs, placeholders, fan-out and gather.
- [Connecting recipes](document.md#connecting-recipes): `$<recipe>` references
  that declare the read and record the edge; the colon for pure ordering.

**Beyond the build**

- [Tests](document.md#tests): test results are content-keyed to what they
  consume; unchanged code doesn't retest.
- [Chores](document.md#chores): run-every-time side effects with a real TTY
  and real parameters.
- [Configuration](document.md#configuration): declared knobs, named overlays
  (`cook app @release`), and no invisible inputs.

**Data shapes the build**

- [Probes](document.md#probes-and-data-driven-fan-out): named, cached values
  the graph can see: strings, JSON, tool identities, environment.
- [The `files` producer](document.md#caching-and-cache-trust): a sealable
  per-file manifest for inputs your ingredients line can't hold.

**The cache**

- [Caching and cache trust](document.md#caching-and-cache-trust): one key
  from what you declared; `seal`, `local`, `pinned`, `nondet`.
- [`cook why` and `cook cache verify`](document.md#caching-and-cache-trust):
  every miss attributable, every divergence catchable.
- [Sharing a cache across a team](docs/shared-cache.md): the store is a
  directory; point everyone at one path, no server to run.

**Lua underneath**

- [Dropping into Lua](document.md#dropping-into-lua): `>{ ... }` bodies,
  `register` blocks, `cook.add_unit`, and `cook emit-lua` to see the
  lowering.

**At scale**

- [Modules](document.md#modules-and-composition): distributed through
  LuaRocks, pinned in `cook.lock`; `cook_cc` for C and C++, `cook_pnpm` for
  pnpm monorepos.
- [Workspaces and directory hopping](document.md#workspaces-and-directory-hopping):
  per-subtree Cookfiles joined by `import`; run cook from any directory
  without splitting the cache.
- [`cook affected`](document.md#cook-affected): CI that builds and tests only
  the slice a PR touched.

**Day to day**

- [The cook CLI](document.md#the-cook-cli): `cook menu`, `cook serve` to
  watch and re-run, the `cook dag` TUI, `cook logs`, and tab completion
  served by the binary itself.

**Reference**

- [The Cook Standard](standard/): the authoritative specification; RFC-2119
  keywords, formal grammar appendix.
- [Examples](examples/): runnable projects, arranged in learning order.

## Chef's note

cook is pre-1.0 software. If this friendly tour and the Standard disagree,
the Standard wins, and the README has a bug. Every claim above that carries
a number ships with the script that produced it.

## License

Apache-2.0. See [LICENSE](LICENSE).
