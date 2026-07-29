import { describe, it, expect } from 'vitest';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { SLUG_RENAMES } from '../../../scripts/slug-renames.ts';
import { harvestClauses, defaultContentRoot } from '../clauses.ts';

// The living slugs are the `[#slug]` anchors the chapters actually define, so
// they are harvested from the chapters — the same call, on the same content
// root, that the build itself uses to resolve `§{...}`.
//
// This used to read `Object.values(SLUG_MAPPING)`, which is a different set and
// a smaller one. SLUG_MAPPING is the v0.10 positional redirect table, keyed by
// the old `sec-N-M-K` anchors; it only ever covered sections that HAD a
// positional anchor at the reorg. Every section written since is absent from it
// by construction — `sec-22` still maps to `lua.reg`, the Lua-registration
// chapter that used to hold that number — so the check failed on five renames
// whose replacements are real, anchored, and reachable:
// `cat.probes.member-source` (§22.5.10), `exec.cache.lua-variable-reads`
// (§17.1.2), `rationale.member-index` (C.6.27), and `changes.cs0022` /
// `changes.cs0024` (D.22, D.24). It was reporting the mapping's age, not a
// defect in the renames.
//
// Harvesting also makes the check mean what its name says: a rename pointing at
// a slug no chapter defines now fails, which the old set could not detect —
// an unanchored slug that happened to sit in SLUG_MAPPING passed it.
const projectRoot = path.resolve(fileURLToPath(new URL('.', import.meta.url)), '../../..');
const livingSlugs = new Set(harvestClauses(defaultContentRoot(projectRoot)).keys());

describe('slug-renames registry', () => {
  it('every retired slug names a replacement that exists as a clause anchor (or null)', () => {
    const missing: string[] = [];
    for (const [retired, replacement] of Object.entries(SLUG_RENAMES)) {
      if (replacement === null) continue;
      if (!livingSlugs.has(replacement)) {
        missing.push(`${retired} -> ${replacement}`);
      }
    }
    expect(missing).toEqual([]);
  });

  it('no retired slug is itself a living slug', () => {
    const collisions: string[] = [];
    for (const retired of Object.keys(SLUG_RENAMES)) {
      if (livingSlugs.has(retired)) {
        collisions.push(retired);
      }
    }
    expect(collisions).toEqual([]);
  });
});
