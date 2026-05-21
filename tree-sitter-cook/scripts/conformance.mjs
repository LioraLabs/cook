#!/usr/bin/env node
// Tree-sitter conformance harness.
//
// Walks `standard/conformance/{positive,negative}` and asserts that
// `tree-sitter-cook` agrees with the Cook Standard at the syntactic
// level: every positive Cookfile parses without ERROR/MISSING nodes,
// and every syntactic negative Cookfile produces at least one
// ERROR/MISSING node.
//
// Cases listed in `SEMANTIC_ONLY_NEGATIVES` express constraints that
// the Standard places on a parser but that tree-sitter cannot enforce
// from the grammar alone (top-level ordering, duplicate-step
// detection, accessor-string templating). The harness records them as
// "accepted" without failing — they are the Rust parser's territory.
//
// Cases listed in `KNOWN_STALE_POSITIVES` express grammar features
// that the Cook Standard adopted post-CS-0022 (multi-line Lua opaque
// spans, shell heredocs, single-quoted STRING) but that tree-sitter-
// cook has not yet been updated for. The Rust parser is the v0.x
// reference implementation per pre-v1 checklist E.4; bringing tree-
// sitter into conformance is tracked under CS-0002. The harness
// records these as STALE without failing.
//
// Override the corpus path with COOK_CONFORMANCE_CORPUS=/path/to/dir.
// Default is `../standard/conformance/` relative to this script.

import { execFile } from 'node:child_process';
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
  ['006-accessor-in-using-string',
   'accessor placeholders inside using strings — templater rule, not syntactic'],
  ['008-imperative-then-declarative',
   'recipe-body region rule (Note 4.4.2) — semantic, not syntactic'],
  ['010-triple-arrow-prefix',
   '>>> reservation (§{lexical.line-prefixes}) — tree-sitter accepts as `>>` + content `>...`'],
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
  ['022-lib-accessor-in-using-rejected',
   'CS-0022: {lib.ACCESSOR} inside using-clause body — codegen rejection, not syntactic'],
  ['023-multi-output-one-to-one-mixed-rejected',
   'CS-0022: mixed one-to-one + literal output patterns — codegen rejection, not syntactic'],
  // CS-0024: plate/test mode-deduction and placeholder rejections.
  // Cookfile parses cleanly; rejection is enforced by cook-luagen's
  // validate_plate_test_placeholders and detect_plate_test_mode.
  ['024-plate-out-rejected',
   'CS-0024: {out} in plate body — codegen rejection, not syntactic'],
  ['025-plate-mixed-in-and-all',
   'CS-0024: mixed {in} and {all} in plate body — codegen rejection, not syntactic'],
  ['026-plate-mixed-input-and-inputs',
   'CS-0024: mixed input and inputs in Lua plate body — codegen rejection, not syntactic'],
  ['027-plate-lib-accessor-rejected',
   'CS-0024: {lib.ACCESSOR} in plate body — codegen rejection, not syntactic'],
  ['028-plate-bare-stem-rejected',
   'CS-0024: bare {stem} in plate body — codegen rejection, not syntactic'],
  ['031-one-to-one-empty-source-rejected',
   'CS-0024: plate one-to-one mode with no source — codegen rejection, not syntactic'],
  ['032-test-empty-source-rejected',
   'CS-0024: test one-to-one mode with no source — codegen rejection, not syntactic'],
  ['033-many-to-one-empty-source-rejected',
   'CS-0024: plate many-to-one mode with no source — codegen rejection, not syntactic'],
  // CS-0026: parse-time import-path enforcement. Cookfile parses cleanly;
  // rejection is enforced by the Rust parser's import_declaration validator.
  ['034-import-dotdot-rejected',
   'CS-0026: import path with `..` segments — parse-time semantic rejection, not syntactic'],
  ['035-import-absolute-rejected',
   'CS-0026: absolute import path — parse-time semantic rejection, not syntactic'],
  ['036-import-sigil-dotdot-rejected',
   'CS-0026: sigil import path with `..` segments — parse-time semantic rejection, not syntactic'],
  // CS-0035: env reserved-word check — closed: `env.*` recipe names are
  // rejected by the grammar's `_decl_name` (no-dots rule, COOK-55) which
  // means `env.foo` produces ERROR before any reserved-name check fires.
  // CS-0033: sigil-placeholder semantic rules.
  ['038-out-zero-rejected-sigil',
   'CS-0033: `$<out_0>` (zero index) — codegen rejection, not syntactic'],
  ['039-in-in-many-to-one-sigil',
   'CS-0033: `$<in>` in many-to-one body — codegen rejection, not syntactic'],
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
]);

// Positive fixtures the Rust parser accepts but tree-sitter-cook cannot
// yet parse cleanly because the grammar lags the Cook Standard. Per
// pre-v1 checklist E.4, the Rust parser is the v0.x reference; closing
// these is part of the CS-0002 follow-up to bring tree-sitter into
// conformance.
const KNOWN_STALE_POSITIVES = new Map([
  // CS-0035 multi-line Lua opaque-span tracking — closed by COOK-53.
  // CS-0035 POSIX heredoc opaque-span tracking — closed by COOK-54.
  // CS-0061 single-quoted STRING — closed by COOK-55.
  // (List is empty; the conformance audit is complete for v0.12.
  // Re-add entries here for fixtures that exercise a newly-spec'd
  // grammar feature pending implementation.)
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
  try {
    await exec('tree-sitter', ['parse', '-q', file], { cwd: REPO });
    return { ok: true, output: '' };
  } catch (err) {
    return {
      ok: false,
      code: err.code ?? null,
      output: (err.stdout || '') + (err.stderr || ''),
    };
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
    const stale = KNOWN_STALE_POSITIVES.get(name);
    if (result.ok) {
      if (stale) {
        // Tree-sitter accepted something we listed as known-stale.
        // Post-CS-0086 (v0.12 audit), KNOWN_STALE_POSITIVES is
        // expected to be empty; any entry here that now passes is
        // a regression in the skip list and is fatal.
        console.log(`NOTE   positive/${name} now parses cleanly — remove from KNOWN_STALE_POSITIVES`);
        notes.push({ name, output: 'KNOWN_STALE_POSITIVES entry now parses cleanly' });
      } else {
        console.log(`OK     positive/${name}`);
      }
      continue;
    }
    if (stale) {
      console.log(`STALE  positive/${name} (${stale})`);
      // Treat STALE as a soft failure too — the v0.12 audit closed
      // every entry; a new STALE entry signals a re-divergence.
      notes.push({ name, output: `STALE: ${stale}` });
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

await main();
