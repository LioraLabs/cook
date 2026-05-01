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
  for (const name of cases) {
    const file = join(corpusRoot(), 'positive', name, 'Cookfile');
    const result = await parseCase(file);
    if (result.ok) {
      console.log(`OK     positive/${name}`);
    } else {
      failures.push({ name, output: result.output });
      console.log(`FAIL   positive/${name}`);
    }
  }
  return failures;
}

async function runNegatives() {
  const cases = await listCases('negative');
  const failures = [];
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
        // That is not a failure of the Standard — record it so the
        // skip list can be tightened — but do not fail the run.
        console.log(`NOTE   negative/${name} now rejected (consider removing from skip list)`);
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
  return failures;
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

  const posFailures = await runPositives();
  console.log('');
  const negFailures = await runNegatives();

  console.log('');
  if (posFailures.length === 0 && negFailures.length === 0) {
    console.log('All conformance checks passed.');
    process.exit(0);
  }

  console.log('Failures:');
  for (const f of [...posFailures, ...negFailures]) {
    console.log(`\n  ${f.name}:`);
    console.log(fmtBlock(f.output, '    '));
  }
  process.exit(1);
}

await main();
