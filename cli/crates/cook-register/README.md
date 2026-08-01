# cook-register

`cook-register` runs a Cookfile's generated Lua far enough to know every unit
of work it declares, and no further.

One entry point, `register_cookfile`, returns one `RegisteredCookfile`: the
recipes it registered, the work units each body captured, the probes, and the
variables the config blocks resolved.

## How it does that well

- **Bodies run; commands do not.** `cook.exec` and `cook.interactive` append a
  `CapturedUnit` instead of spawning (`capture.rs`). The single deliberate
  exception is `cook.sh`, whose return value drives author control flow, and it
  goes through `cook-shell`'s one primitive so its failure text is built by the
  same code the execute phase's `cook.sh` uses (CS-0188, COOK-377). It also
  does not disarm `cook-fingerprint`'s stat memo, and says why: capture mode has
  nothing for the memo to have gone stale against.
- **One installation of the API surface, for both passes.** `install_all_apis`
  is what `register_cookfile` and `list_names` both call, so `cook menu` cannot
  list a Cookfile a build would reject or miss a verb a build would register.
  The two used to install the core separately, which is how they drifted
  (COOK-396).
- **Register order is a demand-driven DFS, not a sort.** The topological sort is
  seed order only; `BodyDriver` is the authority, because `cook.require_recipe`
  declares an edge while a body is running and no sort computed beforehand can
  know it (CS-0144). `VisitState` carries `forced` on the state rather than in a
  parallel set, and keeps `Skipped` and `Failed` distinct from `Visited`. That
  enum has been the site of three bugs in one family, every one of them a state
  that answered fewer questions than the code asked of it.
- **The probe pre-pass calls the evaluator the executor calls.**
  `cook_probe::eval::evaluate`, with `RegisterVmRunner` supplying the one
  genuinely phase-specific step (`engine.rs`). Fingerprinting, CS-0178
  keylessness, cache lookup and publish, and the CS-0102 local copy have one
  implementation. Before COOK-359 that sequence existed twice here, and this
  side's cache block turned out never to have run at all.
- **A unit's identity is blind to where it was declared, on purpose.**
  `build_local_cache_key` takes `_cookfile_path` and `_recipe` and has never
  used either, so moving a test within a recipe or a recipe between Cookfiles
  does not bust its cache (§17.4, CS-0186). The effective seal key set *is*
  folded in, because without it `test { ./run } seal toolchain` and a bare
  `test { ./run }` are one identity, and they then invalidate each other on
  every run: the permanent churn CS-0169 exists to refuse.
- **The one hash both sides of the cache use is the one function.**
  `command_hash` is `cook_fingerprint::hash_str`, which is what
  `cook-fingerprint`'s `check.rs` compares with. The local twin that used to
  live here was drifted by construction (COOK-396).
- **Nothing is coerced, and nothing removed goes quietly nil.** Every
  `cook.add_unit` field is type-checked and a wrong type is a diagnostic naming
  the API, the expected type, and what arrived (CS-0127). A removed name raises
  and names its replacement: `cook.add_test` (CS-0185), `cook.cache` (CS-0136),
  `suite` (CS-0185). An unbound name would surface as `attempt to call a nil
  value`, which reads as a Cook bug and points nowhere.
- **The config sandbox swaps `_ENV`; it does not strip globals.** The whole pass
  shares one VM, and recipe bodies legitimately keep `os.execute`, so
  `config_sandbox.rs` restricts only the generated dispatcher's environment
  (CS-0163). `_ENV` governs free variables only, so a `use`-imported module
  handle bound as a local stays reachable as an upvalue, which is what makes the
  sandbox usable rather than merely safe.
- **A declared variable cannot be moved out from under the key that consulted
  it.** The store lives in the Lua registry with its metatable hidden; the only
  Lua-reachable handles are the read-only `var` proxy and `cook.require_var`
  (CS-0172). `--set` naming an undeclared variable is an error with the closest
  declared names, where before CS-0172 it silently invented one.
- **Diagnostics point at a line the author can open, or say who to blame
  instead.** `chore_site` keys on `origin` rather than on `line == 0`: a
  module-registered chore has a real line number pointing into a file the author
  did not write, and a surface chore whose line failed to resolve is still a
  Cookfile declaration.

## What it does not do

It does not parse or lower a Cookfile. `cook-lang` parses; `cook-luagen`
lowers; this crate only ever sees generated Lua and the contract names codegen
emits (`cook_contracts::registration`).

It does not run the work, schedule it, build the cross-Cookfile DAG, or check
and replay a cache entry. It computes each unit's `CacheMeta` and hands it over;
`cook-engine` decides what happens next.

It does not own the probe lifecycle (`cook-probe`), the cache backend or store
layout (`cook-cache`), the quoting law (`cook_contracts::quoting`, COOK-389),
the module candidate order or Lua search-path composition
(`cook_contracts::layout`, COOK-393), or the Lua/JSON codec
(`cook_lua_stdlib::json_codec`, COOK-388). It supplies the register VM to the
first and reads the rest.

It does not own the both-phase Lua surface. `fs.*`, `path.*`, `cook.platform`,
the codecs, and `cook.tools.id` are installed from `cook-lua-stdlib` so the
worker VMs in `cook-luaotp` install byte-identical closures (CS-0044, CS-0123,
CS-0158). A surface that behaves differently in the two phases is the failure
this arrangement exists to make impossible.

## Decisions still implemented twice

Findable, per the deliberate-copy protocol, and none of these has an agreement
test. They are recorded here so the next audit's grep lands on them:

- **The probe-produce lowering.** `engine.rs:2094` and
  `cook-luaotp/src/pool.rs:1537` each build `@probe:{key}` as the chunk name and
  wrap the body in `return (function()\n…\nend)()`. Both ends must agree or a
  produce body's reported error lines shift between phases. It is pure string
  law and `cook-contracts` would take it.
- **Escaping a Rust string into a Lua literal**, and it has already drifted.
  `engine.rs:1667` escapes `\`, `"`, `\n`, `\r`, and NUL; the twin at
  `cook-luagen/src/lua_string.rs:1` escapes only `\`, `"`, and `\n`. A value
  carrying a carriage return is a chore-parameter prelude that loads and a
  generated command that does not.
- **The `cook.load_module` sequence.** `module_loader.rs:92` and
  `pool.rs:719` each memoize, detect cycles, evaluate, and call `init()`.
  COOK-393 unified the candidate list and the search-path composition, not the
  loader around them; the register side memoizes by module name and the worker
  side by `<cwd>::<name>`.
- **The `cook.cache` renamed-namespace stub**, verbatim in `module_loader.rs:377`
  and `pool.rs:1079`.

Two smaller exceptions to rules this crate otherwise keeps: `engine.rs:2050` and
`engine.rs:2696` print warnings with `eprintln!` although `RegisteredCookfile`
already carries a `warnings` field for exactly that; and `observing_identity`
(`unit_api.rs:1435`) is cache-identity law hashed against a direct
`xxhash-rust` dependency, where the stratum rule puts hashing law in
`cook-fingerprint`. It has one caller today, so it is not yet a twin.

## Relationship to `cook-contracts`

`cook-contracts` says what a registered thing **is**: `CapturedUnit`,
`RecipeUnits`, `CacheMeta`, `ProbeUnit`, the registration names codegen and this
crate both spell, the sigil grammar, and the canonical renderings. It is
forbidden stateful standard-library access by its own layout test, so it can
describe a work unit but can never discover one.

`cook-register` discovers them. The dividing question is whether an answer needs
a Lua VM, a filesystem, or a process: if it does not, it belongs upstream.
