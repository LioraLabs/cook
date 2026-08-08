#!/usr/bin/env node
// Tree-sitter conformance harness.
//
// Walks `standard/conformance/{positive,negative}` and asserts that
// `tree-sitter-cook` agrees with the Cook Standard at the syntactic
// level: every positive Cookfile parses without ERROR/MISSING nodes,
// and every syntactic negative Cookfile produces at least one
// ERROR/MISSING node.
//
// Cases listed in `SEMANTIC_ONLY_NEGATIVES` require information that a
// context-free syntax tree does not contain: declaration ordering or
// uniqueness, graph/runtime state, source resolution, or templating and
// iteration-mode analysis. Each entry names its enforcement phase.
//
// Override the corpus path with COOK_CONFORMANCE_CORPUS=/path/to/dir.
// Default is `../standard/conformance/` relative to this script.

import { execFile } from 'node:child_process';
import assert from 'node:assert/strict';
import { promisify } from 'node:util';
import { readdir, stat, mkdir } from 'node:fs/promises';
import { dirname, join, basename } from 'node:path';
import { fileURLToPath } from 'node:url';

const exec = promisify(execFile);
const HERE = dirname(fileURLToPath(import.meta.url));
const REPO = dirname(HERE);
// Parser compiled from THIS tree's grammar; see the --lib-path note in parseFile.
const PARSER_LIB = join(REPO, 'build', 'parser', 'cook.so');

const SEMANTIC_ONLY_NEGATIVES = new Map([
  ['003-use-after-recipe',
   'top-level ordering rule (App. A.2) — semantic, not syntactic'],
  ['004-duplicate-ingredients',
   'at-most-one-ingredients rule (App. A.3) — semantic, not syntactic'],
  ['006-accessor-in-cook-body',
   'App. A.4 iteration coherence — codegen templating check across recipe sources'],
  // CS-0095: `ingredients <probe>` parses cleanly; these rejections are
  // register-phase (probe declared? array-valued? non-artifact dep?).
  ['ingredients-probe-undeclared',
   'CS-0095: undeclared probe key — register-phase rejection, not syntactic'],
  ['ingredients-probe-non-array',
   'CS-0095: non-array probe value — register-phase rejection, not syntactic'],
  ['ingredients-probe-files-kind',
   'COOK-353: files probe in driver position — register-phase rejection, not syntactic'],
  ['ingredients-probe-artifact-dep',
   'CS-0095: probe member source with artifact dep — register-phase rejection, not syntactic'],
  // CS-0206: the `use` path form. FOUR of its six negatives ARE syntactic and
  // are deliberately absent from this list — `..`, a leading `/`, the `//`
  // sigil and a third argument are all shapes the token cannot take, and
  // `test/corpus/declarations.txt` pins each. These two are not, and each
  // would otherwise be reported green off an unrelated CS-0134 error in the
  // fixture's recipe body rather than off the rule it names.
  ['068-use-path-derived-alias-rejected',
   'CS-0206: the alias DERIVED from a path basename must be a Lua identifier — a lexer check over a derivation, and the identical path text is legal one position over (`use fmt ./build/9lives.lua`), so a grammar that refused the path would refuse a legal Cookfile'],
  ['069-use-duplicate-alias-rejected',
   'CS-0206: two `use` declarations binding one identifier to different targets — a whole-Cookfile check in the parser, invisible to a per-declaration grammar rule'],
  ['cs0187-retired-file-prefix',
   'CS-0187: the retired `file:` prefix — codegen-phase rejection; the token is syntactically an ordinary placeholder ident'],
  // CS-0022 Phase G: codegen-only rejections — Cookfile parses cleanly,
  // rejection is enforced by cook-luagen::generate_with_names_checked.
  ['017-bare-stem-rejected',
   'CS-0022: bare {stem} in output pattern — codegen rejection, not syntactic'],
  ['019-out-n-in-single-output-rejected',
   'CS-0022: {out_N} in single-output step — codegen rejection, not syntactic'],
  ['020-out-bare-in-multi-output-rejected',
   'CS-0022: {out} in multi-output step — codegen rejection, not syntactic'],
  ['021-mixed-driver-multi-output-rejected',
   'CS-0022: mixed iteration drivers in multi-output — codegen rejection, not syntactic'],
  ['022-lib-accessor-in-cook-body-rejected',
   'CS-0022: {lib.ACCESSOR} inside a cook-step body — codegen rejection, not syntactic'],
  ['023-multi-output-one-to-one-mixed-rejected',
   'CS-0022: mixed one-to-one + literal output patterns — codegen rejection, not syntactic'],
  ['025-in-accessor-rejected-in-many-to-one',
   'App. A.4 iteration coherence — codegen determines many-to-one mode from outputs'],
  ['032-test-empty-source-rejected',
   'App. A.4 test mode coherence — codegen requires a resolved iteration source'],
  // CS-0035: env reserved-word check — closed: `env.*` recipe names are
  // rejected by the grammar's `_decl_name` (no-dots rule, COOK-55) which
  // means `env.foo` produces ERROR before any reserved-name check fires.
  // CS-0033: sigil-placeholder semantic rules.
  ['038-out-zero-rejected-sigil',
   'CS-0033: `$<out_0>` (zero index) — codegen rejection, not syntactic'],
  // CS-0035 use_name LUA_IDENT constraint — closed by COOK-55: 040/041/042
  // are now grammar-rejected by `_lua_ident_name`.
  //
  // Recipe / chore semantic-only rules (CS-0022 / App. A.3 / App. A.5
  // — Rust parser + codegen territory, not syntactic):
  ['051-bare-recipe-ref-in-output-pattern-rejected',
   'bare {NAME} ref inside cook_step output pattern — codegen rejection'],
  ['052-directory-input-rejected',
   'directory input rejection — register/execute-time semantic, not syntactic'],
  ['053-duplicate-recipe-name-rejected',
   'App. A.2 duplicate recipe-vs-recipe name — parse-time semantic, not syntactic'],
  ['054-duplicate-chore-name-rejected',
   'App. A.2 duplicate chore-vs-chore name — parse-time semantic, not syntactic'],
  ['055-recipe-chore-name-collision-rejected',
   'App. A.2 recipe-vs-chore name collision — parse-time semantic, not syntactic'],
  ['recipe-name-collision-surface-vs-dynamic',
   'recipe name collision between surface and dynamic — register-time semantic'],
  // CS-0143: `cook.recipe`'s `origin` metadata field is any Lua value
  // syntactically; only the register-phase `parse_origin_meta` check knows
  // it must be a (non-empty) string.
  ['recipe-origin-not-a-string',
   '§22.3 cook.recipe `origin` field type check — register-time semantic, not syntactic'],
  // CS-0185: a spec-table field is syntactically ordinary either way; only the
  // register pass knows a test unit declares no outputs and accepts no `suite`
  // (§22.4). CS-0153's `add-unit-step-kind-test-rejected` was withdrawn here —
  // `step_kind = "test"` is now accepted.
  ['test-unit-declares-output-rejected',
   'CS-0185: output on a step_kind = "test" unit — register-phase rejection, not syntactic'],
  ['add-test-removed',
   'CS-0185: cook.add_test was removed — register-phase rejection, not syntactic'],
  ['test-unit-suite-rejected',
   'CS-0185: suite on a step_kind = "test" unit — register-phase rejection, not syntactic'],
  // CS-0155: a literal-output first cook step in an ingredients <probe>
  // recipe parses cleanly; only the register pass knows there is no
  // preceding step whose outputs it could gather (§8.4.1).
  ['probe-fanout-literal-first-step',
   'CS-0155: literal-output first step in a probe-driven recipe — register-phase rejection, not syntactic'],
  // §28 cc-module semantic rules (§28.3 — execute-phase / probe-time
  // rejections; the Cookfile parses cleanly):
  ['cc-check-bad-flag',
   '§28.3.14 cc.checks.has_compile_flag — execute-time probe rejection, not syntactic'],
  ['cc-config-header-missing-var',
   '§28.3.15 cc.config_header — missing-var rejection at render time, not syntactic'],
  ['cc-find-conflicting-opts',
   '§28.3.13 cc.find — conflicting-opts rejection at register time, not syntactic'],
  ['cc-find-missing-on-build',
   '§28.3.13/§28.3.14 cc.find — demand-driven build-time rejection, not syntactic'],
  // §22 probe-unit semantic rules (register-time validation):
  ['probe-cycle',
   '§22.5 cook.probe — dependency cycle detection, register-time semantic'],
  ['probe-duplicate-key',
   '§22.5 cook.probe — duplicate-key detection, register-time semantic'],
  ['probe-unresolved-require',
   '§22.5 cook.probe — unresolved require detection, register-time semantic'],
  // §22.7 cook.recipe_name semantic rule (register-time validation):
  ['recipe-name-outside-recipe-body-rejected',
   '§22.7 cook.recipe_name — outside-a-recipe-body detection, register-time semantic'],
  // CS-0144: a `cook.require_recipe(name)` call in a `register` block is an
  // ordinary bare module call syntactically; only the register-phase
  // body-slot check knows there is no enclosing recipe body.
  ['require-recipe-outside-recipe-body-rejected',
   '§22.8 cook.require_recipe — outside-a-recipe-body detection, register-time semantic'],
  // CS-0149: `cook.recipe(...)` called from inside a callback queued via
  // `cook.on_register_complete` is an ordinary register-phase Lua call
  // syntactically; only the register-phase finalizer-drain step (which
  // knows the recipe set is already closed) rejects it.
  ['on-register-complete-mints-recipe-rejected',
   'CS-0149: cook.recipe from a queued finalizer callback — register-phase rejection, not syntactic'],
  // §7.1.1 chore-parameter semantic rules — Rust parser enforces;
  // tree-sitter accepts any ordering / count / reserved-name shape.
  // The dot-ban and the no-default-on-variadic rule ARE syntactic and
  // remain enforced by the grammar (param-name regex / variadic_param
  // production has no `=default` arm).
  ['chore-param-defaulted-before-required',
   '§7.1.1 required-before-defaulted ordering — parse-time semantic'],
  ['chore-param-duplicate-name',
   '§7.1.1 duplicate-parameter detection — parse-time semantic'],
  ['chore-param-multiple-variadics',
   '§7.1.1 at-most-one-variadic rule — parse-time semantic'],
  ['chore-param-variadic-not-last',
   '§7.1.1 variadic-tail rule — parse-time semantic'],
  // CS-0163 / CS-0164: a `config` body is opaque LUA_SOURCE to the grammar, so
  // every config rejection is register-phase — the sandbox `__index` (§5.3.2)
  // or the absent `env` sink (§5.3.1) fires when the config dispatcher runs
  // the body. `cook-register/tests/conformance.rs` baselines the diagnostics.
  ['config-sandbox-rejects-os',
   'CS-0163: `os` unreachable in a sandboxed config body — register-phase rejection, not syntactic'],
  ['config-sandbox-rejects-io',
   'CS-0163: `io` unreachable in a sandboxed config body — register-phase rejection, not syntactic'],
  ['config-env-output-rejected',
   'CS-0164: `env.NAME` config output sink (now `var.NAME`) — register-phase rejection, not syntactic'],
  // CS-0172: the declared-variable namespace is a register-phase artefact —
  // the frozen keyset, the value's Lua type, and the write window all become
  // knowable only once the config dispatcher has run. Each of these three
  // Cookfiles is syntactically ordinary: an undeclared `$<NAME>` is the same
  // placeholder shape as a declared one, a table-valued assignment is opaque
  // config-body Lua, and a `var.NAME = …` line in a `register` block is
  // opaque register-body Lua.
  ['config-var-ambient-env-rejected',
   'CS-0172: `$<HOME>` against the frozen declared keyset — register-phase rejection, not syntactic'],
  ['config-var-table-value-rejected',
   'CS-0172: table-valued declared variable — register-phase type rejection, not syntactic'],
  ['config-var-write-outside-config-rejected',
   'CS-0172: `var` write outside a config body — register-phase rejection, not syntactic'],
  // CS-0175: a `consumes` pattern is a string inside an opaque `register`-block
  // Lua call. Only the register pass compiles it, which is where an
  // unparseable glob is rejected (§17.4 rule 1's narrowing safeguards).
  ['test-unit-consumes-bad-glob',
   'CS-0175: unparseable `consumes` glob on cook.add_test — register-phase rejection, not syntactic'],
  // CS-0176: the Cookfile is a bare `use` plus an ordinary recipe. The
  // rejection fires when the loaded module registers a chore whose name
  // carries no dotted module prefix (§22.11), which is register-phase.
  ['cook-chore-undotted-rejected',
   'CS-0176: undotted `cook.chore` name — register-phase rejection, not syntactic'],
  // CS-0184 §10.2.4: a placeholder is one syntactic shape wherever it appears.
  // What a given ident MEANS, and whether the position it sits in can honour
  // that meaning, is decided by the codegen resolver — so both of these parse
  // exactly like a legal placeholder and are refused a phase later.
  ['061-probe-ref-in-test-command',
   'CS-0184 §10.2.4: probe-value ref in a verbatim test command — codegen refusal, not syntactic'],
  ['062-retired-env-prefix-in-test-body',
   'CS-0184 §10.2.4: retired `env.` prefix in a test body — codegen rejection, not syntactic'],
]);

function corpusRoot() {
  if (process.env.COOK_CONFORMANCE_CORPUS) {
    return process.env.COOK_CONFORMANCE_CORPUS;
  }
  return join(REPO, '..', 'standard', 'conformance');
}

async function listCases(sub) {
  const dir = join(corpusRoot(), sub);
  const entries = await readdir(dir, { withFileTypes: true });
  return entries
    .filter((e) => e.isDirectory())
    .map((e) => e.name)
    .sort();
}

async function parseCase(file) {
  const hasRecoveryNode = (output) =>
    /\((?:ERROR|MISSING)\b/.test(output) ||
    // tree-sitter renders an inserted token through an alias as a zero-width
    // named node rather than spelling `MISSING` (for example the required
    // identifier in `tools {}`). It is still parser recovery, not acceptance.
    /\([^\n]* \[(\d+), (\d+)\] - \[\1, \2\]\)/.test(output);
  const classify = (stdout, stderr, code = 0, message = '') => {
    const output = stdout + stderr;
    const parsed = /^\(source_file\b/m.test(stdout);
    if (!parsed) {
      return {
        parsed: false,
        ok: false,
        code,
        output: output || message || 'tree-sitter parse produced no parse tree',
      };
    }
    return { parsed: true, ok: !hasRecoveryNode(stdout), code, output };
  };

  try {
    // `--lib-path` is load-bearing, not cosmetic. Without an explicit parser,
    // `tree-sitter parse` selects a grammar through the user's global
    // ~/.config/tree-sitter/config.json `parser-directories`, which hold
    // ABSOLUTE paths — so the harness graded whatever cook grammar that
    // config resolved (typically a cached ~/.cache/tree-sitter/lib/cook.so
    // built from some other checkout), never the tree it was invoked in.
    // Every fixture here is named `Cookfile`, matching neither
    // `file-types: ["cook"]` nor the `^#.*[Cc]ookfile` first-line regex, so
    // the fallback always engaged.
    //
    // That misreports in BOTH directions: a false RED when the resolved
    // parser lags this tree, and — the dangerous one for a release gate — a
    // false GREEN when it runs ahead. `buildParser()` compiles this tree's
    // grammar once up front and every fixture is parsed against it, so a
    // clone, a worktree, and CI all grade the same bytes.
    //
    // `--grammar-path` would also pin it but implies --rebuild per
    // invocation: 264 fixtures went from 0.7s to 98s. Build once instead.
    const { stdout, stderr } = await exec(
      'tree-sitter',
      ['parse', '--lib-path', PARSER_LIB, '--lang-name', 'cook', file],
      { cwd: REPO },
    );
    return classify(stdout, stderr);
  } catch (err) {
    return classify(
      err.stdout || '',
      err.stderr || '',
      err.code ?? null,
      err.message || '',
    );
  }
}

function fmtBlock(text, indent = '  ') {
  return text
    .trimEnd()
    .split('\n')
    .map((l) => indent + l)
    .join('\n');
}

async function runPositives() {
  const cases = await listCases('positive');
  const failures = [];
  const notes = [];
  for (const name of cases) {
    const file = join(corpusRoot(), 'positive', name, 'Cookfile');
    const result = await parseCase(file);
    if (!result.parsed) {
      failures.push({ name, output: `infrastructure failure: ${result.output}` });
      console.log(`ERROR  positive/${name} (tree-sitter invocation failed)`);
      continue;
    }
    if (result.ok) {
      console.log(`OK     positive/${name}`);
      continue;
    }
    failures.push({ name, output: result.output });
    console.log(`FAIL   positive/${name}`);
  }
  return { failures, notes };
}

async function runNegatives() {
  const cases = await listCases('negative');
  const failures = [];
  const notes = [];
  for (const name of cases) {
    const file = join(corpusRoot(), 'negative', name, 'Cookfile');
    const result = await parseCase(file);
    const skip = SEMANTIC_ONLY_NEGATIVES.get(name);
    if (!result.parsed) {
      failures.push({ name, output: `infrastructure failure: ${result.output}` });
      console.log(`ERROR  negative/${name} (tree-sitter invocation failed)`);
      continue;
    }
    if (skip) {
      // Semantic-only: tree-sitter is expected to accept.
      if (result.ok) {
        console.log(`SKIP   negative/${name} (${skip})`);
      } else {
        // Tree-sitter rejected something we expected it to accept.
        // Post-CS-0086 (v0.12 audit), SEMANTIC_ONLY_NEGATIVES is
        // expected to be tight; if a skip entry's fixture now rejects
        // at the grammar level, the entry has been overtaken by a
        // grammar tightening and the skip list should shrink.
        console.log(`NOTE   negative/${name} now rejected — remove from SEMANTIC_ONLY_NEGATIVES`);
        notes.push({ name, output: 'SEMANTIC_ONLY_NEGATIVES entry now rejects at grammar level' });
      }
      continue;
    }
    if (result.ok) {
      failures.push({
        name,
        output: 'tree-sitter accepted, expected ERROR/MISSING',
      });
      console.log(`FAIL   negative/${name} (accepted, expected reject)`);
    } else {
      console.log(`OK     negative/${name} (rejected)`);
    }
  }
  return { failures, notes };
}

/// Compile this tree's grammar to a dynamic library so every fixture is graded
/// against the working tree rather than the ambient global config.
async function buildParser() {
  await mkdir(dirname(PARSER_LIB), { recursive: true });
  try {
    await exec('tree-sitter', ['build', REPO, '-o', PARSER_LIB], { cwd: REPO });
  } catch (err) {
    console.error(`failed to build parser from ${REPO}:`);
    console.error(err.stderr || err.message || String(err));
    process.exit(2);
  }
}

/// Grade exactly one fixture, named by its Cookfile path, and exit 0/1.
///
/// This is the entry point the `conformance` recipe fans out over (CS-0182):
/// one cached pass/fail unit per fixture, each keyed on that fixture alone, so
/// editing one re-runs one. The whole-corpus `main()` below is unchanged and
/// stays the way to grade everything in a single process (CI, manual runs).
///
/// The parser is NOT built here. Building it 284 times would cost more than the
/// grading does, so the recipe declares `parser-lib` as a dependency and this
/// path asserts the artifact rather than producing it — the fan-out unit's job
/// is to grade a fixture, not to compile a grammar.
async function runOneCase(cookfilePath) {
  const name = basename(dirname(cookfilePath));
  const kind = basename(dirname(dirname(cookfilePath)));

  if (kind !== 'positive' && kind !== 'negative') {
    console.error(`--case: expected a path under positive/ or negative/, got ${cookfilePath}`);
    process.exit(2);
  }
  try {
    await stat(PARSER_LIB);
  } catch {
    console.error(`--case: parser library not found: ${PARSER_LIB}`);
    console.error(`hint: this mode expects the parser to be built already (cook ts.parser-lib)`);
    process.exit(2);
  }

  const result = await parseCase(cookfilePath);

  if (!result.parsed) {
    console.log(`ERROR  ${kind}/${name} (tree-sitter invocation failed)`);
    console.log(fmtBlock(result.output));
    process.exit(1);
  }

  if (kind === 'positive') {
    if (result.ok) {
      console.log(`OK     positive/${name}`);
      process.exit(0);
    }
    console.log(`FAIL   positive/${name}`);
    console.log(fmtBlock(result.output));
    process.exit(1);
  }

  // negative
  const skip = SEMANTIC_ONLY_NEGATIVES.get(name);
  if (skip) {
    if (result.ok) {
      console.log(`SKIP   negative/${name} (${skip})`);
      process.exit(0);
    }
    // Overtaken by a grammar tightening: the skip entry should shrink. This is
    // a failure in per-case mode rather than a note, because there is no
    // end-of-run summary to collect notes into — the unit IS the report.
    console.log(`NOTE   negative/${name} now rejected — remove from SEMANTIC_ONLY_NEGATIVES`);
    process.exit(1);
  }
  if (result.ok) {
    console.log(`FAIL   negative/${name} (accepted, expected reject)`);
    process.exit(1);
  }
  console.log(`OK     negative/${name} (rejected)`);
  process.exit(0);
}

async function main() {
  const root = corpusRoot();
  try {
    await stat(root);
  } catch {
    console.error(`conformance corpus not found: ${root}`);
    process.exit(2);
  }
  console.log(`tree-sitter-cook conformance harness`);
  console.log(`corpus: ${root}`);
  await buildParser();
  console.log(`parser: ${PARSER_LIB}`);
  console.log('');

  const pos = await runPositives();
  console.log('');
  const neg = await runNegatives();

  console.log('');
  const failures = [...pos.failures, ...neg.failures];
  const notes = [...pos.notes, ...neg.notes];

  if (failures.length === 0 && notes.length === 0) {
    console.log('All conformance checks passed.');
    process.exit(0);
  }

  if (failures.length > 0) {
    console.log('Failures:');
    for (const f of failures) {
      console.log(`\n  ${f.name}:`);
      console.log(fmtBlock(f.output, '    '));
    }
  }
  if (notes.length > 0) {
    if (failures.length > 0) console.log('');
    console.log('Stale-list entries (audit the skip lists):');
    for (const n of notes) {
      console.log(`\n  ${n.name}:`);
      console.log(fmtBlock(n.output, '    '));
    }
  }
  process.exit(1);
}

async function selfTest() {
  const clean = await parseCase(join(corpusRoot(), 'positive', '001-empty-recipe', 'Cookfile'));
  assert.equal(clean.parsed, true);
  assert.equal(clean.ok, true);

  const syntaxError = await parseCase(join(corpusRoot(), 'negative', '001-unterminated-string', 'Cookfile'));
  assert.equal(syntaxError.parsed, true);
  assert.equal(syntaxError.ok, false);

  const infrastructureError = await parseCase(join(REPO, 'does-not-exist', 'Cookfile'));
  assert.equal(infrastructureError.parsed, false);
  assert.equal(infrastructureError.ok, false);
  assert.match(infrastructureError.output, /does-not-exist|No such file|not found|No files were found/i);

  console.log('Conformance harness self-test passed.');
}

const caseFlag = process.argv.indexOf('--case');

if (process.argv.includes('--self-test')) {
  await selfTest();
} else if (caseFlag !== -1) {
  const target = process.argv[caseFlag + 1];
  if (!target) {
    console.error('--case requires a path to a fixture Cookfile');
    process.exit(2);
  }
  await runOneCase(target);
} else {
  await main();
}
