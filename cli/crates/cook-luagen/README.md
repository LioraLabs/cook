# cook-luagen

`cook-luagen` lowers a parsed Cookfile into the register-phase Lua program that
declares its work. It emits that program or it refuses; it never emits one that
means something other than what the author wrote.

## How it does that well

- There is one entry point, `generate_checked` (recipe.rs:94). `parse.rs` and
  `workspace.rs` used to call a checked lowering and then a second one,
  lowering the whole Cookfile twice to throw the Lua away and keep the
  warnings, and that second entry point `.expect()`ed on codegen errors: safe
  only because the first call had already run (COOK-357).
- One resolver answers what a `$<IDENT>` means, and every expander asks it.
  The plate/test expander used to hand-roll its own dispatch chain, which is
  why the same ident answered differently in a `test` body than in a `cook`
  body: `$<sys:os>` reported a missing config block, `$<in.bogus>` emitted
  `path.bogus(...)` and surfaced as "attempt to call a nil value", and
  `$<env.HOME>` advised declaring a variable literally named `env.HOME`
  (CS-0184, template.rs:743). `tests/sigil_agreement.rs` pins that by lowering
  the same sigil in both body kinds through real codegen and comparing the
  diagnostics, rather than calling the resolver and comparing it to itself.
- A sigil that cannot be lowered is a returned `CodegenError`, never a marker
  string in the output. Emitted Lua used to carry `[[SIGIL_ERROR: …]]`
  literals that something else grepped for, and the `Literal` output-pattern
  arm passed its pattern through verbatim, so `cook "out/all-$<suffix>.o"`
  wrote a file called `out/all-$` and handed `$<suffix>` to `/bin/sh` as a
  redirect (COOK-188, COOK-357; cook_step.rs:328, template.rs:544).
- A `Step` variant with no codegen arm is a hard error, not a skipped step.
  `Step` is `#[non_exhaustive]`, so the alternative is emitting a recipe that
  silently drops work (`CodegenError::UnknownStep`, recipe.rs:1078).
- Every name in the generated program is a constant from `cook-contracts`, not
  a string literal spelled here: `REGISTER_SURFACE_NAME`,
  `CONFIG_DISPATCH_NAME`, `MAIN_PROGRAM_NAME`, `PROBE_SUBST_NAME`,
  `QUOTE_PARAM_NAME`, and the `MemberSourceDescriptor` shape plus its key
  constants, which `cook-register`'s `parse_member_source_meta` reads back
  (COOK-390). The emitter and the consumer of each literal are one declaration
  apart.
- It composes a shell block through the law and classifies quoting without
  performing it. The hand-rolled `"set -e\n" + join` here was the copy that
  actually reached `/bin/sh`, so a change to `shell_block::compose` would not
  have reached it (COOK-391, recipe.rs:456); CS-0128's `QCtx` lives in
  `cook_contracts::quoting`, this crate emits the tag and `cook-register`
  quotes (COOK-389, template.rs:264).
- A `command` field is always a string expression, never a deferred
  `function() … end`. `cook.add_unit` coerces a non-string command to `""`,
  which silently no-ops the unit: that is the COOK-187 defect, and the fix is
  to never produce the shape. Where a probe reference must stay literal for
  register-time capture is a named `ProbeLowering` parameter rather than a
  convention (template.rs:22).
- Generated lines are aligned to source lines on purpose. Config bodies are
  padded so a runtime error inside one reports its Cookfile line, and a body
  unit reports its first step's line minus the `use`-statement preamble
  (CS-0126/COOK-191, recipe.rs:689 and recipe.rs:546). The residual
  imprecision (a bundle spanning several steps gets an exact line only for the
  first) is documented where it is created, not left to be rediscovered.

## What it does not do

It does not parse: it takes a `cook_lang::ast::Cookfile` and a set of in-scope
recipe names. It does not run Lua; it has no mlua dependency and never
evaluates what it emits. It does not schedule, cache, spawn, or resolve globs:
`cook.resolve_ingredients`, `cook.dep_output`, `cook.prior_outputs`, and
`cook.probes.get` are calls it writes, not work it does. It does not decide
what a probe value means, quote a shell argument, or own any wire format; those
are `cook-probe`, `cook-register`, and `cook-contracts` respectively.

Its validation is deliberately only the part that decides whether an emission
exists: a builtin's mode and output count, a driver-less accessor, an
incoherent multi-output driver set, a literal-output first step in a fan-out
recipe. A rule that needs the register phase to know the answer is deferred to
it, and says so (`cook.require_var` for declared variables; the register
pre-pass for probe key-versus-field resolution, COOK-190).

## Boundary debt

Two things sit here that the crate name does not cover, and both are named
rather than defended:

- **`lua_var` + `lua_scan` are static analysis of Lua, not generation**
  (~640 LoC). `scan_var_reads` finds the cache determinants a `>{ … }` body
  reads; `scan_probe_reads` finds its literal `cook.probes.get("k")` calls.
  The second has exactly one consumer, `cook-register`'s `unit_api.rs:866`, so
  a runtime crate depends on the codegen crate for a text scanner. It answers
  the same question the sigil scanner in `cook_contracts::sigil` answers for
  the shell surface ("what does this body consume?"), and it is pure, so by
  the admission bar its home is `cook-contracts`, beside its twin.
- **`probe::lower_produce` authors probe semantics as program text**
  (probe.rs:104). The `tools { }` arm emits Lua that shells out to
  `command -v` and `sha256sum … | cut -d' ' -f1` to build
  `{ NAME = { hash = … } }`. `cook.tools.id` computes the same identity in
  Rust through `cook_fingerprint::tool_identity`
  (`cook-lua-stdlib/src/tools_api.rs:20`). Two implementations of one
  decision, agreeing today only because both happen to be lowercase-hex
  sha256, with no agreement test and no comment on either naming the other.

## Relationship to `cook-contracts`

`cook-contracts` owns what a placeholder IS: the sigil grammar, the scanner,
the substitution rendering, the accessor set, the quoting classification, the
registration names. This crate owns what a placeholder BECOMES in
register-phase Lua, which is why `sigil.rs` is now a re-export: CS-0074 kept
parse and render together, CS-0188 removed the second Lua-emitting consumer,
and CS-0195 deleted the last render. Two phases needing the parse and one
needing the render is exactly the split the shared kernel exists for.
