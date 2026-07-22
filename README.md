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

Got a lot of chefs complicating your build:
[Cargo](https://doc.rust-lang.org/cargo/), [CMake](https://cmake.org/),
[pnpm](https://pnpm.io/), codegen, and a `scripts/` directory nobody will admit
to writing, each with its own idea of what's up to date? cook is one kitchen
for the whole crew: a build system that brings their work into one dependency
graph with one [content-addressed cache](#the-part-cook-was-built-for-the-cache).
It keeps the recipe ergonomics of
[`just`](https://github.com/casey/just) and the dependency graph of
[`make`](https://www.gnu.org/software/make/), and swaps macro-heavy
configuration for a [Lua](https://www.lua.org/)-backed DSL. You describe
artifacts in a [`Cookfile`](document.md#your-first-recipe); cook runs exactly
the work required by what changed.

Heard enough? Try it for yourself on Linux and macOS.

```sh
curl -fsSL https://getcook.sh | sh
```

## Real builds

We added a Cookfile to two large projects. Here's what happened.

### Cap: Turborepo, replaced

[Cap](https://github.com/CapSoftware/Cap) is a 20k-star open-source screen
recorder: a pnpm monorepo with eleven Turbo tasks and 48 Rust crates beside
it. [cook-cap](https://github.com/LioraLabs/cook-cap) swaps
[Turborepo](https://turborepo.dev/) for
[`cook_pnpm`](document.md#modules-and-composition) and benchmarks the swap. The
experiment: two different edits to the same file in `@cap/env`, a package
almost everything depends on.

| Edit to `@cap/env` | Turborepo | cook |
| --- | --- | --- |
| Add a comment | rebuilds 7 of 11 tasks, **~47s** | rebuilds 1 task, **~1s** |
| Add a real export | rebuilds the same 7 tasks, ~47s | rebuilds `@cap/env` plus the one app that consumes it, ~30s |

In [Cap's Turbo build](https://github.com/LioraLabs/cook-cap), changing a
package input dirties its downstream tasks, whether or not the package produces
a different artifact. cook keys downstream work on what it actually consumes,
so a comment that compiles away becomes a cache hit everywhere downstream.
[Cap's Cookfile](https://github.com/LioraLabs/cook-cap/blob/main/Cookfile):

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

# Cargo stays Cargo; its workspace joins the same dependency graph.
recipe rust-bins
    seal rust:sources
    cook "target/debug/scap-targets" { cargo build -q -p scap-targets }
```

Cargo still owns the Rust workspace's internal graph. cook lets its builds,
checks, and tests participate in the same dependency graph as the pnpm
workspace.

Every number above is reproduced by
[`bench/compare.sh`](https://github.com/LioraLabs/cook-cap/blob/main/bench/compare.sh)
in that repo. Run it yourself.

### Doom 3: one graph from compiler to demo map

[cook-dhewm3](https://github.com/LioraLabs/cook-dhewm3) builds the Doom 3
source port with [`cook_cc`](document.md#modules-and-composition): a 427-node
graph producing the engine binary,
the gameplay `.so` plugins, and the asset tools (`dmap`, the AAS compiler),
then uses the freshly built `dmap` binary to compile a playable demo map in
the same graph. Clone, `cook`, play.
`compile_commands.json` for clangd falls out of the same graph.

## The five-minute tour

This tour uses [Inkscape](https://inkscape.org/) to rasterize SVGs and
[ImageMagick](https://imagemagick.org/download/) to assemble the sprite sheet.

Say you're handed a pile of SVGs and you need a PNG sprite sheet.

```cook
recipe sprite-sheet
    ingredients "images/*.svg"
    cook "build/sprites/$<in.stem>.png" {
        inkscape $<in> --export-filename=$<out>
    }
    cook "build/spritesheet.png" {
        magick montage $<in> -tile x1 -geometry +0+0 $<out>
    }
```

Each `cook` step declares a target. `$<in.stem>` varies per input, so the
first step fans out: one unit per SVG, parallel across your cores, each
cached under its own key. The second step's target is static, so it gathers:
every PNG in, one sheet out. You never write the loop or the join.
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

**make or just?** cook was born from a project that used both. make owned the
artifacts and incremental builds; just owned the commands people actually
wanted to run. Both were excellent at their jobs, but every new piece of work
came with the same question: does this belong in the Makefile or the justfile?

cook grew out of wanting one answer. It keeps just's recipe ergonomics and
make's artifact graph, then adds content-addressed reuse across the whole
repository, a shared cache, and Lua when shell is not enough.

**Turborepo?** Change a shared package and Turbo can rebuild every app
downstream, even when the package produces exactly the same artifact. In Cap,
adding one comment triggered seven tasks and about 47 seconds of work. cook
rebuilt the package, saw that its output had not changed, and stopped the
cascade in about one second. It can also put Rust recipes, codegen, and assets
in the same graph as JavaScript.

**CMake?** CMake is powerful inside a native build, but pnpm tasks, code
generators, assets, and other workflows tend to become custom commands around
it. cook has no privileged language or ecosystem. `cook_cc`, `cook_pnpm`, and
modules for project-specific tools all contribute ordinary units to the same
graph and cache.

**[Bazel](https://bazel.build/)?** Bazel and cook start from different
assumptions. Bazel asks a repository to enter a controlled build world, where
rules, toolchains, and declared dependencies enforce reproducibility. cook
starts with the tools a team already uses and connects them into one artifact
graph. Bazel provides stronger enforcement. cook takes a lighter adoption
path, relying on the user to declare the cache determinants, then making the
resulting key inspectable and verifiable.

## The part cook was built for: the cache

cook does not pretend to know your project better than you do. You declare
what matters to your build.

### You own the key

Every unit of work is cached under one content-addressed key, built entirely
from its declared determinants. cook does not quietly fold your machine,
locale, or toolchain into the key. That keeps keys reusable across machines by
default, while making *you* responsible for declaring the determinants that
make that reuse safe.

Need the compiler in the key? Say so:
```cook
probe compiler
    tools { cc }

recipe app
    ingredients "src/*.c"
    seal compiler
    cook "build/$<in.stem>.o" { cc -c $<in> -o $<out> }
```

`probe compiler` identifies the resolved `cc` tool. `seal compiler` includes
that identity in each unit's key, so cached objects are reused only when the
compiler matches. The local cache and the shared store are addressed by that
same key, so a teammate or CI runner reuses your artifact when its declared
determinants produce the same key. The store is a directory: point everyone
at one path, no server to run.

### No mystery misses

You own what goes into the key; cook owns explaining every decision it makes
from that key. Every cache miss has a concrete, inspectable cause. `cook why`
prints every determinant behind every unit's key with its hit or miss status;
on a shared-store miss it diffs your key against what the cached artifact was
actually built from. `cook cache verify` re-runs cached work and reports byte
divergence, which is how you catch a determinant you forgot to declare.

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
Modules distributed through [LuaRocks](https://luarocks.org/) use the same
interface to teach cook entire ecosystems; `cook_cc` and `cook_pnpm` above are
exactly that.

## Install

On Linux and macOS install cook with a single command:
```sh
curl -fsSL https://getcook.sh | sh
```

This installs a single Rust binary with
Lua 5.4 and LuaRocks bundled.

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

- [Source-build linking](document.md#source-build-linking): linker flags for
  loading native Lua rocks from a source-built cook binary.
- [The Cook Standard](standard/): the authoritative specification; RFC-2119
  keywords, formal grammar appendix.
- [Examples](examples/): runnable projects, arranged in learning order.

## Chef's note

cook is pre-1.0 software. If this friendly tour and the Standard disagree,
the Standard wins, and the README has a bug. Every claim above that carries
a number ships with the script that produced it.

## License

Apache-2.0. See [LICENSE](LICENSE).
