#!/usr/bin/env node
// One-shot migration: append [#<slug>] to every numbered heading and
// rewrite every `§ N.M[.K]` ref to `§{<slug>}`. Idempotent when rerun
// over already-migrated content (the second pass is a no-op because
// headings already have markers and refs already use §{} syntax).

import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const CONTENT_ROOT = path.join(__dirname, '..', 'src', 'content', 'docs');
const MAPPING_PATH = path.join(__dirname, 'slug-mapping.ts');

// Extract the SLUG_MAPPING object literal from the .ts file.
// Cheap hand-rolled parse — the table is a flat string→string record.
function loadMapping() {
  const src = fs.readFileSync(MAPPING_PATH, 'utf8');
  const start = src.indexOf('{');
  const end = src.lastIndexOf('}');
  const body = src.slice(start + 1, end);
  const out = {};
  for (const line of body.split('\n')) {
    const m = line.match(/^\s*['"]([^'"]+)['"]\s*:\s*['"]([^'"]+)['"]\s*,?\s*(?:\/\/.*)?$/);
    if (m) out[m[1]] = m[2];
  }
  return out;
}

function numberFromSecId(id) {
  // "sec-2-3" → "2.3"; "sec-A-3" → "A.3"; "sec-5" → "5"
  return id.replace(/^sec-/, '').replace(/-/g, '.');
}

function secIdFromNumber(num) {
  return 'sec-' + num.replace(/\./g, '-');
}

function walk(root) {
  const out = [];
  for (const e of fs.readdirSync(root, { withFileTypes: true })) {
    const p = path.join(root, e.name);
    if (e.isDirectory()) out.push(...walk(p));
    else if (e.isFile() && e.name.endsWith('.mdx')) out.push(p);
  }
  return out;
}

function rewriteHeadings(src, mapping, file, unmapped) {
  // Match numbered headings; append [#slug] if absent.
  return src.replace(
    /^(#+)\s+([0-9]+|[A-Z])(?:\.([0-9]+)(?:\.([0-9]+))?)?\.(\s+)(.*?)(\s*)$/gm,
    (match, hashes, top, mid, bot, sep, title) => {
      // Already migrated? Skip.
      if (/\[#[a-z][a-z0-9.-]*\]\s*$/.test(title)) return match;
      const num = [top, mid, bot].filter(Boolean).join('.');
      const secId = secIdFromNumber(num);
      const slug = mapping[secId];
      if (!slug) {
        unmapped.push(`${file}: ${secId} "${title.trim()}"`);
        return match;
      }
      return `${hashes} ${num}. ${title.replace(/\s+$/, '')} [#${slug}]`;
    },
  );
}

function rewriteRefs(src, mapping, file, unmapped) {
  // Match bare § N.M refs. Skip inside fenced code blocks and inline code.
  // Simple approach: split on fence boundaries, rewrite only outside.
  const segments = src.split(/(```[\s\S]*?```|`[^`\n]*`)/g);
  for (let i = 0; i < segments.length; i++) {
    // Odd indices are code; even indices are prose.
    if (i % 2 === 1) continue;
    segments[i] = segments[i].replace(
      /§\s+([0-9]+|[A-Z])(?:\.([0-9]+)(?:\.([0-9]+))?)?(?=\b|[^0-9A-Za-z])/g,
      (match, top, mid, bot) => {
        const num = [top, mid, bot].filter(Boolean).join('.');
        const secId = secIdFromNumber(num);
        const slug = mapping[secId];
        if (!slug) {
          unmapped.push(`${file}: § ${num} (no mapping for ${secId})`);
          return match;
        }
        return `§{${slug}}`;
      },
    );
  }
  return segments.join('');
}

function main() {
  const mapping = loadMapping();
  const files = walk(CONTENT_ROOT);
  const unmapped = [];

  for (const abs of files) {
    const rel = path.relative(CONTENT_ROOT, abs);
    let src = fs.readFileSync(abs, 'utf8');
    src = rewriteHeadings(src, mapping, rel, unmapped);
    src = rewriteRefs(src, mapping, rel, unmapped);
    fs.writeFileSync(abs, src);
  }

  if (unmapped.length > 0) {
    console.error('Unmapped clauses encountered:');
    for (const u of unmapped) console.error('  ' + u);
    process.exit(1);
  }
  console.log(`Migrated ${files.length} files`);
}

main();
