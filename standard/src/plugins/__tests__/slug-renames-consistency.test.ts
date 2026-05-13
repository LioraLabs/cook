import { describe, it, expect } from 'vitest';
import { SLUG_RENAMES } from '../../../scripts/slug-renames.ts';
import { SLUG_MAPPING } from '../../../scripts/slug-mapping.ts';

const livingSlugs = new Set(Object.values(SLUG_MAPPING));

describe('slug-renames registry', () => {
  it('every retired slug names a replacement that exists in slug-mapping (or null)', () => {
    const missing: string[] = [];
    for (const [retired, replacement] of Object.entries(SLUG_RENAMES)) {
      if (replacement === null) continue;
      if (!livingSlugs.has(replacement)) {
        missing.push(`${retired} -> ${replacement}`);
      }
    }
    expect(missing).toEqual([]);
  });

  // Skipped until Task 8 finalises slug-mapping.ts by removing retired entries.
  // During Tasks 3–7 the existing entries for retired slugs (e.g. `grammar.*`,
  // `modules.*`, `stdmods.*`) remain in SLUG_MAPPING so that intermediate
  // commits keep the build green.
  it.skip('no retired slug is itself a living slug', () => {
    const collisions: string[] = [];
    for (const retired of Object.keys(SLUG_RENAMES)) {
      if (livingSlugs.has(retired)) {
        collisions.push(retired);
      }
    }
    expect(collisions).toEqual([]);
  });
});
