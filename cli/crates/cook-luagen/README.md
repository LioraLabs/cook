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
  `CONFIG_DISPATCH_NAME`, `MAIN_PROGRAM_NAME`, `QUOTE_PARAM_NAME`, and the
  `MemberSourceDescriptor` shape plus its key constants, which
  `cook-register`'s `parse_member_source_meta` reads back (COOK-390). The
  emitter and the consumer of each literal are one declaration apart. For
  `__probe_subst` the whole call comes from there too — `probe_subst_call`,
  because pairing the shared name with a privately spelled receiver and escape
  at three sites is the same drift one step out (COOK-440).
- Text this crate embeds in the generated program is quoted and escaped by
  `cook_contracts::lua_string`, called by its own name: `literal` where the
  value IS the literal, `escape_double_quoted` where a larger template supplies
  the quotes around it. There is no crate-local escaper and deliberately no
  crate-local alias for the contract one — a rename would put shared law beyond
  the reach of a grep for it, which is how this crate and `cook-register` came
  to disagree about carriage returns (COOK-398, COOK-440). Long-bracket
  wrapping (`long_bracket::wrap_lua_string`, `lua_chunk_literal`) stays here:
  choosing a bracket level no inner close can match is a lowering choice, not a
  rule two crates must agree on.
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

## Boundary history

- **`probe::lower_produce` used to author probe semantics as program text.**
  Its `tools { }` arm emitted Lua that shelled out to `command -v` and
  `sha256sum … | cut -d' ' -f1` to build `{ NAME = { hash = … } }` — a second
  implementation of an identity the probe's own fingerprint already computed
  in Rust, in a different language, with a different resolver, at a different
  moment in the run, agreeing only because both happened to land on
  lowercase-hex SHA-256. It also could not run on a host without GNU
  coreutils. CS-0214 retired it: the arm now emits the reserved
  `@tools-identity` sentinel and the engine synthesises the value from the
  same `inputs.tools` pairs the fingerprint folds, exactly as CS-0148 did for
  `files { }`.

  The general lesson is the one the crate's charter already states: when this
  crate would have to *decide* what a value is, the emission is a declaration
  and the decision belongs to whoever owns the value. Emitting a program that
  computes it is how the decision gets implemented twice.

## Relationship to `cook-contracts`

`cook-contracts` owns what a placeholder IS: the sigil grammar, the scanner,
the substitution rendering, the accessor set, the quoting classification, the
registration names. This crate owns what a placeholder BECOMES in
register-phase Lua, which is why `sigil.rs` is now a re-export: CS-0074 kept
parse and render together, CS-0188 removed the second Lua-emitting consumer,
and CS-0195 deleted the last render. Two phases needing the parse and one
needing the render is exactly the split the shared kernel exists for.
