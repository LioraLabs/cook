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
import { readdir, stat } from 'node:fs/promises';
import { dirname, join, basename } from 'node:path';
import { fileURLToPath } from 'node:url';

const exec = promisify(execFile);
const HERE = dirname(fileURLToPath(import.meta.url));
const REPO = dirname(HERE);

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
  ['ingredients-probe-artifact-dep',
   'CS-0095: probe member source with artifact dep — register-phase rejection, not syntactic'],
  // CS-0101: `$<file:PATH>` parses cleanly anywhere a placeholder does;
  // these rejections are codegen-phase / register-phase.
  ['cs0101-file-ref-in-output-pattern',
   'CS-0101: file-ref placeholder in an output pattern — codegen-phase rejection, not syntactic'],
  ['cs0101-file-ref-missing-file',
   'CS-0101: file-ref to a missing file — register-phase rejection, not syntactic'],
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
  // CS-0153: `cook.add_unit({step_kind = "test"})` is an ordinary spec-table
  // field syntactically; only the register pass knows a test work unit is
  // registrable solely through `cook.add_test` (§22.4).
  ['add-unit-step-kind-test-rejected',
   'CS-0153: step_kind = "test" on cook.add_unit — register-phase rejection, not syntactic'],
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
    const { stdout, stderr } = await exec('tree-sitter', ['parse', file], { cwd: REPO });
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

if (process.argv.includes('--self-test')) {
  await selfTest();
} else {
  await main();
}
